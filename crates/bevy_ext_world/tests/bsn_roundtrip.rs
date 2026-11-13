//! Round-trip XML → `.bsn` → `XmlNode` e overrides da camada de editor.

use std::fs;
use std::path::Path;

use bevy_ext_world::bsn::{
    EmitOptions, emit_file, load_bsn_world, load_bsn_world_in, worlds_equivalent,
};
use bevy_ext_world::xml::include::load_world;

fn roundtrip(dir: &Path, xml: &Path, bsn: &Path) {
    let text = emit_file(xml, dir, EmitOptions::default()).expect("emitir bsn");
    fs::write(bsn, text).expect("gravar bsn");

    let from_xml = load_world(xml).expect("xml");
    let from_bsn = load_world(bsn).expect("bsn");
    worlds_equivalent(
        &from_xml.root_attrs,
        &from_xml.nodes,
        &from_bsn.root_attrs,
        &from_bsn.nodes,
    )
    .unwrap_or_else(|err| panic!("{err}"));
}

#[test]
fn xml_vira_bsn_e_volta_igual() {
    let dir = tempfile::tempdir().expect("tempdir");
    let xml = dir.path().join("world.xml");
    fs::write(
        &xml,
        r##"<world static="true" title="Teste">
  <Entity name="a" translation="1 2 3" euler="0 90 0">
    <GltfScene url="/assets/x.glb" lod1-url="/assets/x_lod1.glb" />
    <PointLight color="#ff0000" intensity="500" shadows="1" />
  </Entity>
  <Group name="g">
    <Entity name="b" transform="pos: 4 5 6; euler: 10 20 30; scale: 2" />
  </Group>
</world>
"##,
    )
    .expect("xml");
    let bsn = dir.path().join("world.bsn");
    roundtrip(dir.path(), &xml, &bsn);
}

#[test]
fn includes_em_bsn_expandem_como_xml() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(
        dir.path().join("part.xml"),
        r#"<world><Entity name="child"><GltfScene url="/assets/c.glb" /></Entity></world>"#,
    )
    .expect("part");
    let xml = dir.path().join("world.xml");
    fs::write(
        &xml,
        r#"<world static="true">
  <Entity name="root">
    <Include src="part.xml" translation="0 0 5" />
  </Entity>
</world>
"#,
    )
    .expect("world");
    let part_bsn = dir.path().join("part.bsn");
    fs::write(
        &part_bsn,
        emit_file(
            &dir.path().join("part.xml"),
            dir.path(),
            EmitOptions::default(),
        )
        .expect("emit part"),
    )
    .expect("part bsn");
    let bsn = dir.path().join("world.bsn");
    roundtrip(dir.path(), &xml, &bsn);
}

#[test]
fn transform_e_name_do_editor_mandam_nos_attrs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let xml = dir.path().join("world.xml");
    fs::write(
        &xml,
        r#"<world static="true"><Entity name="a" translation="1 2 3"><PointLight intensity="500" /></Entity></world>"#,
    )
    .expect("xml");
    let text = emit_file(&xml, dir.path(), EmitOptions::default()).expect("emit");
    let text = text.replace("x: 1.0, y: 2.0, z: 3.0", "x: 10.0, y: 20.0, z: 30.0");
    let text = text.replace("#\"a\"", "#\"renamed\"");
    let text = text.replace("intensity: 500.0", "intensity: 900.0");
    let bsn = dir.path().join("world.bsn");
    fs::write(&bsn, text).expect("bsn");

    let world = load_bsn_world(&bsn).expect("load");
    let node = &world.nodes[0];
    assert_eq!(node.tag, "Entity");
    assert_eq!(node.attr("name"), Some("renamed"));
    assert_eq!(node.attr("translation"), Some("10 20 30"));
    let light = &node.children[0];
    assert_eq!(light.tag, "PointLight");
    assert_eq!(light.attr("intensity"), Some("900"));
}

#[test]
fn gltf_source_do_editor_muda_o_url() {
    let dir = tempfile::tempdir().expect("tempdir");
    let xml = dir.path().join("world.xml");
    fs::write(
        &xml,
        r#"<world static="true"><GltfScene name="m" url="/assets/old.glb" /></world>"#,
    )
    .expect("xml");
    let text = emit_file(&xml, dir.path(), EmitOptions::default()).expect("emit");
    // O emissor já não escreve `GltfSource`; documentos antigos (ou um
    // override do editor) ainda o trazem e mandam no `url`.
    assert!(!text.contains("GltfSource"), "{text}");
    let at = text
        .rfind("bevy_world_xml::bsn::WorldNode")
        .expect("WorldNode");
    let indent = &text[text[..at].rfind('\n').map_or(0, |i| i + 1)..at];
    let text = format!(
        "{}jackdaw_scene_types::types::GltfSource {{ path: \"assets/new.glb\", scene_index: 0 }}\n{indent}{}",
        &text[..at],
        &text[at..]
    );
    let bsn = dir.path().join("world.bsn");
    fs::write(&bsn, text).expect("bsn");

    let world = load_bsn_world(&bsn).expect("load");
    assert_eq!(world.nodes[0].attr("url"), Some("/assets/new.glb"));
}

#[test]
fn include_em_memoria_nao_depende_de_ficheiros_bsn() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(
        dir.path().join("part.xml"),
        r#"<world><Entity name="child"><GltfScene url="/assets/c.glb" /></Entity></world>"#,
    )
    .expect("part");
    let xml = dir.path().join("world.xml");
    fs::write(
        &xml,
        r#"<world static="true"><Include src="part.xml" /></world>"#,
    )
    .expect("world");
    let text = emit_file(&xml, dir.path(), EmitOptions::default()).expect("emit");
    let bsn = dir.path().join("world.bsn");
    fs::write(&bsn, text).expect("bsn");

    let world = load_bsn_world_in(&bsn, dir.path()).expect("load");
    assert_eq!(world.nodes.len(), 1);
    assert_eq!(world.nodes[0].tag, "Entity");
    assert_eq!(world.nodes[0].attr("name"), Some("child"));
}

/// `<Include>` vira `IsA` (prefab do Jackdaw): o editor expande o módulo e o
/// loader resolve-o com o mesmo `jackdaw_prefab`.
#[test]
fn include_vira_instancia_de_prefab() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(dir.path().join("world/sub")).expect("dirs");
    let part = dir.path().join("world/sub/part.xml");
    fs::write(
        &part,
        r#"<world><Entity name="child" translation="1 2 3"><GltfScene url="/assets/c.glb" /></Entity></world>"#,
    )
    .expect("part");
    fs::write(
        dir.path().join("world/sub/part.bsn"),
        emit_file(&part, dir.path(), EmitOptions::default()).expect("emit part"),
    )
    .expect("part bsn");
    let xml = dir.path().join("world.xml");
    fs::write(
        &xml,
        r#"<world static="true"><Include src="/world/sub/part.xml" /></world>"#,
    )
    .expect("world");
    let text = emit_file(&xml, dir.path(), EmitOptions::default()).expect("emit");
    assert!(
        text.contains("jackdaw::prefab::components::IsA { source: \"world/sub/part.bsn\""),
        "{text}"
    );
    assert!(text.contains("#\"part\""), "{text}");
    let bsn = dir.path().join("world.bsn");
    fs::write(&bsn, &text).expect("bsn");

    let world = load_bsn_world_in(&bsn, dir.path()).expect("load");
    assert_eq!(world.nodes.len(), 1, "{:?}", world.nodes);
    let child = &world.nodes[0];
    assert_eq!(child.tag, "Entity");
    assert_eq!(child.attr("name"), Some("child"));
    assert_eq!(child.attr("translation"), Some("1 2 3"));
    // O nome derivado do `GltfScene` não vira atributo.
    assert_eq!(child.children[0].attr("name"), None);

    // Mover a instância no editor (Transform) embrulha o módulo num Group.
    let moved = text.replacen(
        "translation: glam::Vec3 { x: 0.0, y: 0.0, z: 0.0 },\n        rotation: glam::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },\n        scale: glam::Vec3 { x: 1.0, y: 1.0, z: 1.0 },\n    }\n    jackdaw::prefab",
        "translation: glam::Vec3 { x: 5.0, y: 0.0, z: 0.0 },\n        rotation: glam::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },\n        scale: glam::Vec3 { x: 1.0, y: 1.0, z: 1.0 },\n    }\n    jackdaw::prefab",
        1,
    );
    assert_ne!(
        moved, text,
        "a pose da instância não foi encontrada:\n{text}"
    );
    fs::write(&bsn, &moved).expect("bsn");
    let world = load_bsn_world_in(&bsn, dir.path()).expect("load moved");
    assert_eq!(world.nodes[0].tag, "Group");
    assert_eq!(world.nodes[0].attr("translation"), Some("5 0 0"));
    assert_eq!(world.nodes[0].children[0].attr("name"), Some("child"));
}

/// A cor das luzes vai como `Color::Srgba(Srgba { .. })` — a forma que o
/// Jackdaw aplica — e volta igual ao loader.
#[test]
fn cor_da_luz_no_formato_do_bevy() {
    let dir = tempfile::tempdir().expect("tempdir");
    let xml = dir.path().join("world.xml");
    fs::write(
        &xml,
        r##"<world static="true"><PointLight name="l" color="#ff8000" intensity="900" /></world>"##,
    )
    .expect("xml");
    let text = emit_file(&xml, dir.path(), EmitOptions::default()).expect("emit");
    assert!(
        text.contains("bevy_color::color::Color::Srgba(bevy_color::srgba::Srgba { red: 1.0"),
        "{text}"
    );
    let bsn = dir.path().join("world.bsn");
    fs::write(&bsn, text).expect("bsn");
    let world = load_bsn_world(&bsn).expect("load");
    assert_eq!(world.nodes[0].attr("color"), Some("#ff8000"));
}

#[derive(bevy::prelude::Reflect, Clone, Debug, PartialEq)]
#[type_path = "teste"]
struct Ajustes {
    hora: f64,
    nome: String,
    ligado: bool,
}

impl Default for Ajustes {
    fn default() -> Self {
        Self {
            hora: 8.0,
            nome: "padrão".into(),
            ligado: true,
        }
    }
}

/// Pastas sem `WorldNode` são transparentes (os filhos sobem) e as entidades
/// com componentes próprios chegam ao jogo como definições tipadas.
#[test]
fn pasta_transparente_e_definicoes_nativas() {
    let dir = tempfile::tempdir().expect("tempdir");
    let xml = dir.path().join("world.xml");
    fs::write(
        &xml,
        r#"<world static="true"><Entity name="a" translation="1 0 0" /></world>"#,
    )
    .expect("xml");
    let text = emit_file(&xml, dir.path(), EmitOptions::default()).expect("emit");
    // Embrulhar o filho numa pasta e juntar uma entidade de definições.
    let (head, rest) = text
        .split_once("bevy_ecs::hierarchy::Children [\n")
        .expect("children");
    let body = rest.trim_end().strip_suffix(']').expect("fecho");
    let wrapped = format!(
        "{head}bevy_ecs::hierarchy::Children [\n    #\"Definições\"\n    teste::Ajustes {{ hora: 17.5 }}\n    --\n    #\"Pasta\"\n    bevy_transform::components::transform::Transform\n    bevy_ecs::hierarchy::Children [\n{}\n    ]\n]\n",
        body.lines()
            .map(|line| format!("    {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let bsn = dir.path().join("world.bsn");
    fs::write(&bsn, &wrapped).expect("bsn");

    let world = load_bsn_world(&bsn).unwrap_or_else(|err| panic!("{err:#}\n{wrapped}"));
    assert_eq!(world.nodes.len(), 1, "{:?}", world.nodes);
    assert_eq!(world.nodes[0].attr("name"), Some("a"));
    assert_eq!(world.nodes[0].attr("translation"), Some("1 0 0"));

    let ajustes: Ajustes =
        bevy_ext_world::bsn::settings::native_value(&world.native).expect("definição");
    assert_eq!(
        ajustes,
        Ajustes {
            hora: 17.5,
            ..Default::default()
        }
    );
}

#[test]
fn componentes_tipados_migram_sem_perdas() {
    let dir = tempfile::tempdir().expect("tempdir");
    let xml = dir.path().join("world.xml");
    fs::write(
        &xml,
        r#"<world static="true">
  <GltfScene name="a" url="/assets/tree.glb"
    collider="shape: cylinder; radius: 0.45; half-height: 2.2"
    rigidbody="type: fixed; mass: 0; gravity-scale: 0"
    destructible="popup-text: Pedra; popup-color: #cccccc; preset: dust; burst-count: 22; hits: 3; spark-on-hit: 1; hit-preset: rockshards; hit-burst-count: 10; crack-on-hit: 1; break-style: shatter"
    resource-kind="stone" resource-yield="2" />
  <GltfScene name="b" url="/assets/b.glb" collider="shape: box; size: 0.80 1 1" />
</world>"#,
    )
    .expect("xml");
    let text = emit_file(&xml, dir.path(), EmitOptions::default()).expect("emit");
    let before = dir.path().join("before.bsn");
    fs::write(&before, &text).expect("before");

    let (migrated, report) = bevy_ext_world::bsn::specs::migrate_text(&text);
    assert_eq!(report.nodes, 1, "{migrated}");
    assert_eq!(report.components.values().sum::<usize>(), 4, "{report:?}");
    assert!(migrated.contains("ColliderSpec"), "{migrated}");
    assert!(!migrated.contains("key: \"destructible\""), "{migrated}");
    // O colisor não canónico do `b` fica no DSL.
    assert!(migrated.contains("size: 0.80 1 1"), "{migrated}");
    // Idempotente.
    assert_eq!(
        bevy_ext_world::bsn::specs::migrate_text(&migrated).0,
        migrated
    );

    let after = dir.path().join("after.bsn");
    fs::write(&after, &migrated).expect("after");
    let a = load_bsn_world(&before).expect("load before");
    let b = load_bsn_world(&after).expect("load after");
    let same =
        bevy_ext_world::bsn::worlds_equivalent(&a.root_attrs, &a.nodes, &b.root_attrs, &b.nodes);
    assert!(same.is_ok(), "{same:?}\n{migrated}");
}

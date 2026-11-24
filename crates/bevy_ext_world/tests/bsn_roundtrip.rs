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

/// Câmara/áudio/bioma migram pela tag: `at`, `size` e `id` também existem
/// noutras tags e um lift global mudava o significado do atributo.
#[test]
fn camaras_audio_e_biomas_migram_por_tag() {
    let dir = tempfile::tempdir().expect("tempdir");
    let xml = dir.path().join("world.xml");
    fs::write(
        &xml,
        r##"<world static="true">
  <Entity name="cam"><OrbitCamera distance="3.3" height="1.55" pitch="18.3" fov="64" follow-lag="0.18" turn-lag="0.38" min-terrain-distance="1" /></Entity>
  <Entity name="sala"><InteriorScene at="0 0" size="60 55" camera-distance="16" camera-pitch="58" camera-yaw="0" /></Entity>
  <Entity name="mix"><AudioMixer master="1" music="0.7" sfx="0.8" voice="0.9" ambience="0.7" /></Entity>
  <Entity name="mus"><MusicLayer layer="explore" sound="bgm-explore" base-volume="0.18" /></Entity>
  <Entity name="bioma"><BiomeRegion id="dark-forest" display-name="Floresta Sombria" polygon="[-56,56;56,56]" tint="#1a3320" fog-density="0.0008" /></Entity>
  <Entity name="zona"><SpawnExclusion at="140 360" radius="18" /></Entity>
</world>"##,
    )
    .expect("xml");
    let text = emit_file(&xml, dir.path(), EmitOptions::default()).expect("emit");
    let (migrated, report) = bevy_ext_world::bsn::specs::migrate_text(&text);
    assert_eq!(report.nodes, 6, "{migrated}");
    for component in [
        "bevy_ext_scene::CameraSpec",
        "bevy_ext_scene::InteriorSceneSpec",
        "bevy_ext_scene::AudioMixerSpec",
        "bevy_ext_scene::MusicLayerSpec",
        "bevy_ext_scene::BiomeRegionSpec",
        "bevy_ext_scene::SpawnExclusionSpec",
    ] {
        assert!(migrated.contains(component), "falta {component}:\n{migrated}");
    }
    // O `at`/`radius` da exclusão não foram para a câmara nem o contrário.
    assert!(!migrated.contains("key: \"camera-distance\""));
    assert!(!migrated.contains("key: \"polygon\""));
    assert_eq!(
        bevy_ext_world::bsn::specs::migrate_text(&migrated).0,
        migrated
    );

    let before = dir.path().join("before.bsn");
    fs::write(&before, &text).expect("before");
    let after = dir.path().join("after.bsn");
    fs::write(&after, &migrated).expect("after");
    let a = load_bsn_world(&before).expect("load before");
    let b = load_bsn_world(&after).expect("load after");
    let same =
        bevy_ext_world::bsn::worlds_equivalent(&a.root_attrs, &a.nodes, &b.root_attrs, &b.nodes);
    assert!(same.is_ok(), "{same:?}\n{migrated}");
}

/// Um `<Entity>` com `radius` e `at` (como o `SpawnExclusion` usa) NÃO é
/// câmara: o lift continua a ser por tag.
#[test]
fn atributos_partilhados_nao_migram_fora_da_tag() {
    let attrs = vec![
        ("at".to_string(), "1 2".to_string()),
        ("radius".to_string(), "6".to_string()),
    ];
    let lifted = bevy_ext_world::bsn::specs::lift_specs("Entity", &attrs);
    assert!(lifted.components.is_empty(), "{:?}", lifted.consumed);
    let lifted = bevy_ext_world::bsn::specs::lift_specs("OrbitCamera", &attrs);
    assert!(lifted.components.is_empty(), "câmara não tem at/radius");
}

/// A cerca levanta pela chave (`fence-*` só existem nela), em qualquer tag, e
/// a migração não muda o mundo: antes e depois carregam igual.
#[test]
fn cercas_migram_pela_chave_em_qualquer_tag() {
    let dir = tempfile::tempdir().expect("tempdir");
    let xml = dir.path().join("world.xml");
    fs::write(
        &xml,
        r##"<world static="true">
  <Entity name="cerca" fence-style="pasture_rail" fence-points="-24.6 0 0; 5.9 0 0" fence-height="1.2" fence-closed="1" />
  <Group name="curral" translation="4 0 4"><Entity fence-points="0 0 3 0" fence-kind="wattle" fence-board-length="0.9 1.1" fence-seed="7" /></Group>
</world>"##,
    )
    .expect("xml");
    let text = emit_file(&xml, dir.path(), EmitOptions::default()).expect("emit");
    let (migrated, report) = bevy_ext_world::bsn::specs::migrate_text(&text);
    assert_eq!(report.nodes, 2, "{migrated}");
    assert_eq!(
        migrated.matches("bevy_ext_scene::FenceSpec").count(),
        2,
        "{migrated}"
    );
    assert!(!migrated.contains("key: \"fence-"), "{migrated}");
    assert_eq!(
        bevy_ext_world::bsn::specs::migrate_text(&migrated).0,
        migrated,
        "idempotente"
    );

    let before = dir.path().join("before.bsn");
    fs::write(&before, &text).expect("before");
    let after = dir.path().join("after.bsn");
    fs::write(&after, &migrated).expect("after");
    let a = load_bsn_world(&before).expect("load before");
    let b = load_bsn_world(&after).expect("load after");
    let same =
        bevy_ext_world::bsn::worlds_equivalent(&a.root_attrs, &a.nodes, &b.root_attrs, &b.nodes);
    assert!(same.is_ok(), "{same:?}\n{migrated}");
}

/// Um `fence-*` sem linha não é cerca: fica no `WorldNode` para o lint do
/// mundo o acusar, em vez de uma `FenceSpec` vazia.
#[test]
fn cerca_sem_linha_nao_migra() {
    let attrs = vec![
        ("name".to_string(), "cerca".to_string()),
        ("fence-style".to_string(), "wattle".to_string()),
    ];
    let lifted = bevy_ext_world::bsn::specs::lift_specs("Entity", &attrs);
    assert!(lifted.components.is_empty(), "{:?}", lifted.consumed);
}

/// Um nó que já tem o componente (e o atributo homónimo, como fica depois de
/// o editor voltar a gravar) não ganha um segundo componente.
#[test]
fn migracao_nao_duplica_componente_existente() {
    let text = r#"#"world"
bevy_world_xml::bsn::WorldNode {
    tag: "world",
    attrs: [],
    text: "",
}
bevy_ecs::hierarchy::Children [
    #"fumo"
    bevy_ext_scene::ParticleSpec {
        preset: "smoke",
    }
    bevy_world_xml::bsn::WorldNode {
        tag: "ParticleSystem",
        attrs: [
            bevy_world_xml::bsn::WorldProp { key: "preset", value: "smoke" },
        ],
        text: "",
    }
]
"#;
    let (migrated, report) = bevy_ext_world::bsn::specs::migrate_text(text);
    assert_eq!(report.nodes, 0, "{migrated}");
    assert_eq!(migrated.matches("bevy_ext_scene::ParticleSpec").count(), 1);
    assert_eq!(migrated, text, "nada mudou");
}

/// O `SceneNodeId` do documento fica disponível pela origem do nó: é a chave
/// que o jogo usa para receber as edições do Play do Jackdaw.
#[test]
fn scene_node_id_fica_registado_pela_origem() {
    let dir = std::env::temp_dir().join(format!("bsn_node_id_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.yaml"), "assets:\n  roots: assets\n").unwrap();
    let path = dir.join("world.bsn");
    std::fs::write(
        &path,
        r#"#"world"
bevy_world_xml::bsn::WorldNode {
    tag: "world",
    attrs: [],
    text: "",
}
bevy_ecs::hierarchy::Children [
    #"poço"
    bevy_transform::components::transform::Transform {
        translation: glam::Vec3 { x: 3.0, y: 0.0, z: 0.0 },
        rotation: glam::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },
        scale: glam::Vec3 { x: 1.0, y: 1.0, z: 1.0 },
    }
    bevy_world_xml::bsn::WorldNode {
        tag: "Entity",
        attrs: [],
        text: "",
    }
    jackdaw_scene_types::node_id::SceneNodeId(4294967301)
    jackdaw_animation_runtime::AnimationState("idle")
]
"#,
    )
    .unwrap();
    let world = bevy_ext_world::bsn::load::load_bsn_world(&path).expect("carrega");
    let node = world
        .nodes
        .iter()
        .find(|n| n.tag == "Entity")
        .expect("o nó");
    let origin = node.origin.as_ref().expect("origem");
    assert_eq!(
        bevy_ext_world::bsn::specs::scene_node_id_for(origin),
        Some(4294967301)
    );
    // O componente que o DSL não conhece fica guardado para o jogo; os
    // consumidos (WorldNode/Transform/SceneNodeId) não.
    let native = bevy_ext_world::bsn::specs::native_components_for(origin).expect("nativos");
    let paths: Vec<&str> = native
        .iter()
        .filter_map(jackdaw_bsn::patch_type_path)
        .collect();
    assert_eq!(paths, ["jackdaw_animation_runtime::AnimationState"]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// O nó fica registado com os atributos crus e os finais; `live_attrs`
/// recompõe-nos com o que o editor mandou, mantendo os derivados.
#[test]
fn live_attrs_recompoe_o_no_editado() {
    let dir = std::env::temp_dir().join(format!("bsn_live_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.yaml"), "assets:\n  roots: assets\n").unwrap();
    let path = dir.join("world.bsn");
    std::fs::write(
        &path,
        r#"#"world"
bevy_world_xml::bsn::WorldNode {
    tag: "world",
    attrs: [],
    text: "",
}
bevy_ecs::hierarchy::Children [
    #"pedra"
    bevy_transform::components::transform::Transform {
        translation: glam::Vec3 { x: 3.0, y: 0.0, z: 0.0 },
        rotation: glam::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },
        scale: glam::Vec3 { x: 1.0, y: 1.0, z: 1.0 },
    }
    bevy_world_xml::bsn::WorldNode {
        tag: "GltfScene",
        attrs: [
            bevy_world_xml::bsn::WorldProp { key: "url", value: "/a.glb" },
        ],
        text: "",
        pose: true,
    }
]
"#,
    )
    .unwrap();
    let world = bevy_ext_world::bsn::load::load_bsn_world(&path).expect("carrega");
    let node = world.nodes.iter().find(|n| n.tag == "GltfScene").unwrap();
    let origin = node.origin.as_ref().unwrap();
    let recorded = bevy_ext_world::bsn::specs::node_for(origin).expect("registado");
    assert!(!recorded.has_children);
    assert_eq!(recorded.raw_attrs, vec![("url".into(), "/a.glb".into())]);
    // Derivado do load: o nome autoral e a pose.
    assert!(recorded.attrs.iter().any(|(k, v)| k == "name" && v == "pedra"));

    let raw = vec![("url".to_string(), "/b.glb".to_string())];
    let specs = bevy_ext_scene::NodeSpecs {
        script: Some(bevy_ext_scene::ScriptRef {
            path: "pedra.lua".into(),
        }),
        ..Default::default()
    };
    let attrs = bevy_ext_world::bsn::specs::live_attrs(&recorded, Some(&raw), &specs, None);
    let get = |key: &str| attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
    assert_eq!(get("url"), Some("/b.glb"), "o editor ganha");
    assert_eq!(get("name"), Some("pedra"), "derivado mantém-se");
    assert_eq!(get("script"), Some("pedra.lua"), "spec escrito");
    assert!(get("translation").is_some(), "pose do load mantém-se");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Um nó com filhos no mesmo ficheiro tem a subárvore recomposta a partir
/// do registo do load (re-spawn a quente de um grupo).
#[test]
fn subarvore_registada_recompoe_os_filhos() {
    let dir = std::env::temp_dir().join(format!("bsn_sub_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.yaml"), "assets:\n  roots: assets\n").unwrap();
    let path = dir.join("world.bsn");
    std::fs::write(
        &path,
        r#"#"world"
bevy_world_xml::bsn::WorldNode {
    tag: "world",
    attrs: [],
    text: "",
}
bevy_ecs::hierarchy::Children [
    #"acampamento"
    bevy_world_xml::bsn::WorldNode {
        tag: "Group",
        attrs: [],
        text: "",
    }
    bevy_ecs::hierarchy::Children [
        #"fogueira"
        bevy_world_xml::bsn::WorldNode {
            tag: "GltfScene",
            attrs: [
                bevy_world_xml::bsn::WorldProp { key: "url", value: "/f.glb" },
            ],
            text: "",
        }
        --
        #"tenda"
        bevy_world_xml::bsn::WorldNode {
            tag: "GltfScene",
            attrs: [
                bevy_world_xml::bsn::WorldProp { key: "url", value: "/t.glb" },
            ],
            text: "",
        }
    ]
]
"#,
    )
    .unwrap();
    let world = bevy_ext_world::bsn::load::load_bsn_world(&path).expect("carrega");
    let group = world.nodes.iter().find(|n| n.tag == "Group").expect("grupo");
    let origin = group.origin.as_ref().unwrap();
    let node = bevy_ext_world::bsn::specs::node_for(origin).unwrap();
    assert!(node.has_children);
    let kids = bevy_ext_world::bsn::specs::recorded_subtree(origin).expect("subárvore");
    let urls: Vec<&str> = kids
        .iter()
        .filter_map(|k| k.attrs.iter().find(|(key, _)| key == "url").map(|(_, v)| v.as_str()))
        .collect();
    assert_eq!(urls, ["/f.glb", "/t.glb"]);
    assert_eq!(kids.len(), group.children.len());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Um spec removido no editor leva o seu atributo: os atributos derivados
/// de specs são recompostos só a partir dos specs actuais.
#[test]
fn spec_removido_sai_dos_atributos() {
    let recorded = bevy_ext_world::bsn::specs::RecordedNode {
        tag: "GltfScene".into(),
        raw_attrs: vec![("url".into(), "/a.glb".into())],
        attrs: vec![
            ("url".into(), "/a.glb".into()),
            ("name".into(), "pedra".into()),
            ("collider".into(), "shape: box".into()),
            ("prop-x".into(), "1".into()),
        ],
        text: String::new(),
        has_children: false,
    };
    let empty = bevy_ext_scene::NodeSpecs::default();
    for raw in [None, Some(recorded.raw_attrs.as_slice())] {
        let attrs = bevy_ext_world::bsn::specs::live_attrs(&recorded, raw, &empty, None);
        let keys: Vec<&str> = attrs.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"url") && keys.contains(&"name"), "{keys:?}");
        assert!(!keys.contains(&"collider") && !keys.contains(&"prop-x"), "{keys:?}");
    }
}

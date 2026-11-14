//! Instâncias de prefab sem overrides (`PrefabEntityId` sem `IsA` nem mais
//! nada): as "árvores-espelho" vazias que o editor deixa e que o
//! `bsn_organize --drop-empty-overrides` remove.
//! `cargo run -p bevy_ext_world --example bsn_prefab_stubs -- <ficheiro.bsn>`
use jackdaw_bsn::{BsnPatch, patch_type_path};
fn main() {
    let text = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
    let ast = jackdaw_bsn::parse_bsn_text(&text).unwrap();
    let (mut stubs, mut real) = (0, Vec::new());
    for node in ast.entities_with_component("jackdaw::prefab::components::PrefabEntityId") {
        if ast.find_patch_by_type_path(node, "jackdaw::prefab::components::IsA").is_some() { continue; }
        let mut extra = Vec::new();
        for &pe in &ast.get_patches(node).unwrap().0 {
            let p = ast.get_patch(pe).unwrap();
            match p { BsnPatch::Children(_) | BsnPatch::Name(_) => continue, _ => {} }
            let path = patch_type_path(p).unwrap_or("?");
            if path.ends_with("PrefabEntityId") || path.ends_with("SceneNodeId") { continue; }
            extra.push(path.to_string());
        }
        if extra.is_empty() { stubs += 1 } else { real.push((ast.get_name(node).unwrap_or("?").to_string(), extra)); }
    }
    println!("stubs vazios: {stubs}; com override: {}", real.len());
    for r in real.iter().take(10) { println!("  {:?}", r); }
}

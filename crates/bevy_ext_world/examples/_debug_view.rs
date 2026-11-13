//! TEMPORÁRIO: imprime a vista DSL de um `.bsn` (debug da ponte terrain).

use bevy_ext_world::bsn::load::load_bsn_world_in;
use bevy_ext_world::xml::XmlNode;

fn print_node(node: &XmlNode, depth: usize) {
    println!(
        "{}{} {:?}",
        "  ".repeat(depth),
        node.tag,
        node.attrs
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
    );
    for child in &node.children {
        print_node(child, depth + 1);
    }
}

fn main() -> anyhow::Result<()> {
    let path = std::path::PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("uso: _debug_view <ficheiro.bsn>"),
    );
    let root = path.parent().unwrap().to_path_buf();
    let world = load_bsn_world_in(&path, &root)?;
    for node in &world.nodes {
        print_node(node, 0);
    }
    Ok(())
}

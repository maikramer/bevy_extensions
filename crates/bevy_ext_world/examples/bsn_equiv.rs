//! Compara dois mundos pelo que o jogo carrega (includes/prefabs expandidos,
//! pastas transparentes achatadas), ignorando a ORDEM dos irmãos — uma
//! reorganização do Outliner não pode mudar o mundo.
//!
//! `cargo run -p bevy_ext_world --example bsn_equiv -- antes.bsn depois.bsn`

use std::path::PathBuf;

use bevy_ext_world::bsn::worlds_equivalent;
use bevy_ext_world::xml::XmlNode;
use bevy_ext_world::xml::include::load_world;

fn canonical(node: &XmlNode) -> String {
    let mut attrs = node.attrs.clone();
    attrs.sort();
    let mut children: Vec<String> = node.children.iter().map(canonical).collect();
    children.sort();
    format!(
        "{}{:?}{:?}[{}]",
        node.tag,
        attrs,
        node.text,
        children.join(",")
    )
}

/// Chave de emparelhamento: `(tag, name, canónico)`. O `name` é estável entre
/// duas versões do mesmo mundo mesmo quando o conjunto de attrs muda (a forma
/// canónica de um componente acrescenta attrs default); ordenar só pelo
/// canónico completo emparelhava irmãos trocados.
fn sort_key(node: &XmlNode) -> (String, String, String) {
    let name = node
        .attrs
        .iter()
        .find(|(key, _)| key == "name")
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    (node.tag.clone(), name, canonical(node))
}

fn sorted(nodes: &[XmlNode]) -> Vec<XmlNode> {
    let mut nodes: Vec<XmlNode> = nodes
        .iter()
        .map(|node| XmlNode {
            children: sorted(&node.children),
            ..node.clone()
        })
        .collect();
    nodes.sort_by_cached_key(sort_key);
    nodes
}

fn main() -> anyhow::Result<()> {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [left, right] = args.as_slice() else {
        anyhow::bail!("uso: bsn_equiv <antes.bsn> <depois.bsn>");
    };
    let a = load_world(left)?;
    let b = load_world(right)?;
    let count = |nodes: &[XmlNode]| -> usize { nodes.iter().map(|n| 1 + count_children(n)).sum() };
    println!(
        "antes: {} nós ({} definições) · depois: {} nós ({} definições)",
        count(&a.nodes),
        a.native.len(),
        count(&b.nodes),
        b.native.len()
    );
    worlds_equivalent(
        &a.root_attrs,
        &sorted(&a.nodes),
        &b.root_attrs,
        &sorted(&b.nodes),
    )
    .map_err(|err| anyhow::anyhow!("mundos diferentes: {err}"))?;
    println!("equivalentes");
    Ok(())
}

fn count_children(node: &XmlNode) -> usize {
    node.children.iter().map(|n| 1 + count_children(n)).sum()
}

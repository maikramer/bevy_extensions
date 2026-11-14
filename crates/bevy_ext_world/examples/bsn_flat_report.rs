//! Grupos com muitos filhos directos (candidatos a células da grelha).
fn main() -> anyhow::Result<()> {
    let mut rows = Vec::new();
    for file in std::env::args().skip(1) {
        let text = std::fs::read_to_string(&file)?;
        let ast = jackdaw_bsn::parse_bsn_text(&text).map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut stack = ast.roots.clone();
        while let Some(node) = stack.pop() {
            let kids = ast.get_children_ast(node);
            if kids.len() >= 40 {
                rows.push((kids.len(), format!("{} :: {}", file.rsplit("Vael/").next().unwrap_or(&file), ast.get_name(node).unwrap_or("?"))));
            }
            stack.extend(kids);
        }
    }
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    for (n, label) in rows.iter().take(25) { println!("{n:>5}  {label}"); }
    println!("grupos ≥40 filhos: {}", rows.len());
    Ok(())
}

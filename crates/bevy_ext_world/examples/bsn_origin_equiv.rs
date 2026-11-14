//! Gate de identidade: compara as origens (ficheiro#caminho, a chave dos
//! saves de colheita/destrutíveis) de dois mundos nó a nó.
//! `cargo run -p bevy_ext_world --example bsn_origin_equiv -- antes.bsn depois.bsn`
fn walk(n: &bevy_ext_world::xml::XmlNode, out: &mut Vec<String>) {
    let o = n.origin.as_ref().map(|o| format!("{}#{:?}", o.file.display(), o.path)).unwrap_or_default();
    let name = n.attrs.iter().find(|(k, _)| k == "name").map(|(_, v)| v.clone()).unwrap_or_default();
    out.push(format!("{}|{}|{}", n.tag, name, o));
    for c in &n.children { walk(c, out); }
}
/// Caminho de origem a partir de `/world` (o resto é onde o projecto está).
fn norm(s: &str) -> String {
    s.split('|')
        .map(|part| match part.find("/world") {
            Some(at) if part.starts_with('/') => part[at..].to_string(),
            _ => part.to_string(),
        })
        .collect::<Vec<_>>()
        .join("|")
}

fn main() {
    let mut a = std::env::args().skip(1);
    let (x, y) = (a.next().unwrap(), a.next().unwrap());
    let load = |p: &str| { let w = bevy_ext_world::bsn::load::load_bsn_world(std::path::Path::new(p)).unwrap(); let mut v = Vec::new(); for n in &w.nodes { walk(n, &mut v); } v };
    let (mut l, mut r) = (load(&x), load(&y));
    // Como conjunto: reorganizar o Outliner reordena irmãos.
    let strip0 = |s: &String| norm(s);
    l.sort_by_key(strip0);
    r.sort_by_key(strip0);
    let strip = |s: &String| norm(s);
    let diffs: Vec<_> = l.iter().zip(r.iter()).filter(|(a, b)| strip(a) != strip(b)).take(5).collect();
    println!("nós {} vs {}; origens diferentes: {}", l.len(), r.len(), l.iter().zip(r.iter()).filter(|(a, b)| strip(a) != strip(b)).count());
    for d in diffs { println!("  {} \n  {}", d.0, d.1); }
}

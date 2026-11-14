//! Move os atributos do DSL para componentes tipados.
//!
//! Por omissão são os atributos de props (`collider`, `rigidbody`,
//! `destructible`, `resource-kind/yield` → `bevy_ext_scene`). Com `--terrain`
//! (requer `--features terrain`) é o terreno: a âncora `VoxelTerrain` no
//! `<Terrain>` e os `<Cave>` (com `<Chamber>`/`<Shaft>`) → `VoxelCave`.
//!
//! ```text
//! cargo run -p bevy_ext_world --example bsn_migrate -- [--dry-run] a.bsn b.bsn …
//! cargo run -p bevy_ext_world --features terrain --example bsn_migrate -- --terrain [--dry-run] a.bsn …
//! ```
//!
//! Só migra o que volta exatamente igual; idempotente. Verificar depois com
//! o `bsn_equiv` contra uma cópia de antes.

use std::collections::BTreeMap;

fn main() -> anyhow::Result<()> {
    let mut dry_run = false;
    // `--props`: `AuthoredProps` → `CustomProperties` (componente do Jackdaw).
    let mut props = false;
    // `--node-ids`: `SceneNodeId` em cada nó (edições ao vivo do Play).
    let mut node_ids = false;
    #[cfg(feature = "terrain")]
    let mut terrain = false;
    #[cfg(not(feature = "terrain"))]
    let mut terrain_requested = false;
    let mut files = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            "--props" => props = true,
            "--node-ids" => node_ids = true,
            "--terrain" => {
                #[cfg(feature = "terrain")]
                {
                    terrain = true;
                }
                #[cfg(not(feature = "terrain"))]
                {
                    terrain_requested = true;
                }
            }
            _ => files.push(arg),
        }
    }
    #[cfg(feature = "terrain")]
    if terrain {
        return migrate_terrain(files, dry_run);
    }
    #[cfg(not(feature = "terrain"))]
    if terrain_requested {
        anyhow::bail!("--terrain requer --features terrain");
    }
    if node_ids {
        let mut total = 0;
        for file in &files {
            let text = std::fs::read_to_string(file)?;
            let (minted, n) = bevy_ext_world::bsn::specs::mint_node_ids_text(&text, || {
                jackdaw_scene_types::SceneNodeId::next().0
            });
            if n == 0 {
                continue;
            }
            // Recusa escrever um ficheiro que o Jackdaw não volte a ler.
            jackdaw_bsn::parse_bsn_text(&minted)
                .map_err(|err| anyhow::anyhow!("{file}: ficaria inválido: {err}"))?;
            println!("{file}: {n} ids");
            total += n;
            if !dry_run {
                let tmp = format!("{file}.migrate.tmp");
                std::fs::write(&tmp, minted)?;
                std::fs::rename(&tmp, file)?;
            }
        }
        println!("total: {total}{}", if dry_run { " (dry-run)" } else { "" });
        return Ok(());
    }
    if props {
        let mut total = 0;
        for file in &files {
            let text = std::fs::read_to_string(file)?;
            let (migrated, n) = bevy_ext_world::bsn::specs::migrate_props_text(&text);
            if n == 0 {
                continue;
            }
            println!("{file}: {n} AuthoredProps → CustomProperties");
            total += n;
            if !dry_run {
                let tmp = format!("{file}.migrate.tmp");
                std::fs::write(&tmp, migrated)?;
                std::fs::rename(&tmp, file)?;
            }
        }
        println!("total: {total}{}", if dry_run { " (dry-run)" } else { "" });
        return Ok(());
    }
    let mut total_nodes = 0;
    let mut total: BTreeMap<String, usize> = BTreeMap::new();
    for file in &files {
        let text = std::fs::read_to_string(file)?;
        let (migrated, report) = bevy_ext_world::bsn::specs::migrate_text(&text);
        if report.nodes == 0 {
            continue;
        }
        println!("{file}: {} nós {:?}", report.nodes, report.components);
        total_nodes += report.nodes;
        for (name, count) in report.components {
            *total.entry(name).or_default() += count;
        }
        if !dry_run {
            let tmp = format!("{file}.migrate.tmp");
            std::fs::write(&tmp, migrated)?;
            std::fs::rename(&tmp, file)?;
        }
    }
    println!(
        "total: {total_nodes} nós {total:?}{}",
        if dry_run { " (dry-run)" } else { "" }
    );
    Ok(())
}

#[cfg(feature = "terrain")]
fn migrate_terrain(files: Vec<String>, dry_run: bool) -> anyhow::Result<()> {
    let (mut anchors, mut caves, mut arches, mut cliffs, mut skipped) = (0, 0, 0, 0, 0);
    for file in &files {
        let text = std::fs::read_to_string(file)?;
        let (migrated, report) = bevy_ext_world::bsn::terrain::migrate_terrain_text(&text)?;
        if report.anchors == 0 && report.caves == 0 && report.arches == 0 && report.cliffs == 0 {
            if !report.skipped.is_empty() {
                println!(
                    "{file}: nada a migrar; ficaram em DSL: {:?}",
                    report.skipped
                );
            }
            continue;
        }
        println!(
            "{file}: {} âncora(s), {} cave(s), {} arco(s), {} cliff(s){}",
            report.anchors,
            report.caves,
            report.arches,
            report.cliffs,
            if report.skipped.is_empty() {
                String::new()
            } else {
                format!("; ficaram em DSL: {:?}", report.skipped)
            }
        );
        anchors += report.anchors;
        caves += report.caves;
        arches += report.arches;
        cliffs += report.cliffs;
        skipped += report.skipped.len();
        if !dry_run {
            let tmp = format!("{file}.migrate.tmp");
            std::fs::write(&tmp, migrated)?;
            std::fs::rename(&tmp, file)?;
        }
    }
    println!(
        "total: {anchors} âncora(s), {caves} cave(s), {arches} arco(s), {cliffs} cliff(s), \
         {skipped} ficaram em DSL{}",
        if dry_run { " (dry-run)" } else { "" }
    );
    Ok(())
}

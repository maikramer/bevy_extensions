//! Save envelope, slots and atomic JSON I/O for Bevy games.
//!
//! This crate owns the on-disk format: the opaque `SaveGame` envelope
//! (position, volumes, scene, locale, terrain patches, `world_kv` and the
//! game section), the slot paths and the atomic write/load. It has no plugin
//! and no runtime state: a game decides *when* to capture and *what* to put in
//! `world_kv`/`game`; the Viber runtime keeps that adapter in `save.rs`.
//!
//! The types preserve the historical `viber::save::…` reflection paths with
//! `#[type_path]`, so the BRP/`viber.ecs` contract does not change after the
//! extraction.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy_ext_terrain_runtime::runtime::TerrainRuntime;
use bevy_ext_world::DeltaPatch;
use serde::{Deserialize, Serialize};

/// Nome do save deste mundo ([`world_save_key`]), inserido pelo `run` a
/// partir do caminho do world.xml; `None` = o nome global histórico.
#[derive(Debug, Clone, Resource, Default, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::save"]
pub struct WorldSaveKey(pub Option<String>);

/// Chave do save de um ficheiro de mundo: a pasta do jogo para `world.xml`
/// (`simple-rpg/world.xml` → `simple-rpg`, o nome dos saves existentes) e
/// `pasta-ficheiro` para os restantes — mundos irmãos na mesma pasta
/// (`worlds/qa-agua.xml`, `worlds/qa-cliffs.xml`) deixam de partilhar o save.
/// O sufixo `.static` do stem é removido (`world.static.xml`, o default
/// pós-`viber bake`, partilha a chave do `world.xml` de onde veio — a
/// continuidade dos saves não pode depender do caminho de arranque; só o
/// sufixo exato conta, `qa-static.xml` fica `qa-static`).
/// O caminho é tornado absoluto: `viber run world.xml` a partir da pasta do
/// jogo tem `parent()` vazio e caía no nome global.
pub fn world_save_key(world_file: &Path) -> Option<String> {
    let abs = world_file
        .canonicalize()
        .or_else(|_| std::path::absolute(world_file))
        .ok()?;
    let dir = abs.parent()?.file_name()?.to_string_lossy().into_owned();
    let mut stem = abs.file_stem()?.to_string_lossy().into_owned();
    if let Some(base) = stem.strip_suffix(".static") {
        stem = base.to_owned();
    }
    Some(if stem.eq_ignore_ascii_case("world") {
        dir
    } else {
        format!("{dir}-{stem}")
    })
}

/// Diretório de saves do `config.yaml` do jogo (`save.dir`); `None` = o
/// histórico `~/.local/share/viber` (apps mínimas de teste).
#[derive(Debug, Clone, Resource, Default, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::save"]
pub struct SaveDir(pub Option<PathBuf>);

/// Nome do ficheiro de save fallback (quando o mundo é desconhecido).
pub const SAVE_FILENAME: &str = "simple-rpg.save.json";

/// Caminho do save: `save.dir` do config do jogo (fallback histórico
/// `~/.local/share/viber` via `dirs::home_dir` — semântica `$HOME` exata,
/// NÃO `XDG_DATA_HOME`, os saves históricos vivem aí; último fallback: cwd).
pub fn save_path() -> PathBuf {
    save_path_for(None, None)
}

/// Caminho do save do mundo (`world_key` = [`world_save_key`]): gravar no
/// mundo A e carregar no B nunca mais restaura o estado errado. Sem chave
/// conhecida, cai no nome global histórico.
pub fn save_path_for(world_key: Option<&str>, save_dir: Option<&Path>) -> PathBuf {
    let name = world_key
        .filter(|key| !key.is_empty())
        .map(|key| format!("{key}.save.json"))
        .unwrap_or_else(|| SAVE_FILENAME.to_string());
    let dir = match save_dir.map(Path::to_path_buf) {
        Some(dir) => {
            let _ = std::fs::create_dir_all(&dir);
            dir
        }
        None => dirs::home_dir()
            .map(|home| {
                let dir = home.join(".local/share/viber");
                let _ = std::fs::create_dir_all(&dir);
                dir
            })
            .unwrap_or_default(),
    };
    if dir.as_os_str().is_empty() {
        PathBuf::from(name)
    } else {
        dir.join(name)
    }
}

/// Caminho de um slot. `None` = save legado; `0` = autosave; `1..` = slots
/// manuais. O sufixo entra antes de `.save.json`, por isso o save antigo do
/// mundo continua no sítio e é lido pelo `load_save()` sem migração.
pub fn save_path_for_slot(
    world_key: Option<&str>,
    save_dir: Option<&Path>,
    slot: Option<u32>,
) -> PathBuf {
    let base = save_path_for(world_key, save_dir);
    let Some(slot) = slot else {
        return base;
    };
    let suffix = if slot == 0 {
        "auto".to_string()
    } else {
        format!("slot{slot}")
    };
    let file = base
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(SAVE_FILENAME);
    let name = file
        .strip_suffix(".save.json")
        .map(|stem| format!("{stem}.{suffix}.save.json"))
        .unwrap_or_else(|| format!("{file}.{suffix}"));
    base.with_file_name(name)
}

/// Sumário de um slot para o menu (o jogo não abre o JSON à mão).
#[derive(Debug, Clone, Serialize, Reflect)]
#[type_path = "viber::save"]
pub struct SaveSlotInfo {
    pub slot: u32,
    pub path: String,
    pub exists: bool,
    pub bytes: u64,
    /// Unix seconds: campo do save, ou mtime do ficheiro nos saves antigos.
    pub saved_at: Option<f64>,
    pub level: u32,
    pub gold: u32,
    pub scene: Option<String>,
    pub playtime: Option<f64>,
    pub title: Option<String>,
}

/// Lê os metadados dos slots 0..=3 (0 = autosave) e, se existir, do save
/// legado (slot 4 — `viber.load_save()`, não `load_slot(4)`). Nunca falha:
/// um JSON corrompido lista o slot como existente mas sem nível/ouro.
pub fn list_save_slots(world_key: Option<&str>, save_dir: Option<&Path>) -> Vec<SaveSlotInfo> {
    fn info(slot: u32, path: PathBuf) -> SaveSlotInfo {
        let meta = std::fs::metadata(&path).ok();
        let exists = meta.is_some();
        let bytes = meta.as_ref().map_or(0, |m| m.len());
        let mtime = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64());
        let game = exists
            .then(|| load_from_disk(&path).ok())
            .flatten()
            .unwrap_or_default();
        SaveSlotInfo {
            slot,
            path: path.display().to_string(),
            exists,
            bytes,
            saved_at: game.saved_at.or(mtime),
            level: game.level(),
            gold: game.gold(),
            scene: game.scene.clone(),
            playtime: game.playtime,
            title: game.title.clone(),
        }
    }

    let mut rows: Vec<SaveSlotInfo> = (0..=3)
        .map(|slot| info(slot, save_path_for_slot(world_key, save_dir, Some(slot))))
        .collect();
    let legacy = info(4, save_path_for(world_key, save_dir));
    if legacy.exists {
        rows.push(legacy);
    }
    rows
}

// ── estrutura do save ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, Reflect)]
#[type_path = "viber::save"]
pub struct SaveGame {
    /// Versão do schema do save. Saves antigos (sem o campo) carregam como 0;
    /// um save de versão MAIS NOVA que a engine [`SAVE_VERSION`] carrega na
    /// mesma (campos a mais são preservados na secção opaca `game`) mas o load
    /// avisa — os campos novos perdem-se ao regravar por cima.
    #[serde(default)]
    pub version: u32,
    /// Unix seconds da gravação. Saves antigos não têm; o menu cai no mtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_at: Option<f64>,
    /// Tempo de jogo acumulado (segundos), escrito pelo jogo em `world_kv`
    /// ou directamente aqui.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playtime: Option<f64>,
    /// Rótulo do slot (ex. "Discórdia — Capítulo I").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    // ── envelope genérico ───────────────────────────────────────────────
    // Nada aqui é de um jogo em particular: pose, volumes, cena e o estado
    // world-scoped do Luau. XP, ouro, quests, inventário e talentos vivem na
    // secção opaca `game` (ou em `world_kv`, que é o contrato do `viber.game()`).
    //
    // Os campos nucleares têm `#[serde(default)]`: um save antigo/parcial sem
    // `volumes`/`world_kv`/… carrega em vez de ser recusado inteiro. Posição e
    // volumes são `Option`: o zero do tipo punha o herói na origem e
    // silenciava o áudio. Ausentes, o load mantém o estado da sessão; o JSON
    // de `Some` é o mesmo array de sempre.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f32; 3]>,
    /// Volumes (master, music, sfx) 0..=1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volumes: Option<(f32, f32, f32)>,
    /// Volumes dos buses novos (voice, ambience) 0..=1. Campo separado
    /// porque o save antigo é uma tupla de 3 e não pode mudar de forma.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volumes_extra: Option<(f32, f32)>,
    /// Estado de JOGO world-scoped do Luau (`viber.game()`), JSON plano
    /// (string/número/bool). É o gancho de persistência para lógica de jogo
    /// que vive em Lua — sem ele, scripts não conseguiam guardar nada.
    #[serde(default)]
    #[reflect(ignore)]
    pub world_kv: std::collections::BTreeMap<String, serde_json::Value>,
    /// Secção OPACA do jogo: um blob JSON que a engine não interpreta
    /// (progressão, inventário, quests — o que o jogo quiser). `None` num
    /// save novo; os saves antigos migram para aqui os campos de topo que
    /// esta engine já não conhece (ver [`SaveGame::migrate_legacy`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[reflect(ignore)]
    pub game: Option<serde_json::Value>,
    /// Cena onde o herói estava (projetos com cenas); `position` é relativa
    /// a ELA. Ausente (saves v1) = a cena principal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<String>,
    /// `(cena, portal)` por onde se entrou na cena gravada — o `@back` da porta de saída.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub came_from: Option<(String, String)>,
    /// Idioma do save (`pt`, `en`); ausente = o do arranque.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    /// Recortes de terreno editados em jogo.
    #[serde(default)]
    pub terrain_patches: Vec<DeltaPatch>,
    /// Rebinds do mapa de ações (`ação → tecla`).
    #[serde(default)]
    pub action_keys: std::collections::HashMap<String, String>,
    /// Campos de saves ANTIGOS apanhados pelo `flatten` (XP, ouro, items,
    /// quests, …). Só entra por desserialização e nunca é escrito: o load
    /// chama [`SaveGame::migrate_legacy`] e move tudo para `game`/`world_kv`.
    #[serde(flatten, default, skip_serializing)]
    #[reflect(ignore)]
    pub legacy: std::collections::BTreeMap<String, serde_json::Value>,
}

/// Escalares antigos que ficam directamente legíveis em `world_kv` (o jogo
/// lê-os com `viber.game()`); objectos/arrays viajam só na secção `game`.
///
/// O contrato do `viber.game()` é plano: espelhar todos os primitivos antigos
/// preserva `hp`, `max_hp`, `xp`, `level`, ouro e afins sem obrigar o jogo a
/// conhecer a secção opaca. Os tipos compostos (quests, inventário) ficam em
/// `game.legacy` para o Rust do jogo os migrar.
impl SaveGame {
    /// Move os campos de topo de um save ANTIGO para o envelope novo.
    ///
    /// Os saves anteriores gravavam o estado do RPG no topo do JSON (`gold`,
    /// `items`, `quest_states`, `health`, …). Nada é descartado: tudo viaja
    /// para `game.legacy` (o jogo lê o que quiser de lá) e todos os escalares
    /// são espelhados em `world_kv`, se o jogo ainda não tiver a chave. A
    /// gravação seguinte escreve só o schema novo.
    pub fn migrate_legacy(&mut self) {
        if self.legacy.is_empty() {
            return;
        }
        let legacy = std::mem::take(&mut self.legacy);
        for (key, value) in &legacy {
            if matches!(
                value,
                serde_json::Value::Bool(_)
                    | serde_json::Value::Number(_)
                    | serde_json::Value::String(_)
            ) {
                self.world_kv
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
        }
        let game = self
            .game
            .get_or_insert_with(|| serde_json::Value::Object(Default::default()));
        if !game.is_object() {
            *game = serde_json::Value::Object(Default::default());
        }
        let map = game.as_object_mut().expect("object ensured above");
        let slot = map
            .entry("legacy")
            .or_insert_with(|| serde_json::Value::Object(Default::default()));
        if !slot.is_object() {
            *slot = serde_json::Value::Object(Default::default());
        }
        let slot = slot.as_object_mut().expect("object ensured above");
        for (key, value) in legacy {
            slot.insert(key, value);
        }
    }

    /// Nível mostrado no menu de slots: `save_level` do `world_kv` (o jogo
    /// publica-o em `viber.game()`) ou o `level` de um save antigo migrado.
    pub fn level(&self) -> u32 {
        self.kv_u32("save_level")
            .or_else(|| self.legacy_u32("level"))
            .unwrap_or(0)
    }

    /// Ouro mostrado no menu de slots — mesma precedência do [`Self::level`].
    pub fn gold(&self) -> u32 {
        self.kv_u32("save_gold")
            .or_else(|| self.legacy_u32("gold"))
            .unwrap_or(0)
    }

    fn kv_u32(&self, key: &str) -> Option<u32> {
        self.world_kv
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .map(|value| value.min(u32::MAX as u64) as u32)
    }

    fn legacy_u32(&self, key: &str) -> Option<u32> {
        self.game
            .as_ref()?
            .get("legacy")?
            .get(key)?
            .as_u64()
            .map(|value| value.min(u32::MAX as u64) as u32)
    }
}

/// Versão do schema [`SaveGame`] que ESTA engine escreve (3 = envelope
/// genérico com secção `game`; os campos de RPG saíram do topo).
pub const SAVE_VERSION: u32 = 3;

/// Captura pose, volumes e o envelope do ficheiro. Ouro, quests, talentos
/// e marcos vivem em `world_kv` (`viber.game()`), que o caller preenche.
pub fn capture(position: [f32; 3], volumes: (f32, f32, f32)) -> SaveGame {
    SaveGame {
        version: SAVE_VERSION,
        position: Some(position),
        volumes: Some(volumes),
        ..Default::default()
    }
}

/// Escreve o save em disco (JSON pretty) ATOMICAMENTE: escreve num `.tmp` NO
/// MESMO diretório, sincroniza e só então renomea — o rename é atómico no
/// mesmo filesystem, portanto um ENOSPC/kill -9/OOM da GPU a meio nunca
/// destrói o save anterior (é o único save, sem backup).
pub fn save_to_disk(path: &Path, game: &SaveGame) -> Result<(), String> {
    use std::io::Write;
    let json = serde_json::to_string_pretty(game).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    let write_tmp = || -> Result<(), String> {
        let mut f = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        f.write_all(json.as_bytes())
            .and_then(|()| f.sync_all())
            .map_err(|e| format!("{}: {e}", tmp.display()))
    };
    if let Err(e) = write_tmp() {
        let _ = std::fs::remove_file(&tmp); // não deixar .tmp órfão
        return Err(e);
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", path.display())
    })
}

/// Lê o save do disco. Saves antigos migram os campos de RPG para a secção
/// opaca `game` (ver [`SaveGame::migrate_legacy`]).
pub fn load_from_disk(path: &Path) -> Result<SaveGame, String> {
    let json = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut game: SaveGame = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    game.migrate_legacy();
    Ok(game)
}

// ── ponte de save/load em runtime ────────────────────────────────────────
//
// O pedido e os recortes pendentes viveram no monólito (`save.rs`); cá fica
// a definição — o monólito re-exporta para `viber::save::…` continuar a
// resolver e o host Luau (`viber.save()`/`viber.load()`) continua a escrever
// O MESMO recurso.

/// Pedido de save/load vindo de SCRIPT (`viber.save()` / `viber.load()`) —
/// o host de scripts não pode chamar o sistema de save de dentro do Luau, e
/// o host sem ficheiro `world_kv` não teria como persistir num mundo sem o
/// plugin de opções. Consumido (e limpo) pelo sistema de opções do runtime.
#[derive(Debug, Default, Resource, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::save"]
pub struct SaveRequest {
    pub save: bool,
    pub load: bool,
    /// `None` = save legado; `0` = autosave; `1..` = slot manual.
    pub slot: Option<u32>,
}

/// Recortes de terreno de um save à espera do `TerrainRuntime` certo.
///
/// Importar logo no load punha-os no runtime da cena VELHA: num load que
/// troca de cena, a troca substitui o `TerrainRuntime` e os recortes
/// perdiam-se — ou caíam no terreno PEQUENO da cena errada (o menu tem o seu
/// próprio campo). Ficam pendentes; [`apply_pending_save_patches`] importa-os
/// no primeiro runtime vivo fora de uma troca (na mesma cena: no fim do
/// próprio frame do load).
#[derive(Debug, Default, Resource, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::save"]
pub struct PendingSavePatches(pub Vec<DeltaPatch>);

/// Um loader de cena que pode estar a meio de uma troca (o `busy()` do
/// `SceneLoader`). Contrato SEM tipo: o save não conhece o loader — quem
/// regista [`apply_pending_save_patches`] concreta o `S` e implementa isto
/// sobre o tipo dele.
pub trait SceneSwitchBusy {
    fn busy(&self) -> bool;
}

/// Consome [`PendingSavePatches`] assim que haja terreno vivo e nenhuma troca
/// em curso (`import_patches` sobe a revisão — o mesher volta a cozer as
/// colunas tocadas, valha importar cedo ou tarde).
///
/// Genérico sobre o loader de cena ([`SceneSwitchBusy`]): regista com o tipo
/// concreto do runtime (`apply_pending_save_patches::<SceneLoader>`).
pub fn apply_pending_save_patches<S: Resource + SceneSwitchBusy>(
    mut pending: ResMut<PendingSavePatches>,
    mut runtime: Option<ResMut<TerrainRuntime>>,
    loader: Option<Res<S>>,
) {
    if pending.0.is_empty() {
        return;
    }
    let Some(runtime) = runtime.as_deref_mut() else {
        return; // sala sem terreno: espera pela volta ao campo principal
    };
    if loader.as_deref().is_some_and(SceneSwitchBusy::busy) {
        return;
    }
    std::sync::Arc::make_mut(&mut runtime.deltas).import_patches(&pending.0);
    pending.0.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_key_uses_folder_for_world_and_stem_for_siblings() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("world.xml");
        std::fs::write(&world, "<world/>").unwrap();
        assert_eq!(
            world_save_key(&world).as_deref(),
            Some(dir.path().file_name().unwrap().to_str().unwrap())
        );
    }

    #[test]
    fn roundtrip_writes_and_reads_the_envelope() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("slot.save.json");
        let mut game = capture([1.0, 2.0, 3.0], (1.0, 0.5, 0.25));
        game.scene = Some("main".into());
        game.world_kv
            .insert("save_level".into(), serde_json::json!(7));
        save_to_disk(&path, &game).unwrap();
        let loaded = load_from_disk(&path).unwrap();
        assert_eq!(loaded.position, Some([1.0, 2.0, 3.0]));
        assert_eq!(loaded.volumes, Some((1.0, 0.5, 0.25)));
        assert_eq!(loaded.scene.as_deref(), Some("main"));
        assert_eq!(loaded.level(), 7);
    }

    #[test]
    fn slot_paths_are_stable_and_keep_the_legacy_name() {
        let dir = tempfile::tempdir().unwrap();
        let key = Some("vael");
        let save_dir = Some(dir.path());
        let base = save_path_for(key, save_dir);
        assert_eq!(base, dir.path().join("vael.save.json"));
        assert_eq!(save_path_for_slot(key, save_dir, None), base);
        assert_eq!(
            save_path_for_slot(key, save_dir, Some(0)),
            dir.path().join("vael.auto.save.json")
        );
        assert_eq!(
            save_path_for_slot(key, save_dir, Some(1)),
            dir.path().join("vael.slot1.save.json")
        );
        assert_eq!(
            save_path_for_slot(key, save_dir, Some(3)),
            dir.path().join("vael.slot3.save.json")
        );
    }

    #[test]
    fn world_key_treats_world_static_as_the_same_world_and_keeps_siblings_apart() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game");
        std::fs::create_dir_all(&game).unwrap();
        let world = game.join("world.xml");
        let static_world = game.join("world.static.xml");
        let qa_static = game.join("qa-static.xml");
        for path in [&world, &static_world, &qa_static] {
            std::fs::write(path, "<world/>").unwrap();
        }
        let key = "game";
        assert_eq!(world_save_key(&world).as_deref(), Some(key));
        assert_eq!(world_save_key(&static_world).as_deref(), Some(key));
        assert_eq!(
            world_save_key(&qa_static).as_deref(),
            Some("game-qa-static")
        );
    }

    #[test]
    fn legacy_scalars_move_to_world_kv_and_objects_to_game() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.save.json");
        std::fs::write(
            &path,
            r#"{
                "gold": 12,
                "level": 4,
                "items": ["potion"],
                "quest_states": {"a": "done"}
            }"#,
        )
        .unwrap();
        let loaded = load_from_disk(&path).unwrap();
        // Escalares antigos ficam legíveis no contrato plano do `viber.game()`.
        assert_eq!(loaded.world_kv.get("gold"), Some(&serde_json::json!(12)));
        assert_eq!(loaded.level(), 4);
        assert_eq!(loaded.gold(), 12);
        // Compostos ficam preservados na secção opaca, não se perdem.
        let legacy = &loaded.game.as_ref().unwrap()["legacy"];
        assert_eq!(legacy["items"], serde_json::json!(["potion"]));
        assert_eq!(legacy["quest_states"], serde_json::json!({"a": "done"}));
    }

    #[test]
    fn corrupt_save_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let path = save_path_for_slot(Some("broken"), Some(dir.path()), Some(0));
        std::fs::write(&path, "{ isto não é json").unwrap();
        assert!(load_from_disk(&path).is_err());
        // O menu não rebenta: lista o slot como existente, sem metadados.
        let rows = list_save_slots(Some("broken"), Some(dir.path()));
        assert!(rows[0].exists);
        assert_eq!(rows[0].level, 0);
        assert_eq!(rows[0].gold, 0);
    }

    #[test]
    fn save_is_atomic_and_leaves_no_tmp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("atom.save.json");
        let first = capture([1.0, 0.0, 0.0], (1.0, 1.0, 1.0));
        save_to_disk(&path, &first).unwrap();
        let tmp = path.with_extension("json.tmp");
        assert!(path.is_file());
        assert!(!tmp.exists(), "o .tmp não pode sobreviver ao rename");
        // A segunda gravação substitui o envelope inteiro.
        let mut second = capture([9.0, 8.0, 7.0], (0.5, 0.5, 0.5));
        second.scene = Some("interior".into());
        save_to_disk(&path, &second).unwrap();
        let loaded = load_from_disk(&path).unwrap();
        assert_eq!(loaded.position, Some([9.0, 8.0, 7.0]));
        assert_eq!(loaded.scene.as_deref(), Some("interior"));
        assert!(!tmp.exists());
    }

    #[test]
    fn list_slots_reads_meta_and_includes_the_legacy_save_only_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let key = Some("vael");
        let save_dir = Some(dir.path());

        let mut auto = capture([1.0, 2.0, 3.0], (1.0, 1.0, 1.0));
        auto.world_kv
            .insert("save_level".into(), serde_json::json!(3));
        auto.world_kv
            .insert("save_gold".into(), serde_json::json!(7));
        save_to_disk(&save_path_for_slot(key, save_dir, Some(0)), &auto).unwrap();

        let mut manual = capture([4.0, 5.0, 6.0], (1.0, 1.0, 1.0));
        manual.scene = Some("main".into());
        save_to_disk(&save_path_for_slot(key, save_dir, Some(1)), &manual).unwrap();

        let rows = list_save_slots(key, save_dir);
        assert_eq!(rows.len(), 4, "0..=3, legado ausente");
        assert!(rows[0].exists);
        assert_eq!(rows[0].level, 3);
        assert_eq!(rows[0].gold, 7);
        assert!(rows[1].exists);
        assert_eq!(rows[1].scene.as_deref(), Some("main"));
        assert!(!rows[2].exists);
        assert!(!rows[3].exists);

        // O save antigo do mundo (sem sufixo) só aparece como slot 4.
        save_to_disk(&save_path_for(key, save_dir), &manual).unwrap();
        let rows = list_save_slots(key, save_dir);
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[4].slot, 4);
        assert!(rows[4].exists);
        assert_eq!(rows[4].scene.as_deref(), Some("main"));
    }

    /// Os recortes de terreno de um load ESPERAM por terreno vivo: importá-los
    /// no runtime da cena velha perdia-os na troca (ou caía no campo pequeno
    /// da cena errada). Sem `TerrainRuntime` fica pendente; com um, importa e
    /// limpa.
    #[test]
    fn pending_save_patches_wait_for_a_live_terrain() {
        use bevy::ecs::system::RunSystemOnce as _;

        #[derive(Resource, Default)]
        struct IdleLoader;
        impl SceneSwitchBusy for IdleLoader {
            fn busy(&self) -> bool {
                false
            }
        }

        fn small_runtime() -> TerrainRuntime {
            let spec = bevy_ext_terrain_runtime::spec::TerrainSpec {
                world_size: 128.0,
                max_height: 40.0,
                seed: 5,
                ..bevy_ext_terrain_runtime::spec::TerrainSpec::default()
            };
            let map = bevy_ext_terrain_runtime::HeightMapU16::procedural(
                &spec,
                spec.resolution.max(1) as usize,
            );
            let grid = bevy_ext_terrain_runtime::BrushGrid::from_height_map(
                &map,
                spec.world_size,
                spec.max_height,
                spec.height_smoothing,
            )
            .expect("grid builds");
            TerrainRuntime {
                spec,
                grid: std::sync::Arc::new(grid),
                water: vec![],
                roads: vec![],
                // A camada de pavimento entrou depois destes fixtures; vazia é
                // o default e não muda o que os testes de save comparam.
                paving: Default::default(),
                pads: vec![],
                voxel: std::sync::Arc::new(bevy_ext_terrain_runtime::voxel::VoxelField::default()),
                deltas: std::sync::Arc::new(bevy_ext_terrain_runtime::DeltaGrid::default()),
                paint: std::sync::Arc::new(bevy_ext_terrain_runtime::PaintGrid::default()),
                splat_tuning: Default::default(),
            }
        }

        let mut app = App::new();
        app.init_resource::<PendingSavePatches>();
        app.world_mut()
            .resource_mut::<PendingSavePatches>()
            .0
            .push(DeltaPatch {
                min: [0.0, 0.0],
                step: [1.0, 1.0],
                cols: 2,
                rows: 2,
                data: vec![1.0, 2.0, 3.0, 4.0],
            });
        // Sem terreno vivo (interior/menu à espera): fica pendente.
        app.world_mut()
            .run_system_once(apply_pending_save_patches::<IdleLoader>)
            .unwrap();
        assert_eq!(
            app.world().resource::<PendingSavePatches>().0.len(),
            1,
            "sem terreno os recortes esperam"
        );
        // Terreno vivo: importa e limpa o pendente.
        app.insert_resource(small_runtime());
        app.world_mut()
            .run_system_once(apply_pending_save_patches::<IdleLoader>)
            .unwrap();
        assert!(app.world().resource::<PendingSavePatches>().0.is_empty());
        let runtime = app.world().resource::<TerrainRuntime>();
        assert_eq!(
            runtime.deltas.export_patches().len(),
            1,
            "o recorte entrou no runtime vivo"
        );
    }

    /// Loader OCUPADO (troca de cena a meio): o import espera — os recortes
    /// iam cair no runtime que a troca substitui.
    #[test]
    fn pending_save_patches_wait_for_a_busy_scene_loader() {
        use bevy::ecs::system::RunSystemOnce as _;

        #[derive(Resource, Default)]
        struct BusyLoader;
        impl SceneSwitchBusy for BusyLoader {
            fn busy(&self) -> bool {
                true
            }
        }

        let mut app = App::new();
        app.init_resource::<PendingSavePatches>();
        app.init_resource::<BusyLoader>();
        app.insert_resource(small_runtime_of());
        app.world_mut()
            .resource_mut::<PendingSavePatches>()
            .0
            .push(DeltaPatch {
                min: [0.0, 0.0],
                step: [1.0, 1.0],
                cols: 1,
                rows: 1,
                data: vec![5.0],
            });
        app.world_mut()
            .run_system_once(apply_pending_save_patches::<BusyLoader>)
            .unwrap();
        assert_eq!(
            app.world().resource::<PendingSavePatches>().0.len(),
            1,
            "com a troca em curso os recortes esperam"
        );
    }

    // Igual a `small_runtime` mas fora do bloco do outro teste.
    fn small_runtime_of() -> TerrainRuntime {
        let spec = bevy_ext_terrain_runtime::spec::TerrainSpec {
            world_size: 64.0,
            max_height: 20.0,
            seed: 7,
            ..bevy_ext_terrain_runtime::spec::TerrainSpec::default()
        };
        let map = bevy_ext_terrain_runtime::HeightMapU16::procedural(
            &spec,
            spec.resolution.max(1) as usize,
        );
        let grid = bevy_ext_terrain_runtime::BrushGrid::from_height_map(
            &map,
            spec.world_size,
            spec.max_height,
            spec.height_smoothing,
        )
        .expect("grid builds");
        TerrainRuntime {
            spec,
            grid: std::sync::Arc::new(grid),
            water: vec![],
            roads: vec![],
            paving: Default::default(),
            pads: vec![],
            voxel: std::sync::Arc::new(bevy_ext_terrain_runtime::voxel::VoxelField::default()),
            deltas: std::sync::Arc::new(bevy_ext_terrain_runtime::DeltaGrid::default()),
            paint: std::sync::Arc::new(bevy_ext_terrain_runtime::PaintGrid::default()),
            splat_tuning: Default::default(),
        }
    }
}

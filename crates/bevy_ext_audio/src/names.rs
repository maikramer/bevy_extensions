//! Registry nome→clip para `viber.sound` (case-insensitive, tabela única —
//! um clip novo no enum SEM linha aqui falha o teste
//! `test_lua_registry_covers_all_clips`).
//!
//! Era `src/luau/sfx.rs` do monólito; o domínio é áudio, por isso vive neste
//! crate e o runtime de scripts reexporta ([`sfx_clip_from_str`] alimenta o
//! `resolve_loop_clip` das [`crate::music::ScriptSoundLoops`]).

use bevy_ext_core::SfxClip;

/// Registry nome-clip Lua (`viber.sound`) — case-insensitive, tabela única.
/// Os gatilhos nativos usam as variantes directamente; os scripts passam por
/// aqui. Aliases aceitáveis além do nome canónico (ex. `level_up`) ficam em
/// linhas próprias apontando à mesma variante.
pub const SFX_NAME_REGISTRY: &[(&str, SfxClip)] = &[
    ("hit", SfxClip::Hit),
    ("whoosh", SfxClip::Whoosh),
    ("harvest", SfxClip::Harvest),
    ("ui", SfxClip::Ui),
    ("chop_hit", SfxClip::ChopHit),
    ("chop_break", SfxClip::ChopBreak),
    ("mine_hit", SfxClip::MineHit),
    ("mine_break", SfxClip::MineBreak),
    ("levelup", SfxClip::LevelUp),
    ("level_up", SfxClip::LevelUp),
    ("quest_complete", SfxClip::QuestDone),
    ("quest_done", SfxClip::QuestDone),
    ("travel", SfxClip::Travel),
    ("loot", SfxClip::Loot),
    ("chest_open", SfxClip::Loot),
    ("footstep", SfxClip::Footstep),
    ("footstep_water", SfxClip::FootstepWater),
    ("hurt", SfxClip::Hurt),
    ("heal", SfxClip::Heal),
    ("game_over", SfxClip::GameOver),
    ("quest_accept", SfxClip::QuestAccept),
    ("notification", SfxClip::Notification),
    ("coin", SfxClip::Coin),
    ("buy", SfxClip::Buy),
    ("error", SfxClip::Error),
    ("save", SfxClip::Save),
    ("load", SfxClip::Load),
    ("shop_open", SfxClip::ShopOpen),
    ("enemy_hurt", SfxClip::EnemyHurt),
    ("enemy_death", SfxClip::EnemyDeath),
    ("wolf_growl", SfxClip::WolfGrowl),
    ("growl", SfxClip::WolfGrowl),
    ("slime_squish", SfxClip::SlimeSquish),
    ("boss_roar", SfxClip::BossRoar),
    ("roar", SfxClip::BossRoar),
    ("shield_block", SfxClip::ShieldBlock),
    ("block", SfxClip::ShieldBlock),
    ("door_open", SfxClip::DoorOpen),
    ("door_close", SfxClip::DoorClose),
    ("bomb_drop", SfxClip::BombDrop),
    ("jump", SfxClip::Jump),
    ("dash", SfxClip::Dash),
    ("crash", SfxClip::Crash),
    ("nitro", SfxClip::Nitro),
    ("skid", SfxClip::Skid),
    ("bow_shot", SfxClip::BowShot),
    ("arrow_hit", SfxClip::ArrowHit),
    ("fire_crackle", SfxClip::FireCrackle),
    ("item_drop", SfxClip::ItemDrop),
    ("thunder", SfxClip::Thunder),
];

/// Nome de clip SFX Lua (`"hit"`, `"UI"`) → [`SfxClip`]
/// (case-insensitive via [`SFX_NAME_REGISTRY`]; desconhecido = `None`).
pub fn sfx_clip_from_str(name: &str) -> Option<SfxClip> {
    let lower = name.to_ascii_lowercase();
    SFX_NAME_REGISTRY
        .iter()
        .find(|(alias, _)| *alias == lower)
        .map(|(_, clip)| *clip)
}

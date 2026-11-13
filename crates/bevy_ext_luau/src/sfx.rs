//! Registry nome→clip para os comandos de som (`viber.sound_at_player`,
//! futuros `viber.sound`/`viber.sound_loop`): case-insensitive, tabela
//! única. Portado do `viber::luau::sfx`, apontando para o `SfxClip` do
//! `bevy_ext_core`. Um clip novo no enum SEM linha aqui falha o teste
//! [`tests::registry_cobre_todos_os_clips`].

use bevy_ext_core::SfxClip;

/// Registry nome-clip Lua — case-insensitive, tabela única. Aliases
/// aceitáveis além do nome canónico (ex. `level_up`) ficam em linhas
/// próprias apontando à mesma variante.
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

/// Nome de clip SFX Lua (`"hit"`, `"UI"`) → [`SfxClip`] (case-insensitive;
/// desconhecido = `None`).
pub fn sfx_clip_from_str(name: &str) -> Option<SfxClip> {
    let lower = name.to_ascii_lowercase();
    SFX_NAME_REGISTRY
        .iter()
        .find(|(alias, _)| *alias == lower)
        .map(|(_, clip)| *clip)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_ext_core::SFX_CLIPS_ALL;

    #[test]
    fn registry_cobre_todos_os_clips() {
        for clip in SFX_CLIPS_ALL {
            let covered = SFX_NAME_REGISTRY.iter().any(|(_, c)| c == clip);
            assert!(covered, "clip {clip:?} sem nome no registry Lua");
        }
    }

    #[test]
    fn procura_case_insensitive() {
        assert_eq!(sfx_clip_from_str("hit"), Some(SfxClip::Hit));
        assert_eq!(sfx_clip_from_str("UI"), Some(SfxClip::Ui));
        assert_eq!(sfx_clip_from_str("Growl"), Some(SfxClip::WolfGrowl));
        assert_eq!(sfx_clip_from_str("level_up"), Some(SfxClip::LevelUp));
        assert_eq!(sfx_clip_from_str("nivel"), None);
    }
}

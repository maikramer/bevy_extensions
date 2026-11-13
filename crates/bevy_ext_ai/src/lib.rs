//! Serviço de IA para jogos Bevy — sentidos, locomoção de espaço de
//! velocidades, a FSM simples das criaturas e os vitais.
//!
//! O crate é a camada de SERVIÇO: os produtores declaram um desejo de frame
//! ([`ai::AiLocomotion::drive_to`], eventos no [`senses::SenseBus`]) e o
//! crate fornece o único consumidor ([`ai::apply_ai_locomotion`]) que o
//! transforma em movimento sobre a fonte de chão que o jogo liga
//! ([`Ground`]). O comportamento é de quem monta a App: [`AiPlugin`]
//! (serviço + FSM simples), só [`AiServicePlugin`] (comportamento próprio)
//! ou [`SimpleAiPlugin`] sobre o serviço — a mesma escolha do bootstrap da
//! engine, exposta como [`AiMode`].
//!
//! Os tipos preservam os caminhos de reflexão históricos do monólito
//! (`viber::ai::…`, `viber::senses::…`, `viber::vitals::…`) com
//! `#[type_path]`, para o BRP/`viber.ecs` e os saves não mudarem após a
//! extração.

pub mod ai;
pub mod combat;
pub mod corpse;
pub mod ground;
pub mod locomotion;
pub mod profiler;
pub mod senses;
pub mod vitals;

pub use ai::{
    AiLocomotion, AiLocomotionSystems, AiMode, AiPlugin, AiServicePlugin, DEAGGRO_HYSTERESIS,
    EnemyCreature, EnemyState, FACING_MOVE_SPEED, STOP_EPSILON, SimpleAiPlugin, WANDER_ARRIVE_DIST,
    WANDER_RADIUS, WANDER_RETARGET_SECS, WANDER_SPEED_FRACTION, WanderState, apply_ai_locomotion,
    enemy_ai, enemy_next_state, enemy_seed, wander_target,
};
pub use combat::HitReact;
pub use corpse::{CORPSE_LIFETIME, Corpse};
pub use ground::{Ground, GroundQuery};
pub use locomotion::{
    DEFAULT_ACCEL, DEFAULT_TURN_RATE, LocomotionProfile, facing_rotation, facing_slerp_factor,
};
pub use senses::{
    ALERT_SECS, HEAR_MELEE, HEAR_SHOT, HEAR_THRESHOLD, HeardNoise, LOS_SLACK, LuaScriptRef,
    NoisePing, SEARCH_SECS, SEE_CONE_DOT, SEE_RANGE, SENSE_RAYS_PER_FRAME, SenseBus, SenseCursor,
    SenseEvent, SenseInbox, SenseKind, SenseMemory, SensesPlugin, drain_sense_bus, hear_combat,
    hears, in_view, sense_ai, sense_step,
};
pub use vitals::{
    DEFAULT_HEALTH, Health, Hostile, apply_damage, ensure_creature_vitals, ensure_player_vitals,
    heal_full, health_fraction, radial_damage,
};

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::reflect::TypePath;

    /// O contrato da extração: os tipos refletidos mantêm os caminhos
    /// históricos do monólito — o BRP/`viber.ecs`, os saves e os `.bsn`
    /// não podem ver um caminho novo depois da mudança de crate.
    #[test]
    fn reflected_types_keep_the_historical_viber_paths() {
        assert_eq!(AiLocomotion::type_path(), "viber::ai::AiLocomotion");
        assert_eq!(EnemyCreature::type_path(), "viber::ai::EnemyCreature");
        assert_eq!(EnemyState::type_path(), "viber::ai::EnemyState");
        assert_eq!(WanderState::type_path(), "viber::ai::WanderState");

        assert_eq!(SenseBus::type_path(), "viber::senses::SenseBus");
        assert_eq!(SenseInbox::type_path(), "viber::senses::SenseInbox");
        assert_eq!(SenseCursor::type_path(), "viber::senses::SenseCursor");
        assert_eq!(SenseKind::type_path(), "viber::senses::SenseKind");
        assert_eq!(SenseEvent::type_path(), "viber::senses::SenseEvent");
        assert_eq!(SenseMemory::type_path(), "viber::senses::SenseMemory");
        assert_eq!(NoisePing::type_path(), "viber::senses::NoisePing");

        assert_eq!(Health::type_path(), "viber::vitals::Health");
        assert_eq!(Hostile::type_path(), "viber::vitals::Hostile");

        assert_eq!(Corpse::type_path(), "viber::corpse::Corpse");
        assert_eq!(HitReact::type_path(), "viber::animation::HitReact");
        assert_eq!(
            LocomotionProfile::type_path(),
            "viber::animation::LocomotionProfile"
        );
        assert_eq!(LuaScriptRef::type_path(), "viber::luau::LuaScriptRef");
    }
}

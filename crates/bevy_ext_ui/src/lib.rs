//! Declarative UI for Bevy, extracted from the Viber engine monolith.
//!
//! The module tree mirrors the original `src/ui/**` one-to-one — a host that
//! used to `use viber::ui::…` re-exports this crate's `ui` module and every
//! path keeps working, TypePaths included (`#[type_path = "viber::ui::…"]`
//! freezes them). Three layers, one responsibility each:
//!
//! * **structure** — `<UiRoot>` / `<UiPanel>` / `<UiText>` … authored in the
//!   world XML ([`ui::tree`]);
//! * **presentation** — a CSS-like stylesheet with classes, ids, `:hover`,
//!   media queries and a real cascade ([`ui::style`]);
//! * **data & behaviour** — named bindings fed from gameplay ([`ui::bind`],
//!   [`ui::collect`]) and the `viber.ui.*` Luau surface ([`ui::script`]).
//!
//! The seams to the rest of the engine live in [`ui::host`]: contract types
//! that used to be owned by other `viber` modules (locale, luau, hud, scene,
//! worldsys) and that the host app publishes or re-exports. The set order
//! contract ([`UiSet`](bevy_ext_core::UiSet)) lives in `bevy_ext_core` and is
//! re-exported here.

pub mod ui;

#[cfg(test)]
mod type_path_tests {
    use bevy::prelude::*;

    use crate::ui::collect::UiToast;
    use crate::ui::host::{
        HudHidden, Locale, MinimapRange, SceneSource, ScriptGameEvent, ScriptInteraction,
    };
    use crate::ui::modal::UiModal;
    use crate::ui::runtime::UiBar;
    use crate::ui::script::UiScriptState;
    use crate::ui::style::StyleSheet;
    use crate::ui::tween::UiTransitions;

    /// O TypePath histórico é o contrato com os saves, o BRP (`viber.ecs`) e
    /// o editor: a extração não pode mudá-los. Cada módulo aqui representa a
    /// família congelada pelo `#[type_path]`.
    #[test]
    fn reflected_types_keep_their_historical_paths() {
        assert_eq!(UiBar::type_path(), "viber::ui::runtime::UiBar");
        assert_eq!(UiModal::type_path(), "viber::ui::modal::UiModal");
        assert_eq!(UiToast::type_path(), "viber::ui::collect::UiToast");
        assert_eq!(StyleSheet::type_path(), "viber::ui::style::StyleSheet");
        assert_eq!(
            UiTransitions::type_path(),
            "viber::ui::tween::UiTransitions"
        );
        assert_eq!(UiScriptState::type_path(), "viber::ui::script::UiScriptState");
        assert_eq!(Locale::type_path(), "viber::locale::Locale");
        assert_eq!(
            ScriptInteraction::type_path(),
            "viber::luau::components::ScriptInteraction"
        );
        assert_eq!(
            ScriptGameEvent::type_path(),
            "viber::luau::events::ScriptGameEvent"
        );
        assert_eq!(HudHidden::type_path(), "viber::hud::HudHidden");
        assert_eq!(MinimapRange::type_path(), "viber::hud::minimap::MinimapRange");
        assert_eq!(SceneSource::type_path(), "viber::scene::SceneSource");
    }
}

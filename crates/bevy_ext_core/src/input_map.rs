//! Mapa de ações — o CONTRATO (`Action`, `ActionMap`, `ActionState`) que o
//! movimento, a câmara e o netplay leem.
//!
//! Os tipos viveram no monólito (`viber::input_map`); cá ficam com o
//! TypePath congelado para o BRP e os saves. O PLUGIN não cá vem: o
//! [`ActionState`] é reescrito por um sistema que respeita o foco de texto
//! da UI (`UiFocusedInput`, do host declarativo) e resolve teclas pelo
//! parser do host Luau — dependências de serviço que o core não pode ter.
//! O monólito (`viber::input_map`) mantém `gather_actions` +
//! `InputMapPlugin` lendo ESTES tipos, e o netplay do `bevy_ext_engine`
//! escreve neles diretamente.

use std::collections::HashMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
// Preserva o TypePath do BRP/`viber.ecs` depois da extração.
#[type_path = "viber::input_map"]
pub enum Action {
    Jump,
    Sprint,
    Attack,
    Interact,
    Lock,
    Menu,
}

impl Action {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "jump" => Some(Self::Jump),
            "sprint" => Some(Self::Sprint),
            "attack" => Some(Self::Attack),
            "interact" => Some(Self::Interact),
            "lock" => Some(Self::Lock),
            "menu" => Some(Self::Menu),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Jump => "jump",
            Self::Sprint => "sprint",
            Self::Attack => "attack",
            Self::Interact => "interact",
            Self::Lock => "lock",
            Self::Menu => "menu",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Reflect)]
// Preserva o TypePath do BRP/`viber.ecs` depois da extração.
#[type_path = "viber::input_map"]
pub struct Binding {
    pub key: Option<String>,
    pub pad: Option<String>,
}

#[derive(Resource, Clone, Debug, Reflect)]
#[reflect(Resource)]
// Preserva o TypePath do BRP/`viber.ecs` depois da extração.
#[type_path = "viber::input_map"]
pub struct ActionMap {
    pub bindings: HashMap<Action, Binding>,
}

impl Default for ActionMap {
    fn default() -> Self {
        let mut bindings = HashMap::new();
        let bind = |key: &str, pad: &str| Binding {
            key: Some(key.into()),
            pad: Some(pad.into()),
        };
        bindings.insert(Action::Jump, bind("space", "south"));
        bindings.insert(Action::Sprint, bind("shift", "left_trigger"));
        bindings.insert(Action::Attack, bind("j", "west"));
        bindings.insert(Action::Interact, bind("e", "east"));
        bindings.insert(Action::Lock, bind("z", "left_trigger2"));
        bindings.insert(Action::Menu, bind("q", "start"));
        Self { bindings }
    }
}

impl ActionMap {
    pub fn rebind_key(&mut self, action: Action, key: &str) {
        self.bindings
            .entry(action)
            .or_insert(Binding {
                key: None,
                pad: None,
            })
            .key = Some(key.to_string());
    }
}

#[derive(Resource, Debug, Default, Clone, Reflect)]
#[reflect(Resource)]
// Preserva o TypePath do BRP/`viber.ecs` depois da extração.
#[type_path = "viber::input_map"]
pub struct ActionState {
    pub move_x: f32,
    pub forward: f32,
    pub jump_just: bool,
    pub sprint: bool,
    pub attack_just: bool,
    pub interact_just: bool,
    pub lock_just: bool,
    pub menu_just: bool,
}

impl ActionState {
    pub fn pressed(&self, action: Action) -> bool {
        match action {
            Action::Jump => self.jump_just,
            Action::Sprint => self.sprint,
            Action::Attack => self.attack_just,
            Action::Interact => self.interact_just,
            Action::Lock => self.lock_just,
            Action::Menu => self.menu_just,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::reflect::TypePath;

    #[test]
    fn rebind_replaces_only_the_key() {
        let mut map = ActionMap::default();
        map.rebind_key(Action::Jump, "f");
        assert_eq!(map.bindings[&Action::Jump].key.as_deref(), Some("f"));
        assert!(map.bindings[&Action::Jump].pad.is_some());
    }

    #[test]
    fn action_names_roundtrip() {
        for name in ["jump", "sprint", "attack", "interact", "lock", "menu"] {
            let action = Action::parse(name).unwrap();
            assert_eq!(action.as_str(), name);
        }
    }

    /// Contrato da extração: os TypePaths históricos ficam congelados — o
    /// BRP, os saves e os `.bsn` não podem ver um caminho novo depois do
    /// tipo mudar de crate.
    #[test]
    fn reflected_types_keep_the_historical_viber_paths() {
        assert_eq!(Action::type_path(), "viber::input_map::Action");
        assert_eq!(ActionMap::type_path(), "viber::input_map::ActionMap");
        assert_eq!(ActionState::type_path(), "viber::input_map::ActionState");
        assert_eq!(Binding::type_path(), "viber::input_map::Binding");
    }
}

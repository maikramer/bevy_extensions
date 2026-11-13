//! Parser de teclas/rato partilhado (portado de `viber::luau::input`): serve
//! `viber.input.*`, `viber.set_interaction` e `viber.interacted`. O parser é
//! tolerante (`"Arrow_Up"` → `"arrowup"`), case-insensitive e aceita o
//! teclado TODO — uma interação pode viver em qualquer tecla, não só nas
//! históricas (`e j f q r space`).

use std::sync::{Arc, RwLock};

use bevy::input::keyboard::KeyCode;
use bevy::input::mouse::MouseButton;
use bevy::prelude::*;
use mlua::{Lua, Table};

use crate::ScriptGlobals;

/// Tecla Lua (`"e"`, `"w"`, `"f5"`, `"arrowup"`/`"up"`, `"space"`,
/// `"lshift"`, `"="`) → [`KeyCode`].
pub fn key_code_from_str(key: &str) -> Option<KeyCode> {
    // Símbolos literais primeiro: a normalização abaixo só guarda
    // alfanuméricos, e estes são o seu próprio nome.
    match key {
        "=" | "+" => return Some(KeyCode::Equal),
        "-" => return Some(KeyCode::Minus),
        "[" => return Some(KeyCode::BracketLeft),
        "]" => return Some(KeyCode::BracketRight),
        ";" => return Some(KeyCode::Semicolon),
        "'" => return Some(KeyCode::Quote),
        "," => return Some(KeyCode::Comma),
        "." => return Some(KeyCode::Period),
        "/" => return Some(KeyCode::Slash),
        "\\" => return Some(KeyCode::Backslash),
        "`" => return Some(KeyCode::Backquote),
        _ => {}
    }
    // Normalização: minúsculas, só alfanuméricos (`"Arrow_Up"` → `"arrowup"`).
    let norm: String = key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let k = norm.as_str();
    // Letras e dígitos de uma só letra.
    if k.len() == 1 {
        let c = k.chars().next().unwrap();
        if c.is_ascii_lowercase() {
            return letter_key(c);
        }
        if c.is_ascii_digit() {
            return digit_key(c);
        }
    }
    // F1..F24 e teclado numérico.
    if let Some(rest) = k.strip_prefix('f')
        && let Ok(n) = rest.parse::<u8>()
    {
        return function_key(n);
    }
    if let Some(rest) = k.strip_prefix("numpad")
        && let Ok(n) = rest.parse::<u8>()
    {
        return numpad_key(n);
    }
    Some(match k {
        "space" => KeyCode::Space,
        "enter" | "return" => KeyCode::Enter,
        "escape" | "esc" => KeyCode::Escape,
        "tab" => KeyCode::Tab,
        "backspace" => KeyCode::Backspace,
        "capslock" | "caps" => KeyCode::CapsLock,
        "shift" | "lshift" => KeyCode::ShiftLeft,
        "rshift" => KeyCode::ShiftRight,
        "ctrl" | "control" | "lctrl" | "lcontrol" => KeyCode::ControlLeft,
        "rctrl" | "rcontrol" => KeyCode::ControlRight,
        "alt" | "lalt" => KeyCode::AltLeft,
        "ralt" => KeyCode::AltRight,
        "meta" | "lmeta" | "cmd" | "super" => KeyCode::SuperLeft,
        "rmeta" => KeyCode::SuperRight,
        "up" | "arrowup" => KeyCode::ArrowUp,
        "down" | "arrowdown" => KeyCode::ArrowDown,
        "left" | "arrowleft" => KeyCode::ArrowLeft,
        "right" | "arrowright" => KeyCode::ArrowRight,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "insert" => KeyCode::Insert,
        "delete" | "del" => KeyCode::Delete,
        _ => return None,
    })
}

/// Um input resolvido para `viber.input.*`: tecla OU botão do rato
/// (`"mouse1"`/`"lmb"`, `"mouse2"`/`"rmb"`, `"mouse3"`/`"mmb"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputCode {
    Key(KeyCode),
    Mouse(MouseButton),
}

/// Nome de input Lua → [`InputCode`] (tecla primeiro; `"left"` é a SETA, não
/// o rato — para o rato usar `"mouse1"`/`"lmb"`).
pub fn input_code_from_str(name: &str) -> Option<InputCode> {
    if let Some(k) = key_code_from_str(name) {
        return Some(InputCode::Key(k));
    }
    let norm: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let mouse = match norm.as_str() {
        "mouse1" | "mouseleft" | "lmb" => MouseButton::Left,
        "mouse2" | "mouseright" | "rmb" => MouseButton::Right,
        "mouse3" | "mousemiddle" | "mmb" => MouseButton::Middle,
        _ => return None,
    };
    Some(InputCode::Mouse(mouse))
}

/// `'a'..='z'` → `KeyCode::KeyA..KeyZ`.
fn letter_key(c: char) -> Option<KeyCode> {
    Some(match c {
        'a' => KeyCode::KeyA,
        'b' => KeyCode::KeyB,
        'c' => KeyCode::KeyC,
        'd' => KeyCode::KeyD,
        'e' => KeyCode::KeyE,
        'f' => KeyCode::KeyF,
        'g' => KeyCode::KeyG,
        'h' => KeyCode::KeyH,
        'i' => KeyCode::KeyI,
        'j' => KeyCode::KeyJ,
        'k' => KeyCode::KeyK,
        'l' => KeyCode::KeyL,
        'm' => KeyCode::KeyM,
        'n' => KeyCode::KeyN,
        'o' => KeyCode::KeyO,
        'p' => KeyCode::KeyP,
        'q' => KeyCode::KeyQ,
        'r' => KeyCode::KeyR,
        's' => KeyCode::KeyS,
        't' => KeyCode::KeyT,
        'u' => KeyCode::KeyU,
        'v' => KeyCode::KeyV,
        'w' => KeyCode::KeyW,
        'x' => KeyCode::KeyX,
        'y' => KeyCode::KeyY,
        'z' => KeyCode::KeyZ,
        _ => return None,
    })
}

/// `'0'..='9'` → `KeyCode::Digit0..Digit9`.
fn digit_key(c: char) -> Option<KeyCode> {
    Some(match c {
        '0' => KeyCode::Digit0,
        '1' => KeyCode::Digit1,
        '2' => KeyCode::Digit2,
        '3' => KeyCode::Digit3,
        '4' => KeyCode::Digit4,
        '5' => KeyCode::Digit5,
        '6' => KeyCode::Digit6,
        '7' => KeyCode::Digit7,
        '8' => KeyCode::Digit8,
        '9' => KeyCode::Digit9,
        _ => return None,
    })
}

/// `1..=24` → `KeyCode::F1..F24`.
fn function_key(n: u8) -> Option<KeyCode> {
    Some(match n {
        1 => KeyCode::F1,
        2 => KeyCode::F2,
        3 => KeyCode::F3,
        4 => KeyCode::F4,
        5 => KeyCode::F5,
        6 => KeyCode::F6,
        7 => KeyCode::F7,
        8 => KeyCode::F8,
        9 => KeyCode::F9,
        10 => KeyCode::F10,
        11 => KeyCode::F11,
        12 => KeyCode::F12,
        13 => KeyCode::F13,
        14 => KeyCode::F14,
        15 => KeyCode::F15,
        16 => KeyCode::F16,
        17 => KeyCode::F17,
        18 => KeyCode::F18,
        19 => KeyCode::F19,
        20 => KeyCode::F20,
        21 => KeyCode::F21,
        22 => KeyCode::F22,
        23 => KeyCode::F23,
        24 => KeyCode::F24,
        _ => return None,
    })
}

/// `0..=9` → `KeyCode::Numpad0..Numpad9`.
fn numpad_key(n: u8) -> Option<KeyCode> {
    Some(match n {
        0 => KeyCode::Numpad0,
        1 => KeyCode::Numpad1,
        2 => KeyCode::Numpad2,
        3 => KeyCode::Numpad3,
        4 => KeyCode::Numpad4,
        5 => KeyCode::Numpad5,
        6 => KeyCode::Numpad6,
        7 => KeyCode::Numpad7,
        8 => KeyCode::Numpad8,
        9 => KeyCode::Numpad9,
        _ => return None,
    })
}

/// Instala `viber.input` na tabela `viber`: input CRU de teclado e rato
/// (qualquer tecla, não só as de interação). NÃO é gateado por menus — um
/// script que queira respeitar menus compõe com `viber.menu_open()`. Os três
/// estados partilham o parser e o snapshot do frame publicado por
/// [`crate::publish_input`].
pub(crate) fn install(lua: &Lua, viber: &Table, globals: Arc<RwLock<ScriptGlobals>>) -> mlua::Result<()> {
    let input = lua.create_table()?;
    for (name, pressed, down, released) in [
        ("pressed", true, false, false),
        ("down", false, true, false),
        ("released", false, false, true),
    ] {
        let input_globals = globals.clone();
        input.set(
            name,
            lua.create_function(move |_, key: String| {
                let code = input_code_from_str(&key).ok_or_else(|| {
                    mlua::Error::runtime(format!(
                        "viber.input.{name}: input desconhecido '{key}'"
                    ))
                })?;
                let snapshot = input_globals.read().ok().map(|g| g.input.clone());
                let active = snapshot.is_some_and(|snapshot| {
                    match code {
                        InputCode::Key(k) => match (pressed, down, released) {
                            (true, _, _) => snapshot.keys_pressed.contains(&k),
                            (_, true, _) => snapshot.keys_down.contains(&k),
                            _ => snapshot.keys_released.contains(&k),
                        },
                        InputCode::Mouse(m) => match (pressed, down, released) {
                            (true, _, _) => snapshot.mouse_pressed.contains(&m),
                            (_, true, _) => snapshot.mouse_down.contains(&m),
                            _ => snapshot.mouse_released.contains(&m),
                        },
                    }
                });
                Ok(active)
            })?,
        )?;
    }
    viber.set("input", input)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_cobre_teclas_simbolos_e_rato() {
        assert_eq!(key_code_from_str("e"), Some(KeyCode::KeyE));
        assert_eq!(key_code_from_str("E"), Some(KeyCode::KeyE));
        assert_eq!(key_code_from_str("5"), Some(KeyCode::Digit5));
        assert_eq!(key_code_from_str("space"), Some(KeyCode::Space));
        assert_eq!(key_code_from_str("f5"), Some(KeyCode::F5));
        assert_eq!(key_code_from_str("F24"), Some(KeyCode::F24));
        assert_eq!(key_code_from_str("arrow_up"), Some(KeyCode::ArrowUp));
        assert_eq!(key_code_from_str("numpad7"), Some(KeyCode::Numpad7));
        assert_eq!(key_code_from_str("lshift"), Some(KeyCode::ShiftLeft));
        assert_eq!(key_code_from_str("esc"), Some(KeyCode::Escape));
        assert_eq!(key_code_from_str("="), Some(KeyCode::Equal));
        assert_eq!(key_code_from_str("/"), Some(KeyCode::Slash));
        // `"left"` é a SETA; o rato vive nos nomes próprios.
        assert_eq!(input_code_from_str("left"), Some(InputCode::Key(KeyCode::ArrowLeft)));
        assert_eq!(
            input_code_from_str("lmb"),
            Some(InputCode::Mouse(MouseButton::Left))
        );
        assert_eq!(
            input_code_from_str("mouse2"),
            Some(InputCode::Mouse(MouseButton::Right))
        );
        assert_eq!(input_code_from_str("não-existe"), None);
    }
}

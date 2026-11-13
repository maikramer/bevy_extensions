//! `<PostFxDebugToggle bindings="…">` — teclas que ligam/desligam efeitos de
//! pós-processo AO VIVO (port do plugin `postfx-toggle`; extraído do
//! monólito `viber::postfx_toggle`).
//!
//! ```xml
//! <PostFxDebugToggle />                                  <!-- defaults Ctrl+1..6 -->
//! <PostFxDebugToggle bindings="Digit1:bloom, shift+f4:ssao, ctrl+l:lens" />
//! ```
//!
//! Cada tecla alterna o mesmo gate que o `viber.debug.postfx{…}` do bridge
//! força ([`crate::runtime::fx_runtime_toggle`]), e o `sync_postfx_gates` do
//! `postfx.rs` tira/repõe o componente na câmara no frame seguinte. Os
//! defaults usam Ctrl porque os dígitos nus já são a hotbar do preset RPG.

use bevy::prelude::*;

use bevy_ext_core::MenusOpen;

use crate::env::EngineConfigs;

/// Bindings por omissão — a ordem da referência (bloom, CA, vinheta…), com os
/// dois efeitos que o Viber não comuta ao vivo (AA, tonemapping) trocados por
/// SSAO/DoF, e Ctrl para não roubar a hotbar.
pub const DEFAULT_BINDINGS: &str =
    "ctrl+1:bloom, ctrl+2:ca, ctrl+3:vignette, ctrl+4:ssao, ctrl+5:dof, ctrl+6:volumetrics";

/// Uma tecla (com modificadores exatos) → um gate de [`crate::runtime::fx_off`].
#[derive(Debug, Clone, PartialEq, Eq, Reflect)]
#[type_path = "viber::postfx_toggle"]
pub struct FxBinding {
    pub key: KeyCode,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// Key do gate (`BLOOM`, `SSAO`, `LENS`…).
    #[reflect(ignore)]
    pub fx: &'static str,
    /// Texto da binding como o autor o escreveu (para o log).
    pub label: String,
}

/// Bindings ativas. O estado ON/OFF NÃO vive aqui: é o corte runtime
/// partilhado com o bridge ([`crate::runtime::fx_forced_off`]) — uma cópia
/// local ficava dessincronizada e a 1.ª tecla depois de um
/// `viber.debug.postfx{…}` não fazia nada.
#[derive(Debug, Clone, Default, Resource, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::postfx_toggle"]
pub struct PostFxToggleBindings {
    pub bindings: Vec<FxBinding>,
}

/// Alias do autor → key do gate. `Err` explica aliases reconhecidos mas que
/// não se comutam ao vivo.
pub fn fx_key_for_alias(alias: &str) -> Result<&'static str, String> {
    let norm: String = alias
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    Ok(match norm.as_str() {
        "bloom" => "BLOOM",
        "ca" | "chromatic" | "chromaticaberration" => "CHROMATIC",
        "vignette" => "VIGNETTE",
        "ssao" | "ao" => "SSAO",
        "dof" | "depthoffield" => "DOF",
        "autoexposure" | "exposure" => "AUTOEXPOSURE",
        "contactshadows" => "CONTACT_SHADOWS",
        "cas" | "sharpen" => "CAS",
        "splittone" => "SPLITTONE",
        "aerial" | "aerialperspective" => "AERIAL",
        "volumetrics" | "volumetric" | "godrays" | "fog" => "VOLUMETRICS",
        "motionblur" => "MOTION_BLUR",
        "lens" => "LENS",
        "aa" | "taa" | "fxaa" | "tonemapping" => {
            return Err(format!(
                "'{alias}' não se comuta ao vivo no Viber (TAA/tonemapping são de arranque: VIBER_NO_TAA)"
            ));
        }
        _ => return Err(format!("efeito desconhecido '{alias}'")),
    })
}

/// `"ctrl+shift+Digit1"` → tecla + modificadores. Aceita os códigos de
/// browser (`Digit1`, `KeyB`) e os nomes do `viber.key_pressed` (`1`, `b`,
/// `f4`, `numpad3`…).
pub fn parse_key_spec(spec: &str) -> Option<(KeyCode, bool, bool, bool)> {
    let mut parts: Vec<&str> = spec.split('+').map(str::trim).collect();
    // `ctrl++` → a própria tecla `+`.
    let key = if spec.trim_end().ends_with("++") {
        parts.truncate(parts.len().saturating_sub(2));
        "+"
    } else {
        parts.pop()?
    };
    let (mut ctrl, mut shift, mut alt) = (false, false, false);
    for modifier in parts {
        match modifier.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => ctrl = true,
            "shift" => shift = true,
            "alt" | "option" => alt = true,
            _ => return None,
        }
    }
    let lower = key.to_ascii_lowercase();
    let bare = lower
        .strip_prefix("digit")
        .filter(|d| d.len() == 1)
        .or_else(|| lower.strip_prefix("key").filter(|k| k.len() == 1))
        .unwrap_or(key);
    let code = key_code_from_str(bare)?;
    Some((code, ctrl, shift, alt))
}

/// Faz o parse de `bindings="tecla:efeito, …"`. Entradas inválidas viram
/// warnings e as válidas ficam — uma gralha não custa as restantes teclas.
pub fn parse_bindings(text: &str) -> (Vec<FxBinding>, Vec<String>) {
    let mut bindings = Vec::new();
    let mut warnings = Vec::new();
    for entry in text.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let Some((key, alias)) = entry.rsplit_once(':') else {
            warnings.push(format!("binding '{entry}' sem ':' (esperado tecla:efeito)"));
            continue;
        };
        let Some((code, ctrl, shift, alt)) = parse_key_spec(key) else {
            warnings.push(format!("tecla desconhecida '{}' em '{entry}'", key.trim()));
            continue;
        };
        match fx_key_for_alias(alias.trim()) {
            Ok(fx) => bindings.push(FxBinding {
                key: code,
                ctrl,
                shift,
                alt,
                fx,
                label: entry.to_string(),
            }),
            Err(warning) => warnings.push(warning),
        }
    }
    (bindings, warnings)
}

/// Lê a tag do mundo (depois do spawn) e instala as bindings.
fn install_bindings(mut commands: Commands, configs: Option<Res<EngineConfigs>>) {
    let Some(tag) = configs.as_ref().and_then(|c| c.first("postfxdebugtoggle")) else {
        return;
    };
    if matches!(
        tag.attr("enabled").map(str::trim),
        Some("0" | "false" | "off")
    ) {
        return;
    }
    let text = tag.attr("bindings").unwrap_or(DEFAULT_BINDINGS);
    let (bindings, warnings) = parse_bindings(text);
    for warning in &warnings {
        warn!("postfx-toggle: {warning}");
    }
    info!(
        "postfx-toggle: {} tecla(s) — {}",
        bindings.len(),
        bindings
            .iter()
            .map(|b| b.label.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    commands.insert_resource(PostFxToggleBindings { bindings });
}

fn toggle_on_keys(
    keys: Option<Res<ButtonInput<KeyCode>>>,
    menus: Option<Res<MenusOpen>>,
    toggles: Option<Res<PostFxToggleBindings>>,
) {
    let (Some(keys), Some(toggles)) = (keys, toggles) else {
        return;
    };
    if menus.is_some_and(|m| m.any()) {
        return;
    }
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let alt = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    let pressed: Vec<&'static str> = toggles
        .bindings
        .iter()
        .filter(|b| b.ctrl == ctrl && b.shift == shift && b.alt == alt)
        .filter(|b| keys.just_pressed(b.key))
        .map(|b| b.fx)
        .collect();
    for fx in pressed {
        let now_off = !crate::runtime::fx_forced_off(fx);
        crate::runtime::fx_runtime_toggle(fx, !now_off);
        let state = if now_off {
            "OFF"
        } else if crate::runtime::fx_off(fx) {
            "ON — mas continua cortado (VIBER_NO_* ou tier de qualidade)"
        } else {
            "ON"
        };
        info!("postfx-toggle: {fx} {state}");
    }
}

/// Liga o `<PostFxDebugToggle>`; inerte em mundos sem a tag.
pub struct PostFxDebugTogglePlugin;

impl Plugin for PostFxDebugTogglePlugin {
    fn build(&self, app: &mut App) {
        // TODO(viber-exit): no monólito corria `.after(recipes::spawn::startup)`
        // (o parse do mundo tem de ter preenchido `EngineConfigs`). Aqui o
        // contrato é o documento do recurso: quem publica `EngineConfigs`
        // (o parser/loader do jogo) tem de o fazer no Startup antes disto.
        app.add_systems(Startup, install_bindings)
            .add_systems(Update, toggle_on_keys);
    }
}

/// TODO(viber-exit): cópia do `key_code_from_str` do `luau::input` do
/// monólito (função pura, sem o runtime de scripts) — o parser de bindings
/// precisa dela e o crate de input ainda não aterrou nesta família. Quando
/// aterrar, isto volta a ser um import.
///
/// Tecla (`"e"`, `"f5"`, `"arrowup"`/`"up"`, `"space"`, `"lshift"`, `"="`) →
/// [`KeyCode`]. Case-insensitive e tolerante a `_`/`-` (`"arrow_up"`).
fn key_code_from_str(key: &str) -> Option<KeyCode> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_bindings_all_parse() {
        let (bindings, warnings) = parse_bindings(DEFAULT_BINDINGS);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(bindings.len(), 6);
        assert!(bindings.iter().all(|b| b.ctrl && !b.shift && !b.alt));
        assert_eq!(bindings[0].key, KeyCode::Digit1);
        assert_eq!(bindings[0].fx, "BLOOM");
        assert_eq!(bindings[1].fx, "CHROMATIC");
    }

    #[test]
    fn test_browser_codes_are_accepted() {
        let (bindings, warnings) = parse_bindings("Digit1:bloom,KeyB:ssao,Numpad3:lens");
        assert!(warnings.is_empty(), "{warnings:?}");
        let keys: Vec<_> = bindings.iter().map(|b| b.key).collect();
        assert_eq!(keys, [KeyCode::Digit1, KeyCode::KeyB, KeyCode::Numpad3]);
        assert!(bindings.iter().all(|b| !b.ctrl));
    }

    #[test]
    fn test_modifiers_parse() {
        assert_eq!(
            parse_key_spec("Shift+Alt+F4"),
            Some((KeyCode::F4, false, true, true))
        );
        assert_eq!(
            parse_key_spec("ctrl++"),
            Some((KeyCode::Equal, true, false, false))
        );
        assert_eq!(parse_key_spec("hyper+1"), None);
    }

    #[test]
    fn test_bad_entries_warn_and_keep_the_rest() {
        let (bindings, warnings) =
            parse_bindings("Digit1:bloom, Digit4:aa, Digit5:tonemapping, wat:bloom, 2:sparkles, 3");
        assert_eq!(bindings.len(), 1);
        assert_eq!(warnings.len(), 5, "{warnings:?}");
        assert!(warnings[0].contains("ao vivo"));
    }

    #[test]
    fn test_aliases_are_case_and_dash_insensitive() {
        assert_eq!(fx_key_for_alias("Contact-Shadows"), Ok("CONTACT_SHADOWS"));
        assert_eq!(fx_key_for_alias("motion_blur"), Ok("MOTION_BLUR"));
        assert_eq!(fx_key_for_alias("ChromaticAberration"), Ok("CHROMATIC"));
    }

    #[test]
    fn test_key_press_toggles_gate_off_then_back() {
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(PostFxToggleBindings {
                bindings: parse_bindings("f4:splittone").0,
            })
            .add_systems(Update, toggle_on_keys);
        let press = |app: &mut App| {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.release(KeyCode::F4);
            keys.clear();
            keys.press(KeyCode::F4);
            app.update();
        };
        press(&mut app);
        assert!(crate::runtime::fx_off("SPLITTONE"), "1.º toque desliga");
        press(&mut app);
        assert!(
            std::env::var_os("VIBER_NO_SPLITTONE").is_some()
                || !crate::runtime::fx_off("SPLITTONE"),
            "2.º toque repõe"
        );
    }

    /// O bridge desligou o efeito: a 1.ª tecla tem de o LIGAR (antes o toggle
    /// só via o seu próprio registo e o 1.º toque não fazia nada).
    #[test]
    fn test_key_press_follows_the_bridge_state() {
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(PostFxToggleBindings {
                bindings: parse_bindings("f7:aerial").0,
            })
            .add_systems(Update, toggle_on_keys);
        crate::runtime::fx_runtime_toggle("AERIAL", false);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F7);
        app.update();
        assert!(!crate::runtime::fx_forced_off("AERIAL"), "1.º toque repõe");
    }
}

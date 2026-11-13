//! Comandos que scripts enfileiram e um sistema Bevy drena pós-frame
//! (portado de `viber::luau::commands`, variantes desta fase). A engine
//! provê os BLOCOS PRIMITIVOS — a composição de comportamento vive no Luau.
//!
//! O dreno ([`drain_commands`]) roteia:
//! - `Toast` → `ScriptToast` (contrato do `bevy_ext_core`, consumido pelo
//!   HUD);
//! - `PlaySfx` → `SfxEvent` (contrato do `bevy_ext_core`, consumido pelo
//!   player de áudio da integração);
//! - o resto (feedback de câmara/animção/UI) → [`ScriptCommandMessage`],
//!   para o adaptador do Vael consumir; os consumidores finais ficam para a
//!   integração.

use std::collections::VecDeque;

use bevy::prelude::*;
use bevy_ext_core::{SfxClip, SfxEvent};

/// Comando enfileirado por um script (`viber.*`) e aplicado pós-frame.
#[derive(Debug, Clone)]
pub enum ScriptCommand {
    /// SFX curto na posição dada (`None` = som de interface, volume cheio).
    PlaySfx {
        clip: SfxClip,
        position: Option<Vec3>,
    },
    /// Clip de ação no rig (`viber.play_clip`/`viber.gesture`): fuzzy match
    /// do nome na aplicação. `entity = None` — a integração resolve para a
    /// entidade do script (o BMS não dá o `ctx.entity` às funções).
    Gesture {
        entity: Option<Entity>,
        name: String,
        speed: f32,
        /// `play_clip(..., { lock = true })`: segura a última pose.
        lock: bool,
    },
    /// Câmara: shake (`viber.shake`) e kick de FOV (`viber.fov_kick`).
    Shake(f32),
    FovKick(f32),
    /// Congelamento do frame (`viber.hit_stop`), em segundos.
    HitStop(f32),
    /// Número de dano flutuante (`viber.damage_number`).
    DamageNumber {
        text: String,
        pos: Vec3,
        color: Option<[f32; 3]>,
    },
    /// Anel de choque no chão (`viber.ring`).
    Ring {
        x: f32,
        z: f32,
        radius: f32,
        color: Option<[f32; 3]>,
    },
}

/// Um [`ScriptCommand`] como mensagem Bevy: o adaptador do Vael regista um
/// `MessageReader<ScriptCommandMessage>` e aplica o feedback (shake, FOV,
/// hit-stop, números de dano, anéis, gestos) nos seus sistemas.
#[derive(Debug, Clone, bevy::ecs::message::Message)]
pub struct ScriptCommandMessage(pub ScriptCommand);

/// Id vindo de Lua → `Entity`. O `Entity::from_bits` PANICA com bits
/// inválidos; aqui vira erro Lua normal (portado de `viber::luau::entity`).
pub fn entity_from_id(bits: i64) -> mlua::Result<Entity> {
    Entity::try_from_bits(bits as u64)
        .ok_or_else(|| mlua::Error::runtime(format!("id de entidade inválido: {bits}")))
}

/// Cor `#rgb`/`#rrggbb` (ou `0x…`) → componentes sRGB em `0..=1`. Mesma
/// matemática do `parse_color` do `bevy_ext_world`, sem cores nomeadas
/// (a API Lua destes comandos documenta `#hex`).
pub fn parse_hex_color(value: &str) -> Option<[f32; 3]> {
    let hex = value
        .trim()
        .strip_prefix('#')
        .or_else(|| value.trim().strip_prefix("0x"))
        .or_else(|| value.trim().strip_prefix("0X"))?;
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match hex.len() {
        3 => Some(
            hex.chars()
                .map(|c| (c.to_digit(16).unwrap() * 17) as f32 / 255.0)
                .collect::<Vec<f32>>()
                .try_into()
                .unwrap(),
        ),
        6 => {
            let rgb = u32::from_str_radix(hex, 16).ok()?;
            Some([
                ((rgb >> 16) & 0xff) as f32 / 255.0,
                ((rgb >> 8) & 0xff) as f32 / 255.0,
                (rgb & 0xff) as f32 / 255.0,
            ])
        }
        _ => None,
    }
}

/// Drena UMA fila de comandos para os escritores de mensagens. Os tipos sem
/// registo no app (contratos ainda sem consumidor) soltam um warn — o
/// comando perde-se, mas o script não trava (o Viber fazia o mesmo).
pub(crate) fn drain_queue(
    queue: &mut VecDeque<ScriptCommand>,
    mut sfx: Option<&mut MessageWriter<SfxEvent>>,
    mut out: Option<&mut MessageWriter<ScriptCommandMessage>>,
) {
    for command in queue.drain(..) {
        match command {
            ScriptCommand::PlaySfx { clip, position } => {
                if let Some(writer) = sfx.as_mut() {
                    writer.write(SfxEvent { clip, position });
                } else {
                    log::warn!("[lua] sfx sem SfxEvent registado: {clip:?}");
                }
            }
            other => {
                if let Some(writer) = out.as_mut() {
                    writer.write(ScriptCommandMessage(other));
                } else {
                    log::warn!(
                        "[lua] comando sem ScriptCommandMessage registado: {:?}",
                        std::mem::discriminant(&other)
                    );
                }
            }
        }
    }
}

/// Sistema: drena a fila partilhada dos scripts. Corre no Update (os
/// scripts correm antes, em PreUpdate/BMS). O lock de escrita é curtíssimo:
/// só move a fila para fora.
pub(crate) fn drain_commands(
    mut sfx: Option<MessageWriter<SfxEvent>>,
    mut out: Option<MessageWriter<ScriptCommandMessage>>,
) {
    let shared = crate::globals();
    let mut queue = {
        let Ok(mut globals) = shared.write() else {
            return;
        };
        std::mem::take(&mut globals.commands)
    };
    drain_queue(&mut queue, sfx.as_mut(), out.as_mut());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cores_hex_reconhecidas() {
        assert_eq!(
            parse_hex_color("#ff8800"),
            Some([1.0, 136.0 / 255.0, 0.0])
        );
        assert_eq!(parse_hex_color("#f80"), Some([1.0, 136.0 / 255.0, 0.0]));
        assert_eq!(parse_hex_color("0xFF8800"), Some([1.0, 136.0 / 255.0, 0.0]));
        assert_eq!(parse_hex_color("#12345"), None);
        assert_eq!(parse_hex_color("vermelho"), None);
        assert_eq!(parse_hex_color("#gggggg"), None);
    }

    #[test]
    fn ids_de_entidade_invalidos_viram_erro_lua() {
        // Índice 0 nos bits baixos não é entidade (reservado) — no
        // `from_bits` do Bevy seria pânico; aqui é erro Lua normal.
        assert!(entity_from_id(0).is_err());
        let e = Entity::try_from_bits(7u64).expect("entidade válida");
        assert_eq!(entity_from_id(e.to_bits() as i64).unwrap(), e);
    }
}

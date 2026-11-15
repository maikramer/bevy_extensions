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
use mlua::Function;

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
    /// Registra alvo de interação ("[J] Minerar") na entidade
    /// (`viber.set_interaction`). `entity = None` — a integração resolve
    /// para a entidade do script.
    SetInteraction {
        entity: Option<Entity>,
        label: String,
        key: String,
        range: f32,
    },
    /// Troca de cena (`viber.change_scene`): fade, descarrega a atual,
    /// carrega a outra; `entry` = `<SceneEntry name>` ou `"@back"`. A
    /// validação de ids conhecidos fica na integração (sem registry de
    /// cenas no host).
    ChangeScene {
        scene: String,
        entry: Option<String>,
    },
    /// Teleporte da entidade para (x, y, z) (`viber.set_position`) — o
    /// consumidor faz o snap no terreno ao aplicar (a mesma regra do
    /// `MoveTowards` do monólito).
    SetPosition {
        entity: Entity,
        pos: Vec3,
    },
    // ── F4 (percepção/props) ────────────────────────────────────────────
    /// Vira a entidade para olhar um ponto (yaw only) — `viber.face_towards`
    /// / `viber.face_player` (alvo = posição do herói).
    FaceTowards { entity: Entity, target: Vec3 },
    /// Destrutível com queda (`break-style: fall`) — tomba na direção
    /// herói→entidade e despawna no fim (`viber.topple`).
    Topple { entity: Entity },
    /// Reclama uma ação da UI (`viber.own_action("buy")`) — o handler
    /// nativo correspondente cala e a ação chega a Lua via `viber.events()`.
    OwnUiAction(String),
    /// Remove uma entidade (`viber.entity_despawn` / `viber.despawn_self`).
    Despawn(Entity),
    /// Reclama um sistema nativo (`viber.own_system("dialogue")`) — o
    /// handler nativo correspondente cala e a lógica passa a ser do script.
    OwnSystem(String),
    /// Devolve um sistema nativo ao handler (`viber.release_system`) — o
    /// inverso de [`Self::OwnSystem`].
    ReleaseSystem(String),
    /// Esconde/mostra uma entidade e a subárvore (`viber.set_visible`) — a
    /// `Visibility` do Bevy propaga aos filhos com Inherited.
    SetVisible { entity: Entity, visible: bool },
    /// Escala uniforme da entidade (`viber.set_scale`, 0.05..20).
    SetScale { entity: Entity, scale: f32 },
    /// Pose LOCAL da entidade (`viber.set_pose`). O yaw é
    /// `Quat::from_rotation_y`; a escala fica. Com `own_system("locomotion")`
    /// o herói deixa de andar a pé.
    SetPose { entity: Entity, pos: Vec3, yaw: f32 },
    /// Burst de partículas (`viber.burst`) — preset validado contra a
    /// biblioteca publicada (`ScriptHostState::particle_presets`; `sparks`
    /// é o preset de impacto do melee, fora da biblioteca).
    Burst {
        preset: String,
        pos: Vec3,
        count: usize,
    },
    /// Decal de impacto (`viber.decal`): quad projectado com fade.
    Decal {
        url: String,
        position: Vec3,
        normal: Vec3,
        size: f32,
        life: f32,
        color: [f32; 3],
    },
    /// Instancia um `<Prototype>` do mundo (`viber.spawn_prototype`) — o
    /// spawn REAL é pós-frame na integração, com callback `(bits)`.
    SpawnPrototype {
        name: String,
        pos: Vec3,
        /// Sem cota explícita: assenta o Y na superfície renderizada.
        seat: bool,
        on_spawned: Option<Function>,
        caller_path: Option<String>,
    },
    /// Acende o flash de impacto na entidade (`viber.hit_flash`) — leitura
    /// visual do golpe, sem dano nem regra de combate.
    HitFlash { entity: Entity, secs: f32 },
    /// Marca a entidade como cadáver (`viber.corpse`): o jogo decide a
    /// morte; a engine só deixa de a animar e, com ragdoll, começa o ragdoll.
    Corpse { entity: Entity },
    /// Anexa um GLB a um osso (`viber.attach`) ou remove o slot
    /// (`url = None`, `viber.detach`). Genérico: a engine não sabe o que é
    /// uma arma; só encontra o osso e pendura o prop.
    Attach {
        owner: Option<Entity>,
        slot: String,
        url: Option<String>,
        bone: String,
        pos: Vec3,
        rot: Vec3,
        scale: f32,
    },
}

/// Alcance BASE autorado de interação (o default do `set_interaction`),
/// igual ao `BASE_RANGE_M` do Viber.
pub const BASE_RANGE_M: f32 = 3.5;

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

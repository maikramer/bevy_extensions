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
    // ── F5 (efeitos/áudio) ──────────────────────────────────────────────
    /// Loop de SFX (`viber.sound_loop`). `clip` é nome do registry ou caminho
    /// relativo ao `sfx_dir` (`vehicles/engine_rev.ogg`).
    SoundLoop {
        id: String,
        clip: String,
        pitch: f64,
        volume: f32,
        position: Option<Vec3>,
        /// `true` = bus de ambiente (`<AudioMixer ambience>`); `false` = SFX.
        ambience: bool,
        /// Entidade cujo script pediu o loop (o loop morre com ela). `None`
        /// em chamadas fora de `on_update` (tick/REPL).
        owner: Option<Entity>,
    },
    /// Pára um loop (`viber.sound_stop`).
    SoundStop(String),
    /// Força a layer de BGM (`viber.music.layer`). `None` devolve o automático.
    MusicLayer(Option<String>),
    /// Liga/desliga a música de combate (`viber.music.combat`). `boss` promove
    /// para a layer `boss`; `active=false` corta já (sem esperar o hold).
    MusicCombat { active: bool, boss: bool },
    /// Duck temporário de um bus (`viber.audio.duck`): `gain` linear durante
    /// `secs` segundos. Expira sozinho.
    AudioDuck { bus: String, gain: f32, secs: f32 },
    /// Reverb dos buses de SFX/voz (`viber.audio.reverb`).
    AudioReverb { preset: String, wet: Option<f32> },
    /// Pulso de pós-processo (`viber.punch`): saturação de stops + bloom.
    Punch { stops: f32, bloom: f32 },
    /// Solavanco direcional da câmara (`viber.kick`).
    CameraKick(Vec3),
    /// `viber.lantern(intensity)` — acende/reforça a lanterna do viajante
    /// (a tocha equipada ilumina mesmo de dia; 0 = só a automática nocturna).
    SetLantern(f32),
    /// `viber.bone_offset(bone, rx, ry, rz)` — rotação ADITIVA num osso do
    /// herói, aplicada por cima de qualquer animação (idle/walk/run).
    /// `Quat::IDENTITY` remove a entrada.
    SetBoneOffset { bone: String, rotation: Quat },
    /// Câmara de jogo conduzida por script (`viber.camera_free`). `false`
    /// devolve a pose ao rig de terceira pessoa no frame seguinte.
    CameraFree(bool),
    /// Pose do modo livre (`viber.camera_pose`): olho + ponto olhado.
    CameraPose { pos: Vec3, look: Vec3 },
    // ── F6 (IA/combate/cinemática) ───────────────────────────────────────
    /// Dano direto numa entidade (`viber.entity_damage`) — sem i-frames
    /// (esses são do path do player). HP ≤ 0 EMITE `Kill` na aplicação e
    /// NÃO corre o caminho nativo (cadáver/XP/quests são do melee).
    EntityDamage { entity: Entity, amount: f32 },
    /// Cura (`viber.entity_heal`) — o resultado nunca sai de `[0, max]`.
    EntityHeal { entity: Entity, amount: f32 },
    /// Redimensiona as vitais (`viber.entity_set_max_hp`); cria o `Health`
    /// se faltar (current = max).
    EntitySetMaxHp { entity: Entity, max: f32 },
    /// Dano radial (`viber.radial_damage`): entidade por distância ao
    /// centro, com falloff; `skip` exclui o emissor.
    RadialDamage {
        pos: Vec3,
        radius: f32,
        max_damage: f32,
        skip: Option<Entity>,
    },
    /// Projétil da engine (`viber.fire_projectile`).
    FireProjectile {
        from: Vec3,
        dir: Vec3,
        speed: f32,
        damage: f32,
        faction: String,
        /// `arc` = arco balístico (gravidade); `false` = linha reta.
        arc: bool,
        shooter: Option<Entity>,
    },
    /// Empurrão no herói (`viber.push_player`): impulso na direção dada.
    PushPlayer { dir: Vec3, force: f32 },
    /// Knockback numa entidade (`viber.entity_knockback`): impulso cinemático.
    EntityKnockback { entity: Entity, dir: Vec3, force: f32 },
    /// Movimento de script (`viber.move_towards`/`move_by`): vai para
    /// `AiLocomotion` (o consumidor aplica a velocidade — snap no terreno).
    MoveTowards { entity: Entity, target: Vec3, speed: f32 },
    /// Velocidade base do herói (`viber.set_player_speed`, m/s).
    SetPlayerSpeed(f32),
    /// Cinemática do mundo (`viber.cinematic(id[, anchor])`).
    PlayCinematic { id: String, anchor: Option<Entity> },
    /// Evento de sentidos (`viber.sense`): ruído audível no `SenseBus`.
    Noise { pos: Vec3, loudness: f32 },
    // ── F7 (save/mundo/UI de texto) ─────────────────────────────────────
    /// Pedido de save (`viber.save`/`viber.save_slot`) — o jogo decide
    /// (autosave/slot/legado).
    SaveGame(Option<u32>),
    /// Pedido de load (`viber.load_save`/`viber.load_slot`).
    LoadGame(Option<u32>),
    /// Recortes de terreno (`viber.terrain.*`): edição voxel via
    /// `TerrainEditQueue` na integração.
    TerrainEdit {
        kind: String,
        pos: Vec3,
        radius: f32,
        strength: f32,
    },
    // ── F7-b (fala/UI de texto/acessibilidade) ───────────────────────────
    /// Fala (`viber.say(texto[, segs][, orador])`): o jogo apresenta no
    /// balão próprio (`ExternalSayPresenter`); sem ele, o balão nativo.
    Say {
        text: String,
        secs: f32,
        speaker: Option<Entity>,
    },
    /// Fecha o jogo (`viber.quit`) — `AppExit` limpo; o save é do jogo.
    Quit,
    /// Acessibilidade (`viber.a11y_set{…}`): o jogo desenha; a engine aplica.
    A11ySet {
        subtitle_scale: Option<f32>,
        shake: Option<bool>,
        hitstop: Option<bool>,
        interact_hold: Option<bool>,
    },
    /// Marcadores do HUD do mundo (`viber.markers{…}`): blips (cap 32),
    /// beacons (cap 16) e o waypoint com rótulo.
    SetMarkers {
        blips: Vec<(Vec3, bool)>,
        beacons: Vec<Vec3>,
        waypoint: Option<(Vec3, String)>,
    },
    /// Prompt de interação de MUNDO (`viber.prompt(key, label)`); vazio limpa.
    Prompt { key: String, label: String },
    /// Idioma ativo (`viber.locale.set(lang)`); a tabela recarrega no frame
    /// seguinte (`Say`/`toast`/UI resolvem as chaves `@…`).
    SetLocale(String),
    // ── sinais ───────────────────────────────────────────────────────────
    /// `viber.emit(sinal, args…)`: entrega aos `connect-<sinal>` do nó
    /// emissor. A integração resolve os alvos e corre os handlers no
    /// contexto de cada um (o `viber.sender()` vê quem emitiu).
    EmitSignal {
        emitter: Option<Entity>,
        signal: String,
        args: Vec<serde_json::Value>,
    },
    /// `viber.call_group(grupo, método, args…)`: chama o método em todos
    /// os membros publicados do grupo.
    CallGroup {
        group: String,
        method: String,
        args: Vec<serde_json::Value>,
        sender: Option<Entity>,
    },
    /// Timer a disparar (`viber.after`/`viber.every` vencidos): o host BMS
    /// reenfileira a callback no contexto do path que a criou.
    TimerFire {
        path: String,
        id: String,
        /// `None` = one-shot (`after`); `Some(period)` = repetição (`every`).
        period: Option<f64>,
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

/// Bus de áudio aceite por `viber.audio.duck` — os MESMOS nomes do
/// `AudioBus::parse` do monólito (o comando leva a string; o consumidor
/// revalida contra o seu mixer).
pub fn is_valid_audio_bus(bus: &str) -> bool {
    matches!(
        bus,
        "master" | "music" | "sfx" | "voice" | "ambience" | "ambient"
    )
}

/// Preset de reverb aceite por `viber.audio.reverb` (o mesmo conjunto do
/// `ReverbPreset::parse`).
pub fn is_valid_reverb_preset(preset: &str) -> bool {
    matches!(preset, "off" | "room" | "hall" | "cave")
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

//! O loop de frame dos scripts: `luau_on_add` (activa chunks), `luau_update`
//! (semeia snapshots, corre `on_update`, drena e aplica comandos,
//! fan-out de eventos), `aggro_alert_system` (hook `on_player_attack`) e
//! `luau_on_remove`. É o ÚNICO sítio onde o mundo Bevy é mutado por scripts.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use crate::player::Player;
use crate::vitals::Health;

use super::commands::ScriptCommand;
use super::components::{DEFAULT_ACTIVATION_RADIUS, LuaScriptRef, ScriptActivation, ScriptInteraction};
use super::ctx::{ScriptCtx, SurfaceCache, SurfaceRegistries};
use super::events::{ScriptEventQueue, fan_out_events};
use super::host::LuaScriptHost;

/// Entidade nomeada com pose (snapshot de `viber.find`/`entity_position`).
type NamedPose = (Entity, &'static Name, &'static GlobalTransform);
/// Filtro do delta do snapshot: nome ou pose mudaram (herói excluído).
type NameOrPoseChanged = (
    Without<Player>,
    Or<(Changed<Name>, Changed<GlobalTransform>)>,
);

/// Tira `bits` do snapshot de nomes (`find`) e de posições.
fn forget_named(
    named: &mut HashMap<String, Vec<i64>>,
    positions: &mut HashMap<i64, (String, Vec3)>,
    bits: i64,
) {
    let Some((name, _)) = positions.remove(&bits) else {
        return;
    };
    if let Some(list) = named.get_mut(&name) {
        list.retain(|b| *b != bits);
        if list.is_empty() {
            named.remove(&name);
        }
    }
}

/// Hook `on_add`: when a [`LuaScriptRef`] appears, ensure its chunk is loaded
/// (from `<scripts_dir>/<path>`) and run its top level. Errors warn once and
/// never abort. A posição de spawn (`Transform` autoral — o `GlobalTransform`
/// ainda não propagou no primeiro frame) alimenta o `home` do wander.
pub fn luau_on_add(
    mut host: ResMut<LuaScriptHost>,
    added: Query<(Entity, &LuaScriptRef, Option<&Transform>), Added<LuaScriptRef>>,
) {
    for (entity, lref, transform) in &added {
        let origin = transform.map(|t| t.translation).unwrap_or(Vec3::ZERO);
        let result = host
            .ensure_loaded(&lref.path)
            .and_then(|()| host.activate_at(entity, &lref.path, origin));
        if let Err(e) = result {
            host.warn_once(&lref.path, &e);
        }
    }
}

/// Runs `on_update(dt)` of every live script, then applies queued commands
/// (posição com snap no terreno, teleporte, vitals do player, toasts,
/// interações). A script error is pcall'd: warned once, engine keeps running.
/// Estado por-runtime que sobrevive entre frames, agrupado num `SystemParam`:
/// o `luau_update` chegou ao limite de 16 parâmetros do Bevy.
#[derive(bevy::ecs::system::SystemParam)]
pub struct LuauRuntimeLocals<'w, 's> {
    /// Cena carregada e índice de cenas (`viber.scene_name`, `viber.change_scene`).
    pub scene: Option<bevy::ecs::system::Res<'w, crate::scene_loader::CurrentScene>>,
    pub scene_index: Option<bevy::ecs::system::Res<'w, crate::scene_loader::SceneIndex>>,
    /// Bioma atual (`viber.biome`) — escrito pelo `biome_fog_system` 4×/s.
    pub biome: Option<bevy::ecs::system::Res<'w, crate::ambient::CurrentBiome>>,
    /// Warn 1× por entidade sem rig/clip de gesto.
    pub gesture_warned: bevy::ecs::system::Local<'s, std::collections::HashSet<Entity>>,
    /// Snapshot partilhado de estradas/água (`viber.on_road`/`in_water`).
    pub surfaces: bevy::ecs::system::Local<'s, SurfaceCache>,
    /// Locomoção das entidades scriptadas. Fica aqui, e não como parâmetro
    /// solto, porque o `luau_update` já está no teto de 16 parâmetros do Bevy.
    /// Disjunta da query `scripts` por componente (`AiLocomotion` /
    /// `LocomotionProfile` vs `Transform`), logo as duas coexistem.
    pub locomotion: Query<
        'w,
        's,
        (
            &'static mut crate::ai::AiLocomotion,
            Option<&'static mut crate::animation::LocomotionProfile>,
        ),
    >,
    /// Rato para `viber.input.*` (`"mouse1"`). `Option`: apps mínimas de teste
    /// não registam `ButtonInput<MouseButton>`.
    pub mouse: Option<
        bevy::ecs::system::Res<'w, bevy::input::ButtonInput<bevy::input::mouse::MouseButton>>,
    >,
    /// Fila de eventos engine→Lua (producers → fan-out por-path). `Option`
    /// pelas apps mínimas, como o resto.
    pub events: Option<bevy::ecs::system::ResMut<'w, ScriptEventQueue>>,
    /// Props anexados a ossos (`viber.attach`/`detach`) — consumidos pelo
    /// `AttachmentPlugin` da app completa.
    pub attachments: Option<bevy::ecs::system::ResMut<'w, crate::attachment::AttachmentRequests>>,
    /// Warn 1× por path com a fila de eventos cheia.
    pub events_dropped: bevy::ecs::system::Local<'s, std::collections::HashSet<String>>,
    /// HP mutável de entidades NÃO-player (`viber.entity_*`). A MESMA query
    /// alimenta o snapshot de leitura do frame (`entity_hp`). `Without<Player>`
    /// disjunta da query `players` (o herói vive em `player_hp`).
    pub healths: Query<
        'w,
        's,
        (
            Entity,
            &'static mut crate::vitals::Health,
            &'static GlobalTransform,
        ),
        Without<Player>,
    >,
    /// Nomes + posição para os snapshots `viber.find`/`entity_position`/
    /// `nearby` (herói excluído).
    pub names: Query<'w, 's, (Entity, &'static Name, &'static GlobalTransform), Without<Player>>,
    /// `Name` de QUALQUER entidade — semeia o `ctx.self_name` de cada script
    /// ao frame, FORA do snapshot de nomes (o snapshot é completo hoje, mas o
    /// nome próprio não pode depender de nunca truncar: repro histórica dos
    /// portais do simple-rpg, 20 portais em ~79k nomes com cap 4096).
    pub entity_names: Query<'w, 's, &'static Name>,
    /// `GlobalTransform` de QUALQUER entidade — fallback do `origin_of` dos
    /// timers. Um timer registado num `on_spawned` pertence à entidade NOVA,
    /// que pode não ter script: a query `scripts` não a vê e o dono era
    /// tratado como morto (o timer era descartado no 1.º tick, em silêncio).
    pub all_globals: Query<'w, 's, &'static GlobalTransform>,
    /// Grupos (`groups=`) — snapshot `ctx.groups` por frame.
    pub groups: Query<'w, 's, (Entity, &'static crate::luau::Groups)>,
    /// Conexões de sinal dos emissores (`connect-*`).
    pub connections: Query<'w, 's, &'static crate::luau::SignalConnections>,
    /// Cinemática a tocar (`viber.cinematic_playing`).
    pub cinematic: Option<bevy::ecs::system::Res<'w, crate::cinematic::CinematicPlayback>>,
    /// Câmara de jogo conduzida por script (`viber.camera_free`/
    /// `camera_pose`). `Option` pelas apps mínimas de teste.
    pub script_camera: Option<bevy::ecs::system::ResMut<'w, crate::camera::ScriptCameraFree>>,
    /// Pose viva da câmara para `viber.camera_state()`.
    pub camera_snapshot: Option<bevy::ecs::system::Res<'w, crate::camera::CameraSnapshot>>,
    /// Menus que roubam input (`viber.menu_open`). `Option`: apps mínimas.
    pub menus: Option<bevy::ecs::system::Res<'w, crate::menus::MenusOpen>>,
    /// Snapshot de nomes semeado (a partir daí só deltas).
    pub names_seeded: bevy::ecs::system::Local<'s, bool>,
    /// Entidades nomeadas cujo nome ou pose mudou desde o último frame.
    pub names_changed: Query<'w, 's, NamedPose, NameOrPoseChanged>,
    /// Nomes removidos (inclui despawns) — saem do snapshot.
    pub names_removed: bevy::ecs::lifecycle::RemovedComponents<'w, 's, Name>,
    /// Props `prop-*` das entidades com script — semeia `ctx.self_props`.
    pub script_props: Query<'w, 's, (Entity, &'static crate::luau::ScriptProps)>,
    /// Warn 1× por chave para avisos que um script repete por frame (plugin
    /// ausente, recurso desconhecido, vault curto) — sem isto o log enchia.
    /// (Só a entrega de sinais o usa: os avisos dos comandos vivem agora no
    /// recurso `ScriptWarnState`, partilhado com a ponte BMS.)
    pub once_warned: bevy::ecs::system::Local<'s, HashSet<String>>,
    /// Biblioteca de `<Prototype>` (snapshot `ctx.prototype_names`).
    pub prototypes: Option<bevy::ecs::system::Res<'w, crate::recipes::spawn::PrototypeLibrary>>,
    /// Fila de spawns de prototypes (`viber.spawn_prototype`).
    pub spawns: Option<bevy::ecs::system::ResMut<'w, crate::recipes::spawn::PendingScriptSpawns>>,
    /// Ações da UI reclamadas por scripts (`viber.own_action`).
    pub ui_action_owners: Option<bevy::ecs::system::ResMut<'w, crate::ui::actions::UiActionOwners>>,
    /// Pedido de save/load de script (`viber.save()` / `viber.load()`).
    pub save_request: Option<bevy::ecs::system::ResMut<'w, crate::save::SaveRequest>>,
    /// Chave e pasta do save para `viber.save_slots()` (metadados no menu).
    pub save_key: Option<bevy::ecs::system::Res<'w, crate::save::WorldSaveKey>>,
    pub save_dir: Option<bevy::ecs::system::Res<'w, crate::save::SaveDir>>,
    /// Sistemas nativos reclamados por scripts (`viber.own_system`).
    pub system_owners: Option<bevy::ecs::system::ResMut<'w, super::ownership::ScriptSystemOwners>>,
    /// Sentidos deste frame (`viber.sense` / `on_sense`).
    pub sense: Option<bevy::ecs::system::Res<'w, crate::senses::SenseInbox>>,
    /// Mapa de ações (`viber.action`).
    pub actions: Option<bevy::ecs::system::Res<'w, crate::input_map::ActionState>>,
    /// Balão de diálogo do HUD — mesma query do diálogo nativo (`viber.say`).
    pub balloons: Query<
        'w,
        's,
        (
            &'static mut Visibility,
            &'static mut crate::hud::HudBalloon,
            &'static Children,
        ),
    >,
    pub balloon_texts: Query<'w, 's, &'static mut Text>,
    /// Warn 1× quando um mundo não declara `<DialogueBalloon>`.
    pub balloon_warned: bevy::ecs::system::Local<'s, bool>,
    /// `viber.say` → apresentação do jogo (balões no mundo, caixa, legendas).
    /// `Option` pelas apps mínimas de teste, que não registam a mensagem.
    pub spoken: Option<bevy::ecs::message::MessageWriter<'w, crate::luau::ScriptSay>>,
    /// O jogo apresenta as falas sozinho: o balão nativo fica por escrever.
    pub external_say: Option<bevy::ecs::system::Res<'w, crate::dialogue::ExternalSayPresenter>>,
    /// Recursos de FX das primitivas de script (`viber.burst/ring/shake/…`).
    pub fx: super::fx::ScriptFx<'w>,
    /// Decals de impacto pedidos por script (`viber.decal`).
    pub decals: Option<bevy::ecs::system::ResMut<'w, crate::decal_fx::DecalQueue>>,
    /// Fila das edições vivas do terreno (`viber.terrain.*`): o script
    /// enfileira, o `apply_terrain_edits` (PreUpdate seguinte) aplica ao
    /// overlay. `Option` pelas apps mínimas, como o resto.
    pub terrain_edits:
        Option<bevy::ecs::system::ResMut<'w, crate::terrain::delta::TerrainEditQueue>>,
    /// Pedidos de `viber.fire_projectile` (consumidos pelo `ProjectilePlugin`).
    pub projectiles: Option<bevy::ecs::system::ResMut<'w, crate::projectile::ProjectileQueue>>,
    /// Pose local de entidades SEM script e SEM `Player` (`viber.set_pose`
    /// em rodas/karts rivais). Disjunta das queries `scripts` e `players`.
    pub poses: Query<'w, 's, &'static mut Transform, (Without<LuaScriptRef>, Without<Player>)>,
    /// Loops de SFX pedidos por script (`viber.sound_loop`).
    pub sound_loops: Option<bevy::ecs::system::ResMut<'w, crate::music::ScriptSoundLoops>>,
    /// Layer de BGM forçada (`viber.music.layer`).
    pub music_layer: Option<bevy::ecs::system::ResMut<'w, crate::music::MusicLayerOverride>>,
    /// Estado da música de combate (`viber.music.combat`).
    pub combat_music: Option<bevy::ecs::system::ResMut<'w, crate::music::CombatMusicState>>,
    /// Ducking de buses (`viber.audio.duck`).
    pub audio_duck: Option<bevy::ecs::system::ResMut<'w, crate::music::AudioDuck>>,
    /// Reverb dos buses (`viber.audio.reverb`).
    pub audio_reverb: Option<bevy::ecs::system::ResMut<'w, crate::music::AudioReverb>>,
    /// Marcas de HUD (`viber.markers`).
    pub markers: Option<bevy::ecs::system::ResMut<'w, crate::hud::markers::HudMarkers>>,
    /// Tabela de idioma (`locale/<lang>.json`) para `viber.say`/`viber.toast`
    /// e para a troca em runtime (`viber.locale.set`).
    pub locale: Option<bevy::ecs::system::ResMut<'w, crate::locale::Locale>>,
    /// Acessibilidade (`viber.a11y`/`viber.a11y_set`).
    pub a11y: Option<bevy::ecs::system::ResMut<'w, crate::a11y::A11y>>,
    /// Listen server / cliente (`viber.net_role`).
    pub net: Option<bevy::ecs::system::Res<'w, crate::netplay::NetLink>>,
    /// Alvos hostis autorados (`hostile="true"` / `<Creature role="enemy">`).
    /// Inclui a FSM Rust sem script; a engine não infere pelo path.
    pub hostile_entities: Query<
        'w,
        's,
        (
            Entity,
            Option<&'static crate::luau::LuaScriptRef>,
            Option<&'static crate::ai::EnemyCreature>,
            &'static GlobalTransform,
        ),
        (With<crate::vitals::Hostile>, Without<Player>),
    >,
    /// Velocidade base do herói (`viber.set_player_speed`).
    pub players_speed: Query<'w, 's, &'static mut crate::player::Player, With<Player>>,
    /// Luz da tocha controlada pelo jogo (`viber.lantern`).
    pub lantern: Option<bevy::ecs::system::ResMut<'w, crate::ambient::TorchLightOverride>>,
    /// Prompt de mundo publicado por um controller (`viber.prompt`).
    pub ui_prompt: Option<bevy::ecs::system::ResMut<'w, crate::ui::UiPrompt>>,
}

pub fn luau_update(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    // `Option`: apps mínimas (testes, headless) montam sistemas soltos sem o
    // recurso — sem foco, o `interacted` cai no teste de alcance de sempre.
    focus: Option<Res<crate::interact::InteractionFocus>>,
    // Barra de fala: com um UiInput focado, as teclas são TEXTO, não acções.
    typing: Option<Res<crate::ui::widgets::UiFocusedInput>>,
    mut host: ResMut<LuaScriptHost>,
    mut scripts: Query<
        (
            Entity,
            &LuaScriptRef,
            Option<&mut Transform>,
            // O MUNDO da entidade. O `Transform` é LOCAL: um NPC dentro do
            // grupo `city` (assentado a y≈38,7 no terreno) tem local y=0, e
            // usar isso como posição punha-o 38 m abaixo do herói — o
            // `viber.interacted` (raio 3,5 m) nunca disparava e o [E] "não
            // fazia nada" em toda a cidade (repro do utilizador 2026-09-12).
            Option<&GlobalTransform>,
            Option<&ScriptActivation>,
            Option<&ScriptInteraction>,
        ),
        Without<Player>,
    >,
    mut players: Query<
        (
            Entity,
            &GlobalTransform,
            Option<&mut Transform>,
            Option<&mut Health>,
        ),
        With<crate::player::Player>,
    >,
    terrain: Option<Res<crate::terrain::runtime::TerrainRuntime>>,
    // Rig do herói para o snapshot `ctx.clip_durations` (os gestos vivem
    // agora na extração `bms_bridge::apply_script_command`).
    animators: Query<&crate::animation::CharacterAnimator>,
    // Estado persistente agrupado num `SystemParam` (o Bevy limita sistemas a
    // 16 parâmetros — este trio chegou a esse teto).
    mut locals: LuauRuntimeLocals,
    // Avisos 1× + mapa-sombra de HP criado por `Commands` — recurso
    // partilhado com a ponte BMS (`bms_bridge::apply_script_command`).
    // `Option`: apps mínimas de teste montam o sistema sem o plugin — o
    // recurso nasce pelo `commands.init_resource` do 1.º frame.
    mut warn: Option<ResMut<super::bms_bridge::ScriptWarnState>>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    let elapsed = time.elapsed_secs_f64();
    // Locale para `viber.localize` (tabela estática por run — o idioma só
    // muda reiniciando, por isso semeia-se uma vez).
    if let Some(locale) = locals.locale.as_deref()
        && let Some(mut ctx) = host.lua.app_data_mut::<ScriptCtx>()
        && ctx.locale_table.is_none()
    {
        ctx.locale_table = Some(std::sync::Arc::new(locale.table.clone()));
        ctx.locale_lang = Some(locale.lang.clone());
    }
    // Chave/pasta do save para `viber.save_slots()` (o path é fixo por run).
    if let Some(mut ctx) = host.lua.app_data_mut::<ScriptCtx>() {
        if ctx.world_key.is_none() {
            ctx.world_key = locals.save_key.as_deref().and_then(|k| k.0.clone());
        }
        if ctx.save_dir.is_none() {
            ctx.save_dir = locals.save_dir.as_deref().and_then(|d| d.0.clone());
        }
    }
    let (player_pos, player_components, player_forward) = match players.single_mut() {
        Ok((p_entity, global, transform, health)) => {
            // Forward do modelo (+Z) na MESMA extração (o borrow mutável de
            // `players` vive em `player_components`).
            let f = crate::player::hero_forward(global);
            (
                Some(global.translation()),
                Some((p_entity, transform, health)),
                Vec2::new(f.x, f.z),
            )
        }
        Err(_) => (None, None, Vec2::ZERO),
    };
    let just_pressed: Vec<KeyCode> = keys.get_just_pressed().copied().collect();
    let keys_down: Vec<KeyCode> = keys.get_pressed().copied().collect();
    let keys_released: Vec<KeyCode> = keys.get_just_released().copied().collect();

    // Snapshot dos props autorais para `viber.nearby` (dados, não regra).
    if let Some(mut ctx) = host.lua.app_data_mut::<ScriptCtx>() {
        ctx.props_snapshot.clear();
        for (entity, props) in locals.script_props.iter() {
            ctx.props_snapshot
                .insert(entity.to_bits() as i64, props.0.clone());
        }
    }

    // Semeia as teclas ANTES de correr os on_update: `viber.interacted`
    // tem de ver as teclas pressionadas NESTE frame (semear só no fim
    // fazia os scripts lerem o snapshot do frame anterior).
    if let Some(mut ctx) = host.lua.app_data_mut::<ScriptCtx>() {
        ctx.just_pressed = just_pressed;
        ctx.keys_down = keys_down;
        ctx.keys_released = keys_released;
        // Com um `UiInput` focado as teclas são TEXTO: semeia-se aqui, com o
        // resto do estado do frame, porque os timers (e os contactos) correm
        // fora do loop dos `on_update` — liam o valor do frame anterior e o
        // primeiro frame da barra de fala disparava [E]/hotbar.
        ctx.ui_typing = typing.as_deref().is_some_and(|focused| focused.0.is_some());
        // Relógio, herói e ações do frame, semeados ANTES de qualquer hook:
        // timers, sinais, contactos e o aggro-chain correm fora do
        // `run_update`, e sem isto `viber.time()` ficava no instante do último
        // `on_update` (ou 0 se nenhum script correu) e `viber.after()` agendava
        // a partir desse instante — o timer disparava logo.
        ctx.elapsed = elapsed;
        ctx.dt = dt;
        ctx.player = player_pos;
        // Sentidos do frame por entidade: `viber.sense()` resolve pelo
        // `ctx.entity` de cada hook, sem herdar o pulso de outra entidade.
        // As chaves são ESTÁTICAS (`SenseKind::as_str`) — sem `to_string`
        // por pulso por frame.
        ctx.sense_by_entity = locals
            .sense
            .as_deref()
            .map(|inbox| {
                inbox
                    .pulses
                    .iter()
                    .map(|(entity, kind)| (entity.to_bits() as i64, kind.as_str()))
                    .collect()
            })
            .unwrap_or_default();
        // Ações do frame: array FIXO de chaves estáticas (o
        // `HashMap<String, bool>` realocava o mapa e seis strings por frame).
        let action_state = locals.actions.as_deref();
        let pressed = |action: crate::input_map::Action| {
            action_state.is_some_and(|state| state.pressed(action))
        };
        use crate::input_map::Action;
        ctx.action_pressed = [
            (Action::Jump.as_str(), pressed(Action::Jump)),
            (Action::Sprint.as_str(), pressed(Action::Sprint)),
            (Action::Attack.as_str(), pressed(Action::Attack)),
            (Action::Interact.as_str(), pressed(Action::Interact)),
            (Action::Lock.as_str(), pressed(Action::Lock)),
            (Action::Menu.as_str(), pressed(Action::Menu)),
        ];
        match locals.mouse.as_deref() {
            Some(mouse) => {
                ctx.mouse_pressed = mouse.get_just_pressed().copied().collect();
                ctx.mouse_down = mouse.get_pressed().copied().collect();
                ctx.mouse_released = mouse.get_just_released().copied().collect();
            }
            None => {
                ctx.mouse_pressed.clear();
                ctx.mouse_down.clear();
                ctx.mouse_released.clear();
            }
        }
        // Vencedor por tecla deste frame (decidido no PreUpdate).
        ctx.interaction_focus = focus
            .as_deref()
            .map(crate::interact::InteractionFocus::snapshot)
            .unwrap_or_default();
        // Snapshot do HP do herói para `viber.player_hp` (frame-start).
        ctx.player_hp = player_components
            .as_ref()
            .and_then(|(_, _, health)| health.as_deref().map(|h| (h.current, h.max)));
        ctx.player_bits = player_components
            .as_ref()
            .map(|(entity, _, _)| entity.to_bits() as i64);
        ctx.clip_durations = player_components
            .as_ref()
            .and_then(|(entity, _, _)| animators.get(*entity).ok())
            .map(|animator| {
                animator
                    .clip_names
                    .iter()
                    .zip(animator.durations.iter())
                    .map(|(name, duration)| {
                        (crate::animation::normalize_clip_name(name), *duration)
                    })
                    .collect()
            })
            .unwrap_or_default();
        ctx.a11y = locals
            .a11y
            .as_deref()
            .map(|a11y| super::ctx::A11ySnap {
                subtitle_scale: a11y.subtitle_scale,
                shake: a11y.shake,
                hitstop: a11y.hitstop,
                interact_hold: a11y.interact_hold,
            })
            .unwrap_or_default();
        ctx.net_role = match locals.net.as_deref().map(|net| &net.role) {
            Some(crate::netplay::NetRole::Host { .. }) => "host",
            Some(crate::netplay::NetRole::Join { .. }) => "join",
            _ => "off",
        };
        // Forward do herói (modelo olha +Z) para `viber.player_forward`.
        ctx.player_forward = player_forward;
        // Pose da câmara de jogo (frame anterior — o snapshot é publicado
        // depois do rig de terceira pessoa, no `script_camera`).
        ctx.camera = locals
            .camera_snapshot
            .as_deref()
            .map(|snap| (snap.pos, snap.yaw_deg, snap.pitch_deg));
        ctx.cinematic = locals
            .cinematic
            .as_deref()
            .and_then(|c| c.playing().map(str::to_owned));
        ctx.menu_open = locals.menus.as_deref().is_some_and(|menus| menus.any());
        // Handle de leitura do terreno para `viber.ground_below` — dois
        // clones de Arc por frame; o terreno é imutável pós-bootstrap.
        ctx.terrain = terrain.as_deref().map(|rt| rt.reader());
        // Imutável DENTRO de uma cena; uma troca de cena insere um terreno novo (ou nenhum).
        match terrain.as_ref() {
            Some(rt) if locals.surfaces.is_none() || rt.is_added() => {
                *locals.surfaces = Some(std::sync::Arc::new(SurfaceRegistries {
                    roads: rt.roads.clone(),
                    water: rt.water.clone(),
                }));
            }
            Some(_) => {}
            None => *locals.surfaces = None,
        }
        let scene_id = locals.scene.as_deref().and_then(|s| s.id.clone());
        // Troca de cena: o `luau_update` pára no `Swap`/`Settle` (>2 frames) e
        // os `RemovedComponents<Name>` dos despawns expiram no double-buffer —
        // sem reseed completo os nomes da cena antiga ficavam PARA SEMPRE no
        // snapshot (`find`/`nearby`/`entity_position` devolviam entidades
        // mortas, com a posição de outra cena).
        if ctx.scene != scene_id {
            *locals.names_seeded = false;
        }
        ctx.scene = scene_id;
        ctx.scene_entry = locals.scene.as_deref().and_then(|s| s.entry.clone());
        ctx.biome = locals.biome.as_ref().and_then(|b| b.id.clone());
        if let Some(index) = locals.scene_index.as_ref()
            && (index.is_changed() || ctx.scene_ids.is_empty())
        {
            ctx.scene_ids = std::sync::Arc::new(index.ids.clone());
        }
        ctx.surfaces = locals.surfaces.clone();
        // Snapshots de nomes e HP para `viber.find`/`entity_hp` — início de
        // frame: entidades novas são vistas no frame seguinte (como tudo).
        // SEM cap: o `.take(4096)` antigo cortava por ordem arbitrária de
        // query e em mundos grandes (`simple-rpg`: ~79k entidades nomeadas)
        // `find`/`find_all`/`entity_position(id)` perdiam entidades — 10 dos
        // 20 portais do simple-rpg eram invisíveis aos scripts. Os vetores
        // por nome ficam ORDENADOS por bits de entidade: `find_all` devolve
        // ordem estável entre frames/máquinas (scripts que cacheiam registos
        // no arranque não podem ver ordens diferentes).
        {
            // INCREMENTAL: reconstruir 79k nomes por frame custava ~18 ms
            // (simple-rpg). Semeia 1× e depois só aplica o que mudou
            // (Name/GlobalTransform alterados, Names removidos/despawns) —
            // a vegetação e os props estáticos não custam nada por frame.
            if !*locals.names_seeded {
                *locals.names_seeded = true;
                locals.names_removed.clear();
                let mut named: HashMap<String, Vec<i64>> = HashMap::new();
                let mut positions: HashMap<i64, (String, Vec3)> = HashMap::new();
                for (e, name, transform) in locals.names.iter() {
                    let bits = e.to_bits() as i64;
                    named.entry(name.to_string()).or_default().push(bits);
                    positions.insert(bits, (name.to_string(), transform.translation()));
                }
                for bits in named.values_mut() {
                    bits.sort_unstable();
                }
                ctx.named_entities = named;
                ctx.pos_snapshot = positions;
            } else {
                let ctx = &mut *ctx;
                for e in locals.names_removed.read() {
                    forget_named(
                        &mut ctx.named_entities,
                        &mut ctx.pos_snapshot,
                        e.to_bits() as i64,
                    );
                }
                for (e, name, transform) in locals.names_changed.iter() {
                    let bits = e.to_bits() as i64;
                    let name = name.as_str();
                    let renamed = ctx
                        .pos_snapshot
                        .get(&bits)
                        .is_some_and(|(old, _)| old != name);
                    if renamed {
                        forget_named(&mut ctx.named_entities, &mut ctx.pos_snapshot, bits);
                    }
                    let list = ctx.named_entities.entry(name.to_owned()).or_default();
                    if let Err(at) = list.binary_search(&bits) {
                        list.insert(at, bits);
                    }
                    ctx.pos_snapshot
                        .insert(bits, (name.to_owned(), transform.translation()));
                }
            }
            let mut groups: HashMap<String, Vec<i64>> = HashMap::new();
            for (e, member_of) in locals.groups.iter() {
                for group in member_of.0.iter() {
                    groups
                        .entry(group.clone())
                        .or_default()
                        .push(e.to_bits() as i64);
                }
            }
            for bits in groups.values_mut() {
                bits.sort_unstable();
            }
            ctx.groups = groups;
            let mut hp: HashMap<i64, (f32, f32)> = HashMap::new();
            for (e, h, _) in locals.healths.iter() {
                hp.insert(e.to_bits() as i64, (h.current, h.max));
            }
            ctx.hp_snapshot = hp;
            ctx.hostiles.clear();
            for (e, script, _fsm, global) in locals.hostile_entities.iter() {
                let Some(&(cur, max)) = ctx.hp_snapshot.get(&(e.to_bits() as i64)) else {
                    continue;
                };
                if cur <= 0.0 {
                    continue;
                }
                let kind = script
                    .map(|script| crate::script_kind::script_kind(&script.path))
                    .unwrap_or_else(|| "creature".to_string());
                let rot = global.rotation();
                let forward = rot * Vec3::Z;
                ctx.hostiles.push(super::ctx::HostileSnap {
                    id: e.to_bits() as i64,
                    kind,
                    pos: global.translation(),
                    hp: cur,
                    max,
                    forward: Vec2::new(forward.x, forward.z),
                });
            }
            // A biblioteca é imutável pós-arranque: só re-semeia quando muda
            // (ou na 1.ª vez) — clonar todos os nomes em TODOS os frames era
            // lixo puro. `as_ref` e não `as_deref` para manter o `Res` (a
            // detecção de mudança vive nele).
            if let Some(lib) = locals.prototypes.as_ref()
                && (lib.is_changed() || ctx.prototype_names.is_empty())
            {
                ctx.prototype_names = lib.0.keys().cloned().collect();
            }
        }
    }

    // Fan-out de eventos engine→Lua: cada evento vira tabela Lua e entra na
    // fila de CADA script subscrito (quem chamou `viber.events()` pelo menos
    // uma vez — a presença da chave no registry É a subscrição). Corre ANTES
    // do loop de on_update: o script vê o evento no frame em que corre.
    let incoming = locals
        .events
        .as_mut()
        .map(|q| std::mem::take(&mut q.0))
        .unwrap_or_default();
    // Fan-out de eventos engine→Lua (pcall-style — erro de Lua num evento
    // não derruba o frame) e tick dos timers (`viber.after`/`every`), ambos
    // ANTES dos `on_update`: o script vê o evento/timer no próprio frame.
    fan_out_events(&host.lua, &incoming, &mut locals.events_dropped);
    super::timers::tick(
        &mut host,
        super::timers::TickFrame {
            elapsed,
            dt,
            player: player_pos,
            origin_of: Box::new(|owner| {
                scripts
                    .get(owner)
                    .ok()
                    .and_then(|(_, _, transform, global, _, _)| {
                        global
                            .map(GlobalTransform::translation)
                            .or_else(|| transform.map(|t| t.translation))
                    })
                    .or_else(|| {
                        // Dono sem `LuaScriptRef` (ex.: prop criado por
                        // `viber.spawn_prototype` cujo `on_spawned` armou um
                        // timer): vivo se tem `GlobalTransform`.
                        locals
                            .all_globals
                            .get(owner)
                            .ok()
                            .map(GlobalTransform::translation)
                    })
            }),
            name_of: Box::new(|owner| locals.entity_names.get(owner).ok().map(Name::to_string)),
            props_of: Box::new(|owner| {
                locals
                    .script_props
                    .get(owner)
                    .ok()
                    .map(|(_, props)| props.0.clone())
            }),
            range_of: Box::new(|owner| {
                scripts
                    .get(owner)
                    .ok()
                    .and_then(|(_, _, _, _, _, interaction)| interaction.map(|i| i.range))
            }),
        },
    );

    for (entity, lref, transform, global, activation, interaction) in &mut scripts {
        // Posição no MUNDO (o `Transform` sozinho é local ao grupo pai).
        let Some(origin) = global
            .map(GlobalTransform::translation)
            .or_else(|| transform.as_ref().map(|t| t.translation))
        else {
            continue;
        };
        // Congelamento (LOD de IA): além do raio de ativação o on_update nem
        // roda — inimigo distante custa zero lógica (e a animação para junto).
        let radius = activation
            .map(|a| a.radius)
            .unwrap_or(DEFAULT_ACTIVATION_RADIUS);
        if let Some(p) = player_pos
            && origin.distance(p) > radius
        {
            continue;
        }
        // Range de interação da entidade actual (para `viber.interacted`).
        if let Some(mut ctx) = host.lua.app_data_mut::<ScriptCtx>() {
            ctx.interaction_range = interaction.map(|i| i.range);
            // Nome PRÓPRIO da entidade que vai correr (fora do snapshot com
            // cap — ver `entity_names`), com o buffer reaproveitado.
            ctx.set_self_name(locals.entity_names.get(entity).ok());
            ctx.self_props = locals
                .script_props
                .get(entity)
                .ok()
                .map(|(_, p)| p.0.clone());
        }
        // Secção "scripts" do profiler: um escopo por ficheiro (igual ao
        // `script/<file>` antigo). record_script auto-gateia em freeze.
        let script_t0 = std::time::Instant::now();
        if let Err(e) = host.run_update(entity, &lref.path, dt, origin, player_pos, elapsed) {
            host.warn_once(&lref.path, &e);
        }
        let sensed = locals
            .sense
            .as_deref()
            .and_then(|inbox| inbox.pulses.get(&entity).copied());
        if let Some(kind) = sensed
            && let Ok(text) = host.lua.create_string(kind.as_str())
        {
            let args = mlua::MultiValue::from_vec(vec![mlua::Value::String(text)]);
            if let Err(error) = host.run_method(entity, &lref.path, origin, "on_sense", args, None)
            {
                host.warn_once(&lref.path, &error);
            }
        }
        crate::profiler::timed::record_script(
            &lref.path,
            script_t0.elapsed().as_secs_f32() * 1000.0,
        );
    }

    // Sinais (`viber.emit`) e `viber.call_group`: entregues DEPOIS dos
    // on_update, no mesmo frame, com o contexto do alvo. Handlers que emitem
    // abrem nova vaga; o que sobrar das vagas fica para o frame seguinte.
    // Alvos fora do raio de ativação recebem na mesma (um sinal é um evento —
    // a porta longe tem de abrir).
    for _ in 0..super::signals::MAX_SIGNAL_ROUNDS {
        let calls = super::signals::drain(&host.lua);
        if calls.is_empty() {
            break;
        }
        let mut deliveries: Vec<(Entity, String, mlua::Table, Option<i64>)> = Vec::new();
        if let Some(ctx) = host.lua.app_data_ref::<ScriptCtx>() {
            let members = |bits: Option<&Vec<i64>>| bits.cloned().unwrap_or_default();
            for call in calls {
                match &call.kind {
                    super::signals::CallKind::Emit { signal } => {
                        let Some(emitter) = call.from.and_then(|b| Entity::try_from_bits(b as u64))
                        else {
                            continue;
                        };
                        let Ok(connections) = locals.connections.get(emitter) else {
                            continue; // sinal sem ouvintes: como no Godot, nada acontece
                        };
                        for conn in connections.of(signal) {
                            use super::exports::ConnTarget;
                            let targets = match &conn.target {
                                ConnTarget::Name(name) if name == "self" => {
                                    vec![emitter.to_bits() as i64]
                                }
                                ConnTarget::Name(name) => members(ctx.named_entities.get(name)),
                                ConnTarget::Group(group) => members(ctx.groups.get(group)),
                            };
                            if targets.is_empty()
                                && locals
                                    .once_warned
                                    .insert(format!("signal-target:{signal}:{conn}"))
                            {
                                warn!(
                                    "[luau] sinal '{signal}': alvo '{}' sem entidades",
                                    conn.target
                                );
                            }
                            for bits in targets {
                                if let Some(target) = Entity::try_from_bits(bits as u64) {
                                    deliveries.push((
                                        target,
                                        conn.method.clone(),
                                        call.args.clone(),
                                        call.from,
                                    ));
                                }
                            }
                        }
                    }
                    super::signals::CallKind::Group { group, method } => {
                        for bits in members(ctx.groups.get(group)) {
                            if let Some(target) = Entity::try_from_bits(bits as u64) {
                                deliveries.push((
                                    target,
                                    method.clone(),
                                    call.args.clone(),
                                    call.from,
                                ));
                            }
                        }
                    }
                }
            }
        }
        for (target, method, args, from) in deliveries {
            let Ok((_, lref, transform, global, _, interaction)) = scripts.get(target) else {
                if locals
                    .once_warned
                    .insert(format!("signal-noscript:{target}:{method}"))
                {
                    warn!("[luau] {method}: o alvo {target} não tem script — ignorado");
                }
                continue;
            };
            // Empréstimo, não clone: o `path` do script é lido (e não
            // guardado) na entrega — um `String` novo por handler por vaga.
            let path: &str = &lref.path;
            let origin = global
                .map(GlobalTransform::translation)
                .or_else(|| transform.as_ref().map(|t| t.translation))
                .unwrap_or_default();
            if let Some(mut ctx) = host.lua.app_data_mut::<ScriptCtx>() {
                ctx.interaction_range = interaction.map(|i| i.range);
                ctx.self_name = locals.entity_names.get(target).ok().map(Name::to_string);
                ctx.self_props = locals
                    .script_props
                    .get(target)
                    .ok()
                    .map(|(_, p)| p.0.clone());
            }
            match host.run_method(
                target,
                path,
                origin,
                &method,
                super::signals::unpack(&args),
                from,
            ) {
                Ok(true) => {}
                Ok(false) => {
                    if locals
                        .once_warned
                        .insert(format!("signal-method:{path}:{method}"))
                    {
                        warn!(
                            "[luau] {path}: não define `function {method}(…)` (alvo de sinal/grupo)"
                        );
                    }
                }
                Err(e) => {
                    host.warn_once(path, &e);
                }
            }
        }
    }

    // Recolhe os comandos enfileirados por TODOS os scripts (as teclas já
    // foram semeadas antes do loop) e aplica-os com a EXTRAÇÃO partilhada
    // com a ponte BMS (`bms_bridge::apply_script_command` — os mesmos braços
    // 1:1, agora num recurso comum de avisos/sombra de HP). Os closures
    // correm por ordem no sync point do sistema: o mesmo instante em que os
    // `commands.*` dos braços antigos aplicavam.
    let mut queued: Vec<ScriptCommand> = Vec::new();
    if let Some(mut ctx) = host.lua.app_data_mut::<ScriptCtx>() {
        queued = std::mem::take(&mut ctx.commands);
    }
    // O mapa-sombra é do frame anterior — os Commands já aplicaram.
    if warn.is_none() {
        commands.init_resource::<super::bms_bridge::ScriptWarnState>();
    } else if let Some(warn) = warn.as_deref_mut() {
        warn.fresh_health.clear();
    }
    for command in queued {
        commands.queue(move |world: &mut World| {
            super::bms_bridge::apply_script_command(world, command);
        });
    }

    // Compat: `viber.set_position` legado (posição absoluta = MUNDO; o
    // Transform é local, por isso converte-se pelo pai).
    for (entity, pos) in host.take_pending() {
        if let Ok((_, _, Some(mut transform), global, _, _)) = scripts.get_mut(entity) {
            // `pos` é MUNDO; o `Transform` é local. Infere-se a pose do pai a
            // partir do par (`GlobalTransform`, `Transform`) da própria
            // entidade — a diferença de translações só valia para pais sem
            // rotação/escala. Usar o pai por `ChildOf` lia um `GlobalTransform`
            // ainda não propagado em apps headless (o teste do legado pedia
            // `local + 5` e recebia `5`).
            let local_affine = transform.compute_affine();
            let parent_world = global
                .map(|global| global.affine() * local_affine.inverse())
                .unwrap_or(bevy::math::Affine3A::IDENTITY);
            transform.translation = parent_world.inverse().transform_point3(pos);
        }
    }
}

/// Raio do aggro-chain (m): um golpe acorda aliados scriptados à volta do
/// alvo atingido. Genérico — a política (matilha, som, toast) é do script.
pub const ALERT_RADIUS_M: f32 = 15.0;

/// Aggro-chain (loop 6): ao acertar uma criatura, aliados scriptados a até
/// [`ALERT_RADIUS_M`] recebem `on_player_attack(px, pz)` — os scripts de
/// matilhas usam-no para passar a perseguir.
#[allow(clippy::type_complexity)]
pub fn aggro_alert_system(
    mut alerts: bevy::ecs::message::MessageReader<crate::feedback::AttackAlert>,
    mut host: ResMut<LuaScriptHost>,
    mut scripts: Query<
        (
            Entity,
            &LuaScriptRef,
            &GlobalTransform,
            Option<&Name>,
            Option<&crate::luau::ScriptProps>,
        ),
        Without<crate::player::Player>,
    >,
) {
    for alert in alerts.read() {
        let alert_pos = alert.position;
        for (entity, lref, transform, name, props) in &mut scripts {
            // só quem está perto DO ALVO ATINGIDO (não do player)
            // Early-out por distância quadrada: sqrt por entidade×alerta
            // não compra nada (a comparação é a mesma).
            if transform.translation().distance_squared(alert_pos)
                <= ALERT_RADIUS_M * ALERT_RADIUS_M
                && let Err(error) = host.run_player_attack_alert(
                    entity,
                    &lref.path,
                    transform.translation(),
                    alert.position,
                    name.map(Name::to_string),
                    props.map(|props| props.0.clone()),
                )
            {
                host.warn_once(&lref.path, &error);
            }
        }
    }
}

/// Hook `on_remove`: drop per-entity leftovers; the chunk stays cached in the
/// registry so a respawned entity reuses the script's existing globals.
pub fn luau_on_remove(
    mut host: ResMut<LuaScriptHost>,
    mut removed: RemovedComponents<LuaScriptRef>,
    mut loops: Option<ResMut<crate::music::ScriptSoundLoops>>,
) {
    for entity in removed.read() {
        host.deactivate(entity);
        // Loops de SFX do dono morrem com ele (motor de um veículo que
        // despawna continuava a rugir até à troca de cena).
        if let Some(loops) = loops.as_deref_mut() {
            loops
                .wanted
                .retain(|_, params| params.owner != Some(entity));
        }
    }
}

/// `viber.gesture` numa entidade sem `CharacterAnimator`. A engine não tem
/// como o tocar; um jogo com outro animador (o Vael com o
/// `jackdaw_animation_runtime`) observa este evento e toca o estado.
#[derive(Event, Debug, Clone)]
pub struct UnboundGesture {
    pub entity: Entity,
    pub name: String,
    pub speed: f32,
}

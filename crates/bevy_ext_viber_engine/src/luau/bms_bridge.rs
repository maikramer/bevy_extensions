//! Ponte entre o host BMS ([`bevy_ext_luau`]) e o monólito.
//!
//! Duas metades, espelhos uma da outra:
//!
//! - **comandos**: [`apply_bms_commands`] (Update, depois do dreno do BMS)
//!   converte os `ScriptCommand` do `bevy_ext_luau` para os do monólito e
//!   aplica-os com [`apply_script_command`] — o MESMO caminho que o
//!   `luau_update` usa para os seus próprios comandos;
//! - **estado**: [`publish_host_state`] (PreUpdate, antes do publisher do
//!   BMS) preenche o [`bevy_ext_luau::ScriptHostState`] com o snapshot do
//!   frame extraído do `luau_update` (posições/nomes, HP, props, hostis,
//!   prototypes, superfícies, câmara…).
//!
//! O [`apply_script_command`] é a EXTRAÇÃO dos braços que viviam inline no
//! match de `commands.queue` do `luau_update` (behavior 1:1): os dois
//! consumidores partilham os mesmos recursos/writers (toasts, SFX,
//! `ProjectileQueue`, `TerrainEditQueue`, `UiPrompt`, marcadores do HUD,
//! `SaveRequest`…), e o mapa-sombra de HP criado por `Commands` + os avisos
//! 1× vivem agora no [`ScriptWarnState`] (recurso, partilhado pelos dois
//! caminhos).

use std::collections::{HashMap, HashSet};

use bevy::animation::graph::AnimationNodeIndex;
use bevy::animation::transition::AnimationTransitions;
use bevy::animation::AnimationPlayer;
use bevy::prelude::*;

use crate::player::Player;
use crate::vitals::Health;

use super::commands::ScriptCommand;
use super::components::{LuaScriptRef, ScriptInteraction, ScriptProps, ScriptToast};
use super::ctx::ScriptCtx;
use super::host::LuaScriptHost;
use super::input::key_code_from_str;
use super::sfx::match_gesture_clip;

/// Estado por-frame partilhado pelo `luau_update` e pela ponte BMS:
/// - `once`: avisos 1× (plugin ausente, fila cheia, entidade sem vitais,
///   gesto sem clip…) — as chaves são as mesmas que os `Local`s antigos do
///   `luau_update` usavam (com prefixo para as keyed por entidade);
/// - `fresh_health`: sombra do HP criado por `Commands` NESTE frame — os
///   comandos seguintes do MESMO dreno (set_max_hp seguido de damage) leem
///   daqui; o `luau_update` limpa-o ao início de cada dreno (era o
///   `locals.fresh_health.clear()`).
#[derive(Resource, Default)]
pub struct ScriptWarnState {
    pub(crate) once: HashSet<String>,
    pub(crate) fresh_health: HashMap<Entity, (f32, f32)>,
}

/// Acesso ao estado de aviso/sombra, criando-o se faltar (apps mínimas que
/// montam `apply_script_command` sem nenhum plugin).
fn warn_state(world: &mut World) -> Mut<'_, ScriptWarnState> {
    if !world.contains_resource::<ScriptWarnState>() {
        world.init_resource::<ScriptWarnState>();
    }
    world.resource_mut::<ScriptWarnState>()
}

/// Aviso 1× com chave arbitrária (o formato das chaves é interno).
fn warn_once(world: &mut World, key: &str, message: &str) {
    let fresh = warn_state(world).once.insert(key.to_string());
    if fresh {
        warn!(target: "viber::luau", "{message}");
    }
}

/// A entidade do herói (o `players.single()` do `luau_update`).
fn player_of(world: &mut World) -> Option<Entity> {
    let mut query = world.query_filtered::<Entity, With<Player>>();
    query.iter(world).next()
}

/// Posição de MUNDO de uma entidade de script — o mesmo par de leituras do
/// `scripts` do `luau_update` (`GlobalTransform` preferido ao `Transform`
/// local; ver o comentário do repro da cidade). Exclui o herói, como a
/// query `Without<Player>` fazia: `move_towards`/`face_towards` no herói
/// continuam a ser ignorados pela engine. DIFERENÇA assumida: o `scripts`
/// do `luau_update` exigia `LuaScriptRef` — os comandos da ponte BMS
/// podem mirar entidades sem script (attach BMS não usa o componente),
/// e o comportamento dos comandos do monólito não muda (enfileirados só
/// por entidades com script).
fn scripted_pos(world: &World, entity: Entity) -> Option<Vec3> {
    if world.get::<Player>(entity).is_some() {
        return None;
    }
    world
        .get::<GlobalTransform>(entity)
        .map(|global| global.translation())
        .or_else(|| world.get::<Transform>(entity).map(|t| t.translation))
}

/// Locomoção de script: `drive_to`/`drive`/`look_at` na entidade, criando o
/// `AiLocomotion` + `LocomotionProfile` quando faltam (mesma regra do braço
/// do `MoveBy`: o perfil aprende a velocidade comandada no mesmo gesto).
enum Drive {
    /// Marcha para um ponto (`MoveTowards`): (posição actual, alvo, velocidade).
    To(Vec2, Vec2, f32),
    /// Velocidade desejada (`MoveBy`).
    By(Vec2),
    /// Pedido de olhar (`FaceTowards`).
    Look(Vec2),
}

fn drive_locomotion(world: &mut World, entity: Entity, drive: Drive, dt: f32) {
    // Velocidade a que o perfil de locomoção aprende (`Look` não observa:
    // o braço do `FaceTowards` também não tocava no perfil).
    let observed = match drive {
        Drive::To(_, _, speed) => Some(speed),
        Drive::By(velocity) => Some(velocity.length()),
        Drive::Look(_) => None,
    };
    let has_loco = world.get::<crate::ai::AiLocomotion>(entity).is_some();
    if has_loco {
        if let Some(mut loco) = world.get_mut::<crate::ai::AiLocomotion>(entity) {
            match drive {
                Drive::To(here, goal, speed) => loco.drive_to(here, goal, speed, dt),
                Drive::By(velocity) => loco.drive(velocity),
                Drive::Look(dir) => loco.look_at(dir),
            }
        }
        if let Some(speed) = observed
            && let Some(mut profile) =
                world.get_mut::<crate::animation::LocomotionProfile>(entity)
        {
            profile.observe(speed);
        }
    } else {
        let mut loco = crate::ai::AiLocomotion::default();
        match drive {
            Drive::To(here, goal, speed) => loco.drive_to(here, goal, speed, dt),
            Drive::By(velocity) => loco.drive(velocity),
            Drive::Look(dir) => loco.look_at(dir),
        }
        match observed {
            Some(speed) => {
                let mut profile = crate::animation::LocomotionProfile::default();
                profile.observe(speed);
                world.commands().entity(entity).try_insert((loco, profile));
            }
            None => {
                world.commands().entity(entity).try_insert(loco);
            }
        }
    }
}

/// One-shot com playback speed sobre `&mut World` — corpo 1:1 do
/// `bevy_ext_engine::animation::play_action_scaled` (o helper espera um
/// `Query`, inacessível fora de sistemas). `duration_of` vem pré-lido para
/// não reabrir o componente depois do borrow do player.
fn play_action_scaled_world(
    world: &mut World,
    entity: Entity,
    player_entity: Entity,
    locked: bool,
    duration: f32,
    node: AnimationNodeIndex,
    blend: std::time::Duration,
    terminal: bool,
    speed: f32,
) -> bool {
    if locked {
        return false;
    }
    let speed = if speed.is_finite() && speed > 0.05 {
        speed
    } else {
        1.0
    };
    {
        let mut players = world.query::<(
            &mut AnimationPlayer,
            &mut AnimationTransitions,
        )>();
        let Ok((mut player, mut transitions)) = players.get_mut(world, player_entity)
        else {
            return false;
        };
        transitions
            .play(&mut player, node, blend)
            .set_repeat(bevy::animation::RepeatAnimation::Never)
            .set_speed(speed);
    }
    if let Some(mut animator) = world.get_mut::<crate::animation::CharacterAnimator>(
        entity,
    ) {
        animator.current = Some(node);
        animator.locked = terminal;
        animator.action_time = if terminal {
            f32::INFINITY
        } else {
            // A clip com duração não mensurável trancava o rig para sempre;
            // cai num beat curto para o driver recuperar o rig sempre.
            let d = if duration > 1e-3 { duration } else { 0.25 };
            d / speed
        };
    }
    true
}

/// Destrutível com queda (`Topple`): física genérica; a direção sai do
/// herói ao alvo. Corpo 1:1 do closure que o braço do `Topple` enfileirava —
/// com `&mut World` direto já não precisa de comando diferido.
fn topple_entity(world: &mut World, entity: Entity, player_pos: Option<Vec3>) {
    let Some(player) = player_pos else {
        return;
    };
    let Some(global) = world.get::<GlobalTransform>(entity) else {
        return;
    };
    let target = global.translation();
    let dir = (target - player).normalize_or_zero();
    let initial = world
        .get::<Transform>(entity)
        .map(|t| t.rotation)
        .unwrap_or_default();
    let Ok(mut prop) = world.get_entity_mut(entity) else {
        return;
    };
    prop.insert(crate::physics_fx::Falling {
        axis: Vec3::new(dir.z, 0.0, -dir.x),
        timer: 0.0,
        initial,
    });
    prop.remove::<LuaScriptRef>();
}

/// Cura: um `amount` negativo não tira vida (isso é `entity_damage`) e o
/// resultado nunca sai de `[0, max]`. Sem o chão, `current + (-grande)`
/// ficava negativo e a entidade deixava de poder morrer.
fn apply_heal(current: f32, max: f32, amount: f32) -> f32 {
    (current + amount.max(0.0)).clamp(0.0, max.max(0.0))
}

/// Fallback de `viber.set_visible` quando a reflexão não tem `Visibility`
/// (app mínima de teste sem `ReflectPlugin`): o comportamento antigo de
/// inserir o componente. `Inherited` (e não `Visible`) para a entidade voltar
/// a respeitar o pai escondido quando é uma subárvore.
fn insert_visibility(world: &mut World, entity: Entity, visible: bool) {
    if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
        entity_mut.insert(if visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
}

/// Balão do HUD (`viber.say` sem presenter externo) sobre `&mut World` —
/// corpo 1:1 do `crate::dialogue::show_balloon` (o helper espera `Query`s),
/// com as escritas de texto faseadas para respeitar os borrows.
fn show_balloon_world(world: &mut World, body: &str, secs: f32) -> bool {
    let mut found = false;
    let mut children: Vec<Entity> = Vec::new();
    {
        let mut balloons = world.query::<(
            &mut Visibility,
            &mut crate::hud::HudBalloon,
            &bevy::ecs::hierarchy::Children,
        )>();
        for (mut visibility, mut balloon, kids) in balloons.iter_mut(world) {
            found = true;
            balloon.timer = secs;
            *visibility = Visibility::Visible;
            if let Some(child) = kids.first() {
                children.push(*child);
            }
        }
    }
    let mut texts = world.query::<&mut Text>();
    for child in children {
        if let Ok(mut text) = texts.get_mut(world, child) {
            text.0 = body.into();
        }
    }
    found
}

/// Aplica UM comando enfileirado por script, com `&mut World`.
///
/// EXTRAÇÃO 1:1 dos braços que viviam inline no match de commands.queue do
/// `luau_update` (ver git): os consumidores são o próprio `luau_update` (via
/// `commands.queue`) e a ponte BMS ([`apply_bms_commands`]). Os avisos 1× e
/// o mapa-sombra de HP vivem em [`ScriptWarnState`] no World.
pub(crate) fn apply_script_command(world: &mut World, command: ScriptCommand) {
    let dt = world.resource::<Time>().delta_secs();
    let elapsed = world.resource::<Time>().elapsed_secs_f64();
    match command {
        ScriptCommand::MoveTowards { entity, goal, speed } => {
            // A posição actual vem do MUNDO (o alvo é mundo); sem ela não
            // há direcção nem chegada.
            let Some(here) = scripted_pos(world, entity) else {
                return;
            };
            let here = Vec2::new(here.x, here.z);
            drive_locomotion(world, entity, Drive::To(here, goal, speed), dt);
        }
        ScriptCommand::MoveBy(entity, velocity) => {
            // A velocidade desejada vai para o [`crate::ai::AiLocomotion`];
            // o passo, a rampa de aceleração, o snap de Y e a viragem são
            // do consumidor único (`apply_ai_locomotion`, PostUpdate).
            // Entidades ainda sem o componente recebem-no aqui — o perfil
            // de locomoção aprende a velocidade comandada no mesmo gesto,
            // que é o que faz os 17 scripts do exemplo acertarem o clip e
            // a cadência sem uma linha de autoria.
            drive_locomotion(world, entity, Drive::By(velocity), dt);
        }
        ScriptCommand::FaceTowards(entity, target) => {
            // Direção no MUNDO (o alvo é o herói, em mundo).
            let Some(here) = scripted_pos(world, entity) else {
                return;
            };
            let dir = Vec2::new(target.x - here.x, target.z - here.z);
            // Pedido de olhar, não ordem: `apply_ai_locomotion` só o honra
            // com a entidade praticamente parada. A andar, o corpo aponta
            // para onde VAI — é esta regra que mata o "andar de lado" dos
            // scripts que chamam `face_player()` e `move_towards()` no
            // mesmo frame.
            drive_locomotion(world, entity, Drive::Look(dir), dt);
        }
        ScriptCommand::SetLocomotion { entity, walk, run, turn_rate } => {
            let mut profile = crate::animation::LocomotionProfile::authored(walk, run);
            if let Some(turn_rate) = turn_rate {
                profile.turn_rate = turn_rate;
            }
            let has_loco = world.get::<crate::ai::AiLocomotion>(entity).is_some();
            let has_profile = world.get::<crate::animation::LocomotionProfile>(entity).is_some();
            if has_loco && has_profile {
                if let Some(mut existing) = world.get_mut::<crate::animation::LocomotionProfile>(entity) {
                    *existing = profile;
                }
            } else if has_loco {
                world
                    .commands()
                    .entity(entity)
                    .try_insert(profile);
            } else {
                world
                    .commands()
                    .entity(entity)
                    .try_insert((crate::ai::AiLocomotion::default(), profile));
            }
        }
        ScriptCommand::ChangeScene { scene, entry, via } => {
            let request = crate::scene_loader::ChangeScene {
                scene,
                arrival: crate::scene_loader::Arrival::from_entry(entry.as_deref()),
                via_portal: via,
                toast: None,
                sound: None,
                came_from: None,
                return_pose: None,
            };
            if let Err(error) = crate::scene_loader::request_change(world, request) {
                warn!(target: "viber::luau", "viber.change_scene: {error:#}");
            }
        }
        ScriptCommand::TeleportPlayer { pos, exclude } => {
            let Some(entity) = player_of(world) else {
                return;
            };
            // ASSENTA no destino em vez de largar o herói no ar: as
            // colunas do destino podem estar por assar (vinha de uma
            // bolsa de interior a 2,6 km) e, sem collider, o herói
            // caía pelo mundo fora antes de o streaming o apanhar.
            //
            // O Y vem de um RAIO DE FÍSICA (props/plataformas/pontes)
            // com o terreno analítico como plano B — o braço original
            // diferia num comando porque os SystemParams não expõem o
            // `&mut World` do Rapier; esta extração tem-no.
            //
            // Exclui o herói E o prop pedido (marco): o raio não
            // pode pousá-lo por cima do próprio destino.
            let mut skip = vec![entity];
            if let Some(prop) = exclude {
                skip.push(prop);
            }
            let landed = crate::player::teleport_landing(world, pos, &skip);
            if let Some(mut transform) = world.get_mut::<Transform>(entity) {
                transform.translation = landed;
            }
            if let Some(mut player) = world.get_mut::<Player>(entity) {
                player.vel_x = 0.0;
                player.vel_y = 0.0;
                player.vel_z = 0.0;
            }
            if let Ok(mut hero) = world.get_entity_mut(entity) {
                hero.remove::<crate::physics_fx::Knockback>();
                hero.insert(crate::player::TeleportSettle::default());
            }
        }
        ScriptCommand::PushPlayer { velocity } => {
            let Some(entity) = player_of(world) else {
                return;
            };
            world
                .commands()
                .entity(entity)
                .try_insert(crate::physics_fx::Knockback { velocity });
        }
        ScriptCommand::FireProjectile { template, origin, target, shooter } => {
            let Some(mut queue) = world.get_resource_mut::<crate::projectile::ProjectileQueue>()
            else {
                warn_once(
                    world,
                    "fire_projectile",
                    "viber.fire_projectile sem ProjectilePlugin — pedido ignorado",
                );
                return;
            };
            queue.requests.push(crate::projectile::ProjectileRequest {
                template,
                origin,
                target,
                shooter,
            });
        }
        ScriptCommand::HealPlayer(amount) => {
            let Some(entity) = player_of(world) else {
                return;
            };
            if let Some(mut health) = world.get_mut::<Health>(entity) {
                // O mesmo contrato de `entity_heal`: negativo não tira vida
                // e um NaN do produtor não envenena o HP para sempre.
                health.current = apply_heal(health.current, health.max, amount);
            }
        }
        ScriptCommand::Toast(msg) => {
            let msg = world
                .get_resource::<crate::locale::Locale>()
                .map(|locale| crate::dialogue::localized_lines(&msg, Some(&locale.table)))
                .unwrap_or(msg);
            info!(target: "viber::luau", "[toast] {msg}");
            world.write_message(ScriptToast(msg));
        }
        ScriptCommand::SetInteraction { entity, label, key, range } => {
            let Some(code) = key_code_from_str(&key) else {
                return;
            };
            // Label vazio = sem interação (o script quer esconder o
            // prompt). Sem o remover a entidade continuava a GANHAR o
            // foco da tecla (distância 0, sem prompt nenhum) e calava
            // o NPC real ao lado.
            if label.trim().is_empty() {
                world.commands().entity(entity).try_remove::<ScriptInteraction>();
            } else {
                world.commands().entity(entity).try_insert(ScriptInteraction {
                    label,
                    key: code,
                    // ÚNICO sítio onde o alcance autorado vira alcance
                    // efetivo: daqui para a frente (prompt, foco,
                    // `interacted`, colheita) todos leem o mesmo número.
                    range: crate::interact::scaled_range(range),
                });
            }
        }
        ScriptCommand::Despawn(entity) => {
            world.commands().entity(entity).try_despawn();
        }
        ScriptCommand::Gesture { entity, name, speed, lock, blend } => {
            // Leitura prévia do rig (o borrow do `Mut` não pode coexistir
            // com o do `AnimationPlayer`).
            let prep = world.get::<crate::animation::CharacterAnimator>(entity).map(
                |animator| {
                    (
                        animator.player,
                        animator.locked,
                        match_gesture_clip(&animator, &name)
                            .and_then(|index| animator.nodes.get(index).copied()),
                    )
                },
            );
            let Some((player_entity, locked, node)) = prep else {
                // Sem rig da engine: o jogo pode ter outro (estados de
                // animação autorados no Jackdaw) — fica com o pedido.
                world.commands().trigger(super::runtime::UnboundGesture {
                    entity,
                    name: name.clone(),
                    speed,
                });
                // Rig ainda a carregar: o gesto espera por ele (e é
                // largado em silêncio se não chegar — sem aviso por
                // NPC a cada arranque).
                world.commands().entity(entity).try_insert(
                    crate::animation::PendingGesture {
                        name,
                        speed,
                        lock,
                        blend,
                        age: 0.0,
                    },
                );
                return;
            };
            let Some(node) = node else {
                let clip_names = world
                    .get::<crate::animation::CharacterAnimator>(entity)
                    .map(|animator| animator.clip_names.clone());
                if let Some(clip_names) = clip_names
                    && warn_state(world)
                        .once
                        .insert(format!("gesture:{}", entity.to_bits()))
                {
                    warn!(target: "viber::luau",
                        "viber.gesture('{name}'): nenhum clip do rig corresponde (clips: {clip_names:?})");
                }
                return;
            };
            if locked {
                return;
            }
            let duration = world
                .get::<crate::animation::CharacterAnimator>(entity)
                .map(|animator| animator.duration_of(node))
                .unwrap_or_default();
            // Blend de gesto ~250 ms como o npc_gesture_system; one-shot
            // devolve o rig ao driver no fim do clip. `speed` escala a
            // reprodução (`viber.play_clip`; 1.0 = nominal).
            let blend = blend
                .map(std::time::Duration::from_secs_f32)
                .unwrap_or(std::time::Duration::from_millis(250));
            play_action_scaled_world(
                world,
                entity,
                player_entity,
                false,
                duration,
                node,
                blend,
                lock,
                speed,
            );
        }
        ScriptCommand::TerrainEdit { edit } => {
            let Some(mut queue) = world.get_resource_mut::<crate::terrain::delta::TerrainEditQueue>()
            else {
                // Apps mínimas sem o plugin de terreno: warn 1× (a fila
                // nem existe — o pedido não pode ser aceite em silêncio).
                warn_once(
                    world,
                    "terrain_edit",
                    "viber.terrain.* sem TerrainEditQueue registada — pedido ignorado",
                );
                return;
            };
            if !queue.push(edit) {
                warn_once(
                    world,
                    "terrain_edit_full",
                    &format!(
                        "viber.terrain.*: fila de edições cheia ({} pendentes) — pedidos \
                         descartados até a fila escoar",
                        crate::terrain::delta::EDIT_QUEUE_CAP
                    ),
                );
            }
        }
        ScriptCommand::PlaySfx { clip, position } => {
            world.write_message(crate::ambient::SfxEvent { clip, position });
        }
        ScriptCommand::Topple { entity } => {
            let player_pos = player_of(world).and_then(|p| {
                world.get::<GlobalTransform>(p).map(|g| g.translation())
            });
            topple_entity(world, entity, player_pos);
        }
        ScriptCommand::PlayCinematic { id, anchor } => {
            world.write_message(crate::cinematic::PlayCinematic {
                id,
                anchor,
                inline: None,
            });
        }
        ScriptCommand::SetScale { entity, scale } => {
            // TODO(migration): o nome Lua mantém-se; a escrita passa pela
            // reflexão (`viber.ecs`), que já não depende de a entidade
            // estar na query `scripts`/`poses` — funciona em props, NPCs
            // sem script e no próprio herói.
            let patch = super::ecs::EcsValue::Map(vec![(
                "scale".to_string(),
                super::ecs::EcsValue::Vec3([scale, scale, scale]),
            )]);
            if let Err(error) =
                super::ecs::patch_component(world, entity, super::ecs::TRANSFORM_PATH, &patch)
            {
                // Sem `ReflectPlugin`/`Transform` no registo (app
                // mínima): mantém a escrita directa de sempre.
                warn!(target: "viber::luau", "viber.set_scale: {error}");
                if let Some(mut transform) = world.get_mut::<Transform>(entity) {
                    transform.scale = Vec3::splat(scale);
                }
            }
        }
        ScriptCommand::EntitySetMaxHp { entity, max } => {
            // O herói NÃO entra no dreno de `entity_*` (query
            // `Without<Player>`): o ramo de criação substituía o `Health`
            // inteiro e um level-up curava-o em vez de só subir o teto.
            if let Some(mut health) = world.get_mut::<Health>(entity) {
                health.max = max;
                health.current = health.current.min(max);
            } else {
                // Criação: também grava no mapa-sombra (comandos do
                // mesmo frame não vêem a query).
                world
                    .commands()
                    .entity(entity)
                    .try_insert(Health { current: max, max });
                warn_state(world).fresh_health.insert(entity, (max, max));
            }
        }
        ScriptCommand::EntityDamage { entity, amount } => {
            // Helper local: emitir Kill na TRANSIÇÃO vivo→morto. A ENGINE
            // NÃO decide o que a morte significa: sem cadáver, sem XP, sem
            // remover o script — o jogo ouve o evento e trata da morte, do
            // loot e das quests. Bater num cadáver não mata outra vez.
            // O nome segue o contrato do melee (`script_kind` do path); o
            // herói (fora da query do monólito) fica "creature".
            let kind = if world.get::<Player>(entity).is_some() {
                "creature".to_string()
            } else {
                world
                    .get::<LuaScriptRef>(entity)
                    .map(|lref| crate::script_kind::script_kind(&lref.path))
                    .unwrap_or_else(|| "creature".to_string())
            };
            let player_hit = player_of(world) == Some(entity);
            let mut killed: Option<i64> = None;
            let mut damaged = false;
            if let Some(mut health) = world.get_mut::<Health>(entity) {
                let before = health.current;
                crate::vitals::apply_damage(&mut health, amount);
                if before > 0.0 && health.current <= 0.0 && !player_hit {
                    killed = Some(entity.to_bits() as i64);
                }
                damaged = true;
            }
            if !damaged {
                // Sem `Health` no mundo: a sombra do HP criado por `Commands`
                // neste frame (o padrão set_max_hp → damage do MESMO dreno).
                let next = {
                    let mut warn = warn_state(world);
                    warn.fresh_health.get_mut(&entity).map(|(cur, max)| {
                        let before = *cur;
                        let next = (*cur - amount).clamp(0.0, (*max).max(0.0));
                        *cur = next;
                        if before > 0.0 && next <= 0.0 && !player_hit {
                            killed = Some(entity.to_bits() as i64);
                        }
                        (next, *max)
                    })
                };
                if let Some((next, max)) = next {
                    world
                        .commands()
                        .entity(entity)
                        .try_insert(Health { current: next, max });
                } else {
                    let bits = entity.to_bits();
                    let fresh = warn_state(world).once.insert(format!("vitals:{bits}"));
                    if fresh {
                        warn!(target: "viber::luau",
                            "viber.entity_damage: entidade sem Health — chama viber.entity_set_max_hp primeiro");
                    }
                }
            }
            if let Some(bits) = killed {
                if let Some(mut events) = world.get_resource_mut::<super::events::ScriptEventQueue>()
                {
                    events.push(super::events::ScriptGameEvent::Kill { name: kind, entity: bits });
                }
            }
        }
        ScriptCommand::EntityHeal { entity, amount } => {
            if let Some(mut health) = world.get_mut::<Health>(entity) {
                health.current = apply_heal(health.current, health.max, amount);
            } else {
                let healed = {
                    let mut warn = warn_state(world);
                    warn.fresh_health.get_mut(&entity).map(|(cur, max)| {
                        let next = apply_heal(*cur, *max, amount);
                        *cur = next;
                        (next, *max)
                    })
                };
                if let Some((next, max)) = healed {
                    world
                        .commands()
                        .entity(entity)
                        .try_insert(Health { current: next, max });
                }
            }
        }
        ScriptCommand::EntityKnockback { entity, dir, strength } => {
            if strength > 0.0 && dir.length_squared() > 1e-6 {
                world
                    .commands()
                    .entity(entity)
                    .try_insert(crate::physics_fx::knockback_after(dir, strength));
            }
        }
        ScriptCommand::HitFlash { entity, secs } => {
            world
                .commands()
                .entity(entity)
                .try_insert(crate::feedback::HitFlash { timer: secs });
        }
        ScriptCommand::Prompt { key, label } => {
            if let Some(mut prompt) = world.get_resource_mut::<crate::ui::UiPrompt>() {
                if label.trim().is_empty() {
                    prompt.clear();
                } else {
                    prompt.set_script(key, label);
                }
            }
        }
        ScriptCommand::SetPlayerSpeed(mult) => {
            let mut query = world.query_filtered::<&mut crate::player::Player, With<Player>>();
            for mut player in query.iter_mut(world) {
                player.speed = crate::player::BASE_SPEED * mult;
            }
        }
        ScriptCommand::SetLantern(intensity) => {
            if let Some(mut light) = world.get_resource_mut::<crate::ambient::TorchLightOverride>()
            {
                light.intensity = if intensity.is_finite() {
                    intensity.max(0.0)
                } else {
                    0.0
                };
            }
        }
        ScriptCommand::SetBoneOffset { bone, rotation } => {
            if let Some(mut offsets) = world.get_resource_mut::<crate::ik::PlayerBoneOffsets>() {
                if rotation == Quat::IDENTITY {
                    offsets.clear(&bone);
                } else {
                    offsets.set(&bone, rotation);
                }
            }
        }
        ScriptCommand::Corpse { entity } => {
            // Só a marca: quem tem `RagdollDeath` (attr `ragdoll`) entra
            // em ragdoll; o despawn e o loot são do jogo.
            world.commands().entity(entity).try_insert(crate::corpse::Corpse {
                timer: crate::corpse::CORPSE_LIFETIME,
            });
        }
        ScriptCommand::Attach { owner, slot, url, bone, pos, rot, scale } => {
            let Some(mut queue) = world.get_resource_mut::<crate::attachment::AttachmentRequests>()
            else {
                warn_once(
                    world,
                    "attach",
                    "viber.attach sem AttachmentPlugin — pedido ignorado",
                );
                return;
            };
            queue.0.push(crate::attachment::AttachmentRequest {
                owner,
                slot,
                url,
                bone,
                pos,
                rot,
                scale,
            });
        }
        ScriptCommand::A11ySet { subtitle_scale, shake, hitstop, interact_hold } => {
            if let Some(mut a11y) = world.get_resource_mut::<crate::a11y::A11y>() {
                if let Some(scale) = subtitle_scale {
                    a11y.subtitle_scale = scale.clamp(0.5, 3.0);
                }
                if let Some(value) = shake {
                    a11y.shake = value;
                }
                if let Some(value) = hitstop {
                    a11y.hitstop = value;
                }
                if let Some(value) = interact_hold {
                    a11y.interact_hold = value;
                }
            }
        }
        ScriptCommand::OwnUiAction(name) => {
            if let Some(mut owners) = world.get_resource_mut::<crate::ui::actions::UiActionOwners>()
            {
                owners.0.insert(name);
            }
        }
        ScriptCommand::RadialDamage { x, z, radius, damage, knockback } => {
            // Falloff linear (skills::radial_damage) + knockback radial
            // (physics_fx) + evento Kill. Cadáver, XP e quests são do
            // JOGO: a engine só aplica o dano e conta a morte.
            apply_radial_damage(world, x, z, radius, damage, knockback, None);
        }
        ScriptCommand::Burst { preset, pos, count } => {
            if preset == "sparks" {
                let spec = crate::fx_specs::hit_sparks_spec();
                let mut commands = world.commands();
                crate::particles::spawn_burst(&mut commands, &spec, pos, count.clamp(1, 256));
            } else {
                let spec = crate::recipes::ParticleSpec {
                    preset,
                    emission_rate: None,
                    life: None,
                    speed: None,
                    size: None,
                    color: None,
                    shape_radius: None,
                    looping: false,
                    world_space: false,
                };
                let mut commands = world.commands();
                crate::particles::spawn_burst(&mut commands, &spec, pos, count.clamp(1, 256));
            }
        }
        ScriptCommand::Ring { x, z, radius, color } => {
            let y = world
                .get_resource::<crate::terrain::runtime::TerrainRuntime>()
                .map(|rt| rt.sample_mesh_surface(x, z))
                .unwrap_or(0.0);
            let color = color
                .map(|[r, g, b]| Color::srgb(r, g, b))
                .unwrap_or(Color::srgba(0.85, 0.9, 1.0, 0.9));
            if !world.contains_resource::<Assets<Mesh>>()
                || !world.contains_resource::<Assets<StandardMaterial>>()
            {
                return;
            }
            // O helper original espera `Commands` + dois `ResMut`
            // simultâneos — o `resource_scope` dá o mesmo trio a partir de
            // `&mut World` sem duplicar o corpo.
            world.resource_scope::<Assets<Mesh>, _>(|world, mut meshes| {
                world.resource_scope::<Assets<StandardMaterial>, _>(
                    |world, mut materials| {
                        let mut commands = world.commands();
                        crate::impact::spawn_impact_ring(
                            &mut commands,
                            &mut meshes,
                            &mut materials,
                            Vec3::new(x, y + 0.08, z),
                            radius,
                            color,
                        );
                    },
                );
            });
        }
        ScriptCommand::Shake(amount) => {
            if let Some(mut shake) = world.get_resource_mut::<crate::camera::CameraShake>() {
                crate::camera::add_camera_shake(&mut shake, amount);
            }
        }
        ScriptCommand::CameraKick(impulse) => {
            if let Some(mut kick) = world.get_resource_mut::<crate::camera::CameraKick>() {
                crate::camera::add_camera_kick(&mut kick, impulse);
            }
        }
        ScriptCommand::FovKick(deg) => {
            if let Some(mut fx) = world.get_resource_mut::<crate::camera::CameraFx>() {
                crate::camera::fov_kick(&mut fx, deg);
            }
        }
        ScriptCommand::Punch { stops, bloom } => {
            if let Some(mut postfx) = world.get_resource_mut::<crate::postfx::PostFxState>() {
                crate::postfx::punch_impact(&mut postfx, stops, bloom);
            }
        }
        ScriptCommand::HitStop(secs) => {
            if let Some(mut stop) = world.get_resource_mut::<crate::time_fx::HitStop>() {
                crate::time_fx::request_hit_stop(&mut stop, secs);
            }
        }
        ScriptCommand::DamageNumber { text, pos, color } => {
            world.write_message(crate::feedback::DamageNumberEvent {
                position: pos,
                text,
                color: color
                    .map(|[r, g, b]| Color::srgb(r, g, b))
                    .unwrap_or(Color::srgb(1.0, 0.9, 0.4)),
            });
        }
        ScriptCommand::OwnSystem(name) => {
            if let Some(mut owners) = world.get_resource_mut::<super::ownership::ScriptSystemOwners>()
            {
                owners.0.insert(name);
            }
        }
        ScriptCommand::ReleaseSystem(name) => {
            if let Some(mut owners) = world.get_resource_mut::<super::ownership::ScriptSystemOwners>()
            {
                owners.0.remove(&name);
            }
        }
        ScriptCommand::CameraFree(active) => {
            if let Some(mut state) = world.get_resource_mut::<crate::camera::ScriptCameraFree>() {
                state.active = active;
            }
        }
        ScriptCommand::CameraPose { pos, look } => {
            if let Some(mut state) = world.get_resource_mut::<crate::camera::ScriptCameraFree>() {
                state.pos = pos;
                state.look = look;
            }
        }
        ScriptCommand::SetVisible { entity, visible } => {
            // TODO(migration): shim Lua mantido; o caminho principal é a
            // reflexão (`Visibility` é um enum: a variante é o patch).
            let variant = if visible { "Inherited" } else { "Hidden" };
            let patch = super::ecs::EcsValue::Enum(variant.to_string());
            match super::ecs::patch_component(world, entity, super::ecs::VISIBILITY_PATH, &patch) {
                // Entidade sem `Visibility`, ou app mínima sem o
                // `ReflectPlugin`: mantém o comportamento antigo de a
                // inserir. `Inherited` (e não `Visible`) para a
                // entidade voltar a respeitar o pai escondido.
                Ok(false) => insert_visibility(world, entity, visible),
                Err(error) => {
                    warn!(target: "viber::luau", "viber.set_visible: {error}");
                    insert_visibility(world, entity, visible);
                }
                Ok(true) => {}
            }
        }
        ScriptCommand::Say { text, secs, speaker } => {
            let text = world
                .get_resource::<crate::locale::Locale>()
                .map(|locale| crate::dialogue::localized_lines(&text, Some(&locale.table)))
                .unwrap_or(text);
            let duration = if secs > 0.0 {
                secs
            } else {
                crate::hud::BALLOON_DURATION
            };
            // A linha resolvida chega ao jogo (com o orador) mesmo quando
            // o HUD nativo a desenha: é o contrato para um presenter
            // próprio e para legendas/QA.
            world.write_message(super::components::ScriptSay {
                speaker,
                text: text.clone(),
                seconds: duration,
            });
            // Fala no balão baixa a música (genérico: qualquer `viber.say`
            // é diálogo, não um toast de sistema). A voz, quando existir,
            // fica por cima; o SFX não é tocado.
            if let Some(mut duck) = world.get_resource_mut::<crate::music::AudioDuck>() {
                duck.apply(
                    crate::music::AudioBus::Music,
                    0.35,
                    duration + 0.5,
                    elapsed,
                );
            }
            if world
                .get_resource::<crate::dialogue::ExternalSayPresenter>()
                .is_none()
            {
                let shown = show_balloon_world(world, &text, duration);
                if !shown {
                    warn_once(
                        world,
                        "balloon",
                        "viber.say: o mundo não declara <DialogueBalloon> — o texto não aparece no HUD",
                    );
                }
            }
        }
        ScriptCommand::SaveNow => {
            if let Some(mut request) = world.get_resource_mut::<crate::save::SaveRequest>() {
                request.save = true;
                request.slot = None;
            }
        }
        ScriptCommand::LoadNow => {
            if let Some(mut request) = world.get_resource_mut::<crate::save::SaveRequest>() {
                request.load = true;
                request.slot = None;
            }
        }
        ScriptCommand::SaveSlot(slot) => {
            if let Some(mut request) = world.get_resource_mut::<crate::save::SaveRequest>() {
                request.save = true;
                request.slot = Some(slot);
            }
        }
        ScriptCommand::LoadSlot(slot) => {
            if let Some(mut request) = world.get_resource_mut::<crate::save::SaveRequest>() {
                request.load = true;
                request.slot = Some(slot);
            }
        }
        ScriptCommand::SetLocale(lang) => {
            let seeded = world.get_resource_mut::<crate::locale::Locale>().map(|mut locale| {
                locale.set_lang(&lang);
                (locale.lang.clone(), locale.table.clone())
            });
            if let Some((active, table)) = seeded
                && let Some(host) = world.get_resource::<LuaScriptHost>()
                && let Some(mut ctx) = host.lua.app_data_mut::<ScriptCtx>()
            {
                ctx.locale_table = Some(std::sync::Arc::new(table));
                ctx.locale_lang = Some(active);
            }
        }
        ScriptCommand::Quit => {
            world.write_message(AppExit::Success);
        }
        ScriptCommand::SpawnPrototype { name, pos, seat, on_spawned, caller_path } => {
            if let Some(mut spawns) =
                world.get_resource_mut::<crate::recipes::spawn::PendingScriptSpawns>()
            {
                spawns.0.push(crate::recipes::spawn::PendingPrototypeSpawn {
                    name,
                    pos,
                    seat,
                    on_spawned,
                    caller_path,
                });
            }
        }
        ScriptCommand::SetPose { entity, pos, yaw } => {
            // TODO(migration): shim Lua mantido; a escrita passa pela
            // reflexão. Um patch parcial preserva a escala e cobre
            // qualquer entidade com `Transform` (script, herói, prop).
            let patch = super::ecs::EcsValue::Map(vec![
                (
                    "translation".to_string(),
                    super::ecs::EcsValue::Vec3([pos.x, pos.y, pos.z]),
                ),
                (
                    "rotation".to_string(),
                    super::ecs::EcsValue::Quat(Quat::from_rotation_y(yaw).to_array()),
                ),
            ]);
            if let Err(error) =
                super::ecs::patch_component(world, entity, super::ecs::TRANSFORM_PATH, &patch)
            {
                // Sem reflexão (app mínima): escrita directa de
                // sempre, preservando a escala.
                warn!(target: "viber::luau", "viber.set_pose: {error}");
                if let Some(mut transform) = world.get_mut::<Transform>(entity) {
                    let scale = transform.scale;
                    transform.translation = pos;
                    transform.rotation = Quat::from_rotation_y(yaw);
                    transform.scale = scale;
                }
            }
        }
        ScriptCommand::SoundLoop { id, clip, pitch, volume, position, ambience, owner } => {
            if let Some(mut loops) = world.get_resource_mut::<crate::music::ScriptSoundLoops>() {
                loops.wanted.insert(
                    id,
                    crate::music::ScriptLoopParams {
                        clip,
                        pitch,
                        volume,
                        position,
                        ambience,
                        owner,
                    },
                );
            }
        }
        ScriptCommand::SoundStop(id) => {
            if let Some(mut loops) = world.get_resource_mut::<crate::music::ScriptSoundLoops>() {
                loops.wanted.remove(&id);
            }
        }
        ScriptCommand::MusicLayer(layer) => {
            if let Some(mut over) = world.get_resource_mut::<crate::music::MusicLayerOverride>() {
                over.0 = layer.filter(|name| !name.is_empty());
            }
        }
        ScriptCommand::MusicCombat { active, boss } => {
            if let Some(mut combat) = world.get_resource_mut::<crate::music::CombatMusicState>() {
                if active {
                    combat.engage(elapsed, boss);
                } else {
                    combat.clear();
                }
            }
        }
        ScriptCommand::AudioDuck { bus, gain, secs } => {
            if let Some(mut duck) = world.get_resource_mut::<crate::music::AudioDuck>()
                && let Some(bus) = crate::music::AudioBus::parse(&bus)
            {
                duck.apply(bus, gain, secs, elapsed);
            }
        }
        ScriptCommand::AudioReverb { preset, wet } => {
            if let Some(mut request) = world.get_resource_mut::<crate::music::AudioReverb>()
                && let Some(preset) = crate::music::ReverbPreset::parse(&preset)
            {
                request.preset = preset;
                request.wet = wet;
            }
        }
        ScriptCommand::Decal { url, position, normal, size, life, color } => {
            if let Some(mut queue) = world.get_resource_mut::<crate::decal_fx::DecalQueue>() {
                queue.0.push(crate::decal_fx::DecalRequest {
                    url,
                    position,
                    normal,
                    size,
                    life,
                    color: Color::srgb(color[0], color[1], color[2]),
                });
            }
        }
        ScriptCommand::SetMarkers { blips, beacons, waypoint } => {
            if let Some(mut markers) = world.get_resource_mut::<crate::hud::markers::HudMarkers>() {
                markers.blips = blips
                    .into_iter()
                    .map(|(position, tower)| crate::hud::markers::HudBlip { position, tower })
                    .collect();
                markers.beacons = beacons;
                markers.waypoint = waypoint
                    .map(|(position, label)| crate::hud::markers::HudPoint { position, label });
            }
        }
        ScriptCommand::Noise { pos, loudness } => {
            if let Some(mut bus) = world.get_resource_mut::<crate::senses::SenseBus>() {
                bus.events.push(crate::senses::SenseEvent {
                    kind: crate::senses::SenseKind::Hear,
                    pos,
                    loudness,
                    from: None,
                });
            }
        }
        // `viber.ecs.*` (a tabela Lua) morreu: a escrita genérica por
        // reflexão sobrevive só nas primitivas (`set_scale`/`set_visible`
        // /`set_position` via `patch_component`).
    }
}

/// Dano em ÁREA sobre `&mut World` — corpo 1:1 do braço `RadialDamage` do
/// `luau_update`, com o `skip` do BMS (exclui o emissor; o monólito não tem
/// o campo, passa `None`).
pub(crate) fn apply_radial_damage(
    world: &mut World,
    x: f32,
    z: f32,
    radius: f32,
    damage: f32,
    knockback: f32,
    skip: Option<Entity>,
) {
    // Acções recolhidas no loop (o `iter_mut` guarda o borrow mutável do
    // World) e aplicadas fora — mesma ordem de efeito do braço original.
    let mut knocks: Vec<(Entity, Vec3, f32)> = Vec::new();
    let mut kills: Vec<Entity> = Vec::new();
    {
        let mut query = world.query_filtered::<
            (Entity, &mut Health, &GlobalTransform),
            Without<Player>,
        >();
        for (entity, mut health, transform) in query.iter_mut(world) {
            if health.current <= 0.0 || skip == Some(entity) {
                continue;
            }
            let pos = transform.translation();
            let dist = Vec2::new(pos.x - x, pos.z - z).length();
            let Some(amount) = crate::vitals::radial_damage(dist, radius, damage) else {
                continue;
            };
            crate::vitals::apply_damage(&mut health, amount);
            if knockback > 0.0
                && let Some(strength) =
                    crate::physics_fx::radial_strength(dist, radius, knockback)
            {
                let dir = (pos - Vec3::new(x, pos.y, z)).normalize_or_zero();
                knocks.push((entity, dir, strength));
            }
            if health.current <= 0.0 {
                kills.push(entity);
            }
        }
    }
    for (entity, dir, strength) in knocks {
        world
            .commands()
            .entity(entity)
            .try_insert(crate::physics_fx::knockback_after(dir, strength));
    }
    let kills = kills
        .into_iter()
        .map(|entity| {
            let name = world
                .get::<LuaScriptRef>(entity)
                .map(|lref| crate::script_kind::script_kind(&lref.path))
                .unwrap_or_else(|| "creature".to_string());
            (entity, name)
        })
        .collect::<Vec<_>>();
    for (entity, name) in kills {
        if let Some(mut events) = world.get_resource_mut::<super::events::ScriptEventQueue>() {
            events.push(super::events::ScriptGameEvent::Kill {
                name,
                entity: entity.to_bits() as i64,
            });
        }
    }
}


//! O plugin: lê as cercas do mundo e gera a geometria.
//!
//! Uma cerca é uma entidade com [`FenceSpec`]. Em cada frame o plugin olha as
//! que nasceram ou mudaram (ou todas, quando as definições mudam), compara com
//! o que as gerou da última vez ([`FenceBuilt`]) e põe na fila as que ficaram
//! diferentes. A fila gera até `FenceSettings::batch` cercas por frame: os
//! filhos antigos saem, os novos entram com uma malha por material e, com a
//! feature `physics`, a entidade ganha um colisor de caixas.
//!
//! O chão vem do host: [`FenceGround`] num [`FenceGroundProvider`]. Sem ele
//! (editor, interiores) os postes ficam à cota autorada. As definições chegam
//! por [`FenceStyleSet`] e [`FenceSettingsValue`] — o jogo copia-as dos
//! recursos da cena (`SceneSetting`), o editor lê-as como componentes.

use std::collections::{HashSet, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use bevy::ecs::entity::Entities;
use bevy::math::EulerRot;
use bevy::prelude::*;
use bevy_ext_gltf::textures::WorldTiledTextures;
use bevy_ext_scene::FenceSpec;

use crate::layout::{self, FenceFrame, FenceLayout};
use crate::material::FenceMaterials;
use crate::mesh;
use crate::specs::{FenceKind, FenceSettings, FenceStyle, find, resolve};

/// O chão onde a cerca se apoia, em coordenadas do mundo.
pub trait FenceGround: Send + Sync {
    /// A cota do chão num ponto; `None` quando não há chão ali.
    fn height(&self, x: f32, z: f32) -> Option<f32>;
}

/// O chão do host (o jogo põe aqui o terreno).
#[derive(Resource, Clone, Default)]
pub struct FenceGroundProvider(pub Option<Arc<dyn FenceGround>>);

/// As predefinições do mundo, por ordem de edição.
#[derive(Resource, Debug, Default, Clone, PartialEq)]
pub struct FenceStyleSet(pub Vec<FenceStyle>);

/// As definições globais; `None` até o host as dar (ficam as por omissão).
#[derive(Resource, Debug, Default, Clone, PartialEq)]
pub struct FenceSettingsValue(pub Option<FenceSettings>);

/// A entidade já tem a malha; o número é o que a gerou.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct FenceBuilt(pub u64);

/// Filho gerado de uma cerca (vai com ela ao refazer).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct FencePart;

/// Marca o colisor que o plugin pôs (para o tirar sem tocar nos outros).
#[cfg(feature = "physics")]
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct FenceCollider;

/// Marca o corpo fixo que o plugin pôs com o colisor.
#[cfg(feature = "physics")]
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct FenceBody;

/// Que colisor a cerca leva (`fence-collider`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ColliderMode {
    /// Uma caixa fina por vão: ninguém passa entre as travessas.
    #[default]
    Segment,
    /// Só os postes.
    Posts,
    /// Nenhum.
    None,
}

impl ColliderMode {
    #[cfg(feature = "physics")]
    fn parse(text: Option<&str>) -> Self {
        match text.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("posts" | "post" | "postes") => Self::Posts,
            Some("none" | "off" | "0" | "false" | "sem") => Self::None,
            _ => Self::Segment,
        }
    }
}

#[derive(Resource, Default)]
struct FenceQueue {
    pending: VecDeque<Entity>,
    queued: HashSet<Entity>,
}

impl FenceQueue {
    fn push(&mut self, entity: Entity) {
        if self.queued.insert(entity) {
            self.pending.push_back(entity);
        }
    }

    fn pop(&mut self, entities: &Entities) -> Option<Entity> {
        loop {
            let entity = self.pending.pop_front()?;
            self.queued.remove(&entity);
            if entities.contains(entity) {
                return Some(entity);
            }
        }
    }
}

/// Gera as cercas do mundo a partir de [`FenceSpec`].
pub struct FencePlugin;

impl Plugin for FencePlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<crate::specs::FenceSettingsPlugin>() {
            app.add_plugins(crate::specs::FenceSettingsPlugin);
        }
        // O sampler repetido das texturas do pool tem um dono; sem o registo
        // o material carrega na mesma, só repete mal.
        if !app.is_plugin_added::<bevy_ext_gltf::textures::TexturesPlugin>() {
            app.add_plugins(bevy_ext_gltf::textures::TexturesPlugin);
        }
        app.init_resource::<FenceGroundProvider>()
            .init_resource::<FenceStyleSet>()
            .init_resource::<FenceSettingsValue>()
            .init_resource::<FenceMaterials>()
            .init_resource::<FenceQueue>()
            .add_systems(Update, (collect_fences, build_fences, forget_fences).chain());
    }
}

fn settings_of(settings: &FenceSettingsValue) -> FenceSettings {
    settings.0.clone().unwrap_or_default()
}

/// A moldura de uma entidade: posição, yaw e escala uniforme.
pub fn frame_of(global: &GlobalTransform) -> FenceFrame {
    let (scale, rotation, origin) = global.to_scale_rotation_translation();
    FenceFrame {
        origin,
        yaw: rotation.to_euler(EulerRot::YXZ).0,
        scale: scale.x.abs().max(1e-3),
    }
}

/// A semente da variação: o nome da cerca (estável entre sessões) ou, sem
/// nome, a própria linha.
pub fn seed_key(name: Option<&Name>, spec: &FenceSpec) -> u64 {
    let mut hasher = DefaultHasher::new();
    match name.map(|name| name.as_str()).filter(|name| !name.is_empty()) {
        Some(name) => name.hash(&mut hasher),
        None => spec.points.hash(&mut hasher),
    }
    hasher.finish()
}

/// O que gerou uma cerca: o pedido, a predefinição resolvida, a moldura e as
/// definições globais. Muda um, muda o número — e a cerca volta à fila.
fn fingerprint(spec: &FenceSpec, style: &FenceStyle, frame: &FenceFrame, settings: Option<&FenceSettings>) -> u64 {
    let mut hasher = DefaultHasher::new();
    (format!("{spec:?}"), format!("{style:?}"), format!("{settings:?}")).hash(&mut hasher);
    // O milímetro chega: a cerca não se refaz por um assentamento sub-milimétrico.
    for value in [
        frame.origin.x,
        frame.origin.y,
        frame.origin.z,
        frame.yaw,
        frame.scale,
    ] {
        (value * 1000.0).round().to_bits().hash(&mut hasher);
    }
    hasher.finish()
}

#[allow(clippy::type_complexity)]
fn collect_fences(
    mut queue: ResMut<FenceQueue>,
    settings: Res<FenceSettingsValue>,
    styles: Res<FenceStyleSet>,
    fences: Query<(Entity, Ref<FenceSpec>, Option<&Name>, Option<&GlobalTransform>, Option<&FenceBuilt>)>,
) {
    let definitions_changed = settings.is_changed() || styles.is_changed();
    let defaults = settings_of(&settings);
    for (entity, spec, name, global, built) in &fences {
        if !definitions_changed && built.is_some() && !spec.is_changed() {
            continue;
        }
        let Some(global) = global else {
            continue;
        };
        let key = spec.style.as_deref().unwrap_or(&defaults.default_style);
        let Some(style) = find(&styles.0, key) else {
            continue;
        };
        let frame = frame_of(global);
        let fingerprint = fingerprint(&spec, &resolve(&spec, style), &frame, settings.0.as_ref());
        let _ = name;
        if built.is_none_or(|built| built.0 != fingerprint) {
            queue.push(entity);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_fences(
    mut commands: Commands,
    entities: &Entities,
    mut queue: ResMut<FenceQueue>,
    settings: Res<FenceSettingsValue>,
    styles: Res<FenceStyleSet>,
    ground: Res<FenceGroundProvider>,
    mut materials: ResMut<FenceMaterials>,
    server: Option<Res<AssetServer>>,
    mut tiled: Option<ResMut<WorldTiledTextures>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut standard: ResMut<Assets<StandardMaterial>>,
    fences: Query<(Entity, &FenceSpec, Option<&Name>, &GlobalTransform)>,
    parts: Query<(Entity, &ChildOf), With<FencePart>>,
) {
    let defaults = settings_of(&settings);
    for _ in 0..defaults.batch.max(1) {
        let Some(entity) = queue.pop(entities) else {
            return;
        };
        let Ok((entity, spec, name, global)) = fences.get(entity) else {
            continue;
        };
        let key = spec.style.as_deref().unwrap_or(&defaults.default_style);
        let Some(style) = find(&styles.0, key) else {
            warn!("cerca sem a predefinição `{key}` — falta em world/settings/fences.bsn");
            continue;
        };
        let style = resolve(spec, style);
        let frame = frame_of(global);
        let provider = ground.0.clone();
        let ground = move |x: f32, z: f32| provider.as_ref().and_then(|ground| ground.height(x, z));
        let fence = match layout::layout(spec, &style, &frame, &ground, seed_key(name, spec)) {
            Ok(fence) => fence,
            Err(error) => {
                warn!("cerca {:?}: {error}", name.map(|name| name.as_str()).unwrap_or("?"));
                continue;
            }
        };

        // Os filhos antigos saem; a entidade fica (é ela que o editor endereça).
        let stale: Vec<Entity> = parts
            .iter()
            .filter(|(_, child)| child.parent() == entity)
            .map(|(part, _)| part)
            .collect();
        for part in stale {
            commands.entity(part).despawn();
        }

        let iron = materials.iron(defaults.iron_color, &mut standard);
        let mut handles: [Option<Handle<StandardMaterial>>; 3] = [None, None, Some(iron.clone())];
        if let Some(server) = server.as_deref() {
            let mut tiled = tiled.as_mut();
            let primary = materials.wood(&style.material, &defaults.materials_root, server, tiled.as_mut().map(|t| t.reborrow()), &mut standard);
            let secondary = if style.secondary_material.trim().is_empty() {
                primary.clone()
            } else {
                materials.wood(&style.secondary_material, &defaults.materials_root, server, tiled.as_mut().map(|t| t.reborrow()), &mut standard)
            };
            handles = [Some(primary), Some(secondary), Some(iron)];
        }
        for (slot, surface) in mesh::build(&fence, &style) {
            let label = match slot {
                layout::Slot::Primary => "madeira",
                layout::Slot::Secondary => "varas",
                layout::Slot::Iron => "ferragens",
            };
            let mut part = commands.spawn((
                Name::new(format!("cerca · {label}")),
                ChildOf(entity),
                FencePart,
                Transform::default(),
                Visibility::default(),
                Mesh3d(meshes.add(surface.to_mesh())),
            ));
            if let Some(handle) = &handles[slot as usize] {
                part.insert(MeshMaterial3d(handle.clone()));
            }
        }

        #[cfg(feature = "physics")]
        {
            use avian3d::prelude::{Collider, RigidBody};
            let pieces = collider_pieces(&fence, &style, ColliderMode::parse(spec.collider.as_deref()));
            let mut target = commands.entity(entity);
            if pieces.is_empty() {
                target.remove::<(Collider, FenceCollider)>();
                drop_static_body(&mut commands, entity);
            } else {
                // Corpo FIXO com a forma, como os sólidos do mundo: um `Collider`
                // sozinho entra na árvore (os raios batem-lhe) mas o herói
                // atravessava-o — encontrado na QA ao vivo.
                target.try_insert((
                    RigidBody::Static,
                    FenceBody,
                    Collider::compound(
                        pieces
                            .into_iter()
                            .map(|(position, rotation, size)| (position, rotation, Collider::cuboid(size.x, size.y, size.z)))
                            .collect(),
                    ),
                    FenceCollider,
                ));
            }
        }

        let fingerprint = fingerprint(spec, &style, &frame, settings.0.as_ref());
        commands.entity(entity).insert(FenceBuilt(fingerprint));
    }
}

/// Uma cerca que perdeu o [`FenceSpec`] (removido no editor) perde a malha e
/// o colisor.
fn forget_fences(
    mut commands: Commands,
    mut removed: RemovedComponents<FenceSpec>,
    parts: Query<(Entity, &ChildOf), With<FencePart>>,
    built: Query<(), With<FenceBuilt>>,
) {
    let gone: Vec<Entity> = removed.read().collect();
    if gone.is_empty() {
        return;
    }
    for entity in &gone {
        if built.contains(*entity) {
            let mut target = commands.entity(*entity);
            target.remove::<FenceBuilt>();
            #[cfg(feature = "physics")]
            {
                target.remove::<(avian3d::prelude::Collider, FenceCollider)>();
                drop_static_body(&mut commands, *entity);
            }
        }
    }
    for (part, child) in &parts {
        if gone.contains(&child.parent()) {
            commands.entity(part).despawn();
        }
    }
}

/// Tira o corpo fixo que o plugin pôs — só o seu ([`FenceBody`]): um corpo
/// autorado na entidade não é tocado.
#[cfg(feature = "physics")]
fn drop_static_body(commands: &mut Commands, entity: Entity) {
    commands.queue(move |world: &mut World| {
        if world.get_entity(entity).is_ok_and(|target| target.contains::<FenceBody>()) {
            world
                .entity_mut(entity)
                .remove::<(avian3d::prelude::RigidBody, FenceBody)>();
        }
    });
}

/// A rotação em Y que leva o eixo X a `direction` (horizontal, unitário).
fn yaw_of(direction: Vec3) -> Quat {
    Quat::from_rotation_y((-direction.z).atan2(direction.x))
}

/// As caixas do colisor: posição, rotação e tamanho completo de cada uma.
///
/// Por vão: uma caixa fina e alta — ninguém passa entre as travessas, e a
/// navegação vê a cerca como parede. Um portão fechado bloqueia o vão; aberto,
/// a caixa da folha roda com ela.
pub fn collider_pieces(fence: &FenceLayout, style: &FenceStyle, mode: ColliderMode) -> Vec<(Vec3, Quat, Vec3)> {
    let mut out = Vec::new();
    if mode == ColliderMode::None {
        return out;
    }
    let posts = |out: &mut Vec<(Vec3, Quat, Vec3)>| {
        for post in &fence.posts {
            out.push((
                post.base + Vec3::Y * post.height * 0.5,
                yaw_of(post.along),
                Vec3::new(post.width, post.height + 0.02, post.depth),
            ));
        }
    };
    if mode == ColliderMode::Posts || style.kind == FenceKind::Gate {
        posts(&mut out);
        if style.kind != FenceKind::Gate {
            return out;
        }
    }
    for span in &fence.spans {
        let (a, b) = (&fence.posts[span.a], &fence.posts[span.b]);
        let flat = Vec3::new(b.base.x - a.base.x, 0.0, b.base.z - a.base.z);
        let length = flat.length();
        if length < 0.05 {
            continue;
        }
        let top = (a.base.y + a.height).max(b.base.y + b.height);
        let bottom = a.base.y.min(b.base.y);
        let (open, gate) = (style.gate_open_deg.to_radians(), style.kind == FenceKind::Gate);
        let (center, rotation, size) = if gate {
            // A folha vai de face a face dos postes, a meia altura.
            let inner = length - a.half() - b.half();
            let hinge = Vec3::new(a.base.x, 0.0, a.base.z) + flat / length * a.half();
            let half = inner * 0.5;
            let turn = yaw_of(flat / length);
            let middle = hinge + (turn * Vec3::X) * half;
            let turned = Quat::from_rotation_y(-open) * (middle - hinge) + hinge;
            (
                Vec3::new(turned.x, (top + bottom) * 0.5, turned.z),
                Quat::from_rotation_y(-open) * turn,
                Vec3::new(inner, top - bottom - 0.12, a.depth.max(b.depth)),
            )
        } else {
            (
                Vec3::new((a.base.x + b.base.x) * 0.5, (top + bottom) * 0.5, (a.base.z + b.base.z) * 0.5),
                yaw_of(flat / length),
                Vec3::new(length, top - bottom + 0.02, a.depth.max(b.depth).max(0.2)),
            )
        };
        out.push((center, rotation, size));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::specs::{gate, pasture_rail, resolve};

    fn fence(points: &str, style: &FenceStyle) -> FenceLayout {
        let spec = FenceSpec {
            points: points.into(),
            kind: Some(style.kind.key().into()),
            ..Default::default()
        };
        layout::layout(&spec, &resolve(&spec, style), &FenceFrame::default(), &|_, _| Some(0.0), 1).unwrap()
    }

    #[test]
    fn as_caixas_cobrem_os_voes() {
        let layout = fence("0 0 6 0", &pasture_rail());
        let pieces = collider_pieces(&layout, &pasture_rail(), ColliderMode::Segment);
        assert_eq!(pieces.len(), layout.spans.len());
        assert_eq!(pieces[0].2.x, 3.0, "um vão de 3 m por caixa");
        assert!((pieces[0].2.y - 1.24).abs() < 0.05, "a altura do poste mais a folga");
        assert!(pieces[0].2.z >= 0.2, "ninguém atravessa entre as travessas");
        assert!((pieces[0].0.x - 1.5).abs() < 1e-4 && (pieces[0].0.y - 0.6).abs() < 0.05);
        assert_eq!(collider_pieces(&layout, &pasture_rail(), ColliderMode::Posts).len(), layout.posts.len());
        assert!(collider_pieces(&layout, &pasture_rail(), ColliderMode::None).is_empty());
    }

    #[test]
    fn o_portao_fechado_bloqueia_o_vao_e_o_aberto_rosse() {
        let style = gate();
        let layout = fence("0 0 2.6 0", &style);
        let closed = collider_pieces(&layout, &style, ColliderMode::Segment);
        assert_eq!(closed.len(), 3, "dois postes e a folha");
        assert!((closed[2].0.x - 1.3).abs() < 0.05, "a folha ao meio do vão");
        let mut open = style.clone();
        open.gate_open_deg = 90.0;
        let opened = collider_pieces(&layout, &open, ColliderMode::Segment);
        assert!(opened[2].0.z > 0.8, "a folha saiu para o lado: {:?}", opened[2].0);
    }

    #[test]
    fn a_moldura_le_o_yaw_e_a_escala() {
        let local = Transform::from_translation(Vec3::new(5.0, 2.0, -3.0))
            .with_rotation(Quat::from_rotation_y(0.7))
            .with_scale(Vec3::splat(2.0));
        let global = GlobalTransform::from(local);
        let frame = frame_of(&global);
        assert_eq!(frame.origin, Vec3::new(5.0, 2.0, -3.0));
        assert!((frame.yaw - 0.7).abs() < 1e-5);
        assert_eq!(frame.scale, 2.0);
    }

    #[test]
    fn a_semente_vem_do_nome_e_e_estavel() {
        let spec = FenceSpec {
            points: "0 0 3 0".into(),
            ..Default::default()
        };
        let named = Name::new("cerca-oeste");
        let other = Name::new("cerca-leste");
        assert_eq!(seed_key(Some(&named), &spec), seed_key(Some(&named), &spec));
        assert_ne!(seed_key(Some(&named), &spec), seed_key(Some(&other), &spec));
        assert_eq!(seed_key(Some(&Name::new("")), &spec), seed_key(None, &spec));
    }

    #[test]
    fn o_numero_muda_com_o_que_gera_a_cerca() {
        let spec = FenceSpec::default();
        let style = pasture_rail();
        let frame = FenceFrame::default();
        let base = fingerprint(&spec, &style, &frame, None);
        assert_eq!(base, fingerprint(&spec, &style, &frame, None));
        let moved = FenceFrame {
            origin: Vec3::new(0.0, 0.002, 0.0),
            ..frame
        };
        assert_ne!(base, fingerprint(&spec, &style, &moved, None));
        // Meio milímetro não conta: não se refaz por um assentamento.
        let nudge = FenceFrame {
            origin: Vec3::new(0.0, 0.0004, 0.0),
            ..frame
        };
        assert_eq!(base, fingerprint(&spec, &style, &nudge, None));
        let taller = FenceSpec {
            height: Some(1.4),
            ..spec.clone()
        };
        assert_ne!(base, fingerprint(&taller, &resolve(&taller, &style), &frame, None));
    }

    #[test]
    fn o_modo_do_colisor_le_o_texto() {
        assert_eq!(ColliderMode::parse(None), ColliderMode::Segment);
        assert_eq!(ColliderMode::parse(Some(" Posts ")), ColliderMode::Posts);
        assert_eq!(ColliderMode::parse(Some("none")), ColliderMode::None);
        assert_eq!(ColliderMode::parse(Some("desconhecido")), ColliderMode::Segment);
    }
}

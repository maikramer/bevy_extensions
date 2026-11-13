//! Câmara do viewport à escala do mundo do Vael.
//!
//! A câmara de voo do Jackdaw anda a velocidade fixa: o scroll avança 1 m por
//! tique e o WASD 10 m/s (no máximo 100 m/s). Num mundo de 4 km isso é
//! inutilizável: da vista geral até à vila são milhares de tiques. Este
//! módulo escala as duas velocidades pela distância ao que a câmara está a
//! ver (raycast pelo centro do viewport; sem chão à frente, a altura), à
//! maneira do Blender: cada tique de scroll anda uma fração fixa da distância
//! e o voo acelera no ar e abranda perto do chão.
//!
//! O `JackdawCameraSettings` vive no dylib do editor e não está no SDK das
//! extensões. Lemo-lo pelo `ComponentId` com um espelho do struct
//! ([`CameraSettingsMirror`]); a ligação só é feita se o tamanho, o
//! alinhamento e os valores lidos baterem certo, senão o módulo desliga-se
//! com um aviso (o editor continua com a câmara de origem).

use std::alloc::Layout;

use bevy::ecs::component::ComponentId;
use bevy::picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings, RayCastVisibility};
use bevy::prelude::*;
use jackdaw_extension::viewport::MainViewportCamera;

/// Nome do componente da câmara de voo no editor.
const SETTINGS_TYPE: &str = "jackdaw_camera::JackdawCameraSettings";
/// Velocidade base de voo do Jackdaw (`JackdawCameraSettings::default`).
const JACKDAW_BASE_SPEED: f32 = 10.0;
/// Limite com que o Jackdaw prende a velocidade no clique-direito + scroll.
const JACKDAW_SPEED_CLAMP: (f32, f32) = (0.5, 100.0);

/// Cópia campo a campo de `jackdaw_camera::JackdawCameraSettings` 0.20.2
/// (mesma ordem e tipos → o mesmo layout com o mesmo rustc).
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
struct CameraSettingsMirror {
    sensitivity: f32,
    speed: f32,
    run_multiplier: f32,
    enabled: bool,
    scroll_speed: f32,
    invert_y: bool,
}

impl CameraSettingsMirror {
    /// Os valores lidos parecem mesmo as definições da câmara (guarda contra
    /// um layout diferente noutra versão do editor).
    fn plausible(&self) -> bool {
        self.sensitivity > 0.0
            && self.sensitivity < 1.0
            && self.run_multiplier >= 1.0
            && self.run_multiplier <= 100.0
            && self.speed >= 0.1
            && self.speed.is_finite()
            && self.scroll_speed > 0.0
            && self.scroll_speed.is_finite()
    }
}

/// Distância até ao que a câmara está a ver, medida a 5 Hz.
#[derive(Resource, Default)]
pub struct CameraFocus {
    distance: Option<f32>,
    timer: f32,
}

/// Ligação ao componente do editor e velocidades aplicadas.
#[derive(Resource, Default)]
pub struct WorldScaleCamera {
    component: Option<ComponentId>,
    /// `true` quando a ligação falhou de vez (layout ou nome inesperado).
    disabled: bool,
    /// Velocidade de voo que o utilizador escolheu (clique-direito + scroll),
    /// antes do fator de escala.
    base_speed: f32,
    /// Últimos valores escritos, para detetar mudanças feitas pelo editor.
    applied_speed: f32,
    factor: f32,
}

pub fn app(app: &mut App) {
    app.init_resource::<CameraFocus>()
        .init_resource::<WorldScaleCamera>();
}

/// Fator de voo: 1× até 25 m, depois linear (2 km → 80×).
pub fn fly_factor(distance: f32) -> f32 {
    (distance / 25.0).clamp(1.0, 80.0)
}

/// Metros por tique de scroll: 15% da distância (nunca atravessa o alvo).
pub fn scroll_step(distance: f32) -> f32 {
    (distance * 0.15).clamp(1.0, 800.0)
}

/// Raycast pelo centro do viewport (e para baixo, se olhar para o céu).
pub fn measure_focus(
    time: Res<Time>,
    mut focus: ResMut<CameraFocus>,
    cameras: Query<&GlobalTransform, With<MainViewportCamera>>,
    mut ray_cast: MeshRayCast,
) {
    focus.timer -= time.delta_secs();
    if focus.timer > 0.0 {
        return;
    }
    focus.timer = 0.2;
    let Some(camera) = cameras.iter().next() else {
        focus.distance = None;
        return;
    };
    let origin = camera.translation();
    let settings = MeshRayCastSettings::default().with_visibility(RayCastVisibility::Visible);
    let forward = Ray3d::new(origin, camera.forward());
    let ahead = ray_cast
        .cast_ray(forward, &settings)
        .first()
        .map(|(_, hit)| hit.distance);
    let distance = ahead.or_else(|| {
        let down = Ray3d::new(origin, Dir3::NEG_Y);
        ray_cast
            .cast_ray(down, &settings)
            .first()
            .map(|(_, hit)| hit.distance)
    });
    // Sem nada por baixo (fora do terreno): a altura acima de y = 0.
    focus.distance = Some(distance.unwrap_or(origin.y.abs()).max(0.5));
}

/// Escreve as velocidades na câmara de voo do editor.
pub fn apply_world_scale(world: &mut World) {
    if world.resource::<WorldScaleCamera>().disabled {
        return;
    }
    let Some(distance) = world.resource::<CameraFocus>().distance else {
        return;
    };
    let Some(component) = settings_component(world) else {
        return;
    };
    let cameras: Vec<Entity> = world
        .query_filtered::<Entity, With<MainViewportCamera>>()
        .iter(world)
        .collect();
    let factor = fly_factor(distance);
    let step = scroll_step(distance);

    for camera in cameras {
        let Some(current) = world.get_entity(camera).ok().and_then(|entity| {
            let ptr = entity.get_by_id(component).ok()?;
            // SAFETY: `settings_component` só devolve o id se o layout do
            // componente for o do espelho e os valores lidos forem plausíveis.
            Some(unsafe { ptr.deref::<CameraSettingsMirror>() }.speed)
        }) else {
            continue;
        };
        let mut state = world_scale_state(world);
        if state.applied_speed == 0.0 {
            state.base_speed = current.max(JACKDAW_SPEED_CLAMP.0);
        } else if (current - state.applied_speed).abs() > 1e-3 {
            // O utilizador mudou a velocidade (clique-direito + scroll). O
            // editor prende-a a 100: se bateu no teto, conta como um passo.
            state.base_speed = if current >= JACKDAW_SPEED_CLAMP.1 && current < state.applied_speed {
                state.base_speed * 1.1
            } else {
                current / state.factor.max(1.0)
            }
            .clamp(JACKDAW_SPEED_CLAMP.0, JACKDAW_SPEED_CLAMP.1);
        }
        let speed = state.base_speed * factor;
        state.applied_speed = speed;
        state.factor = factor;
        store_world_scale_state(world, state);

        let Ok(mut entity) = world.get_entity_mut(camera) else {
            continue;
        };
        let Ok(ptr) = entity.get_mut_by_id(component) else {
            continue;
        };
        // SAFETY: ver acima.
        let mut settings = unsafe { ptr.with_type::<CameraSettingsMirror>() };
        let current = *settings.bypass_change_detection();
        if (current.speed - speed).abs() > 1e-3 || (current.scroll_speed - step).abs() > 1e-3 {
            settings.speed = speed;
            settings.scroll_speed = step;
        }
    }
}

#[derive(Clone, Copy)]
struct ScaleState {
    base_speed: f32,
    applied_speed: f32,
    factor: f32,
}

fn world_scale_state(world: &World) -> ScaleState {
    let state = world.resource::<WorldScaleCamera>();
    ScaleState {
        base_speed: if state.base_speed > 0.0 {
            state.base_speed
        } else {
            JACKDAW_BASE_SPEED
        },
        applied_speed: state.applied_speed,
        factor: state.factor,
    }
}

fn store_world_scale_state(world: &mut World, state: ScaleState) {
    let mut res = world.resource_mut::<WorldScaleCamera>();
    res.base_speed = state.base_speed;
    res.applied_speed = state.applied_speed;
    res.factor = state.factor;
}

/// Encontra (uma vez) o `ComponentId` das definições da câmara de voo e
/// valida o layout contra o espelho.
fn settings_component(world: &mut World) -> Option<ComponentId> {
    if let Some(id) = world.resource::<WorldScaleCamera>().component {
        return Some(id);
    }
    let camera = world
        .query_filtered::<Entity, With<MainViewportCamera>>()
        .iter(world)
        .next()?;
    let mirror = Layout::new::<CameraSettingsMirror>();
    let entity = world.get_entity(camera).ok()?;
    let mut by_name = None;
    let mut by_layout = Vec::new();
    for id in entity.archetype().components() {
        let Some(info) = world.components().get_info(*id) else {
            continue;
        };
        let name = info.name().to_string();
        if name == SETTINGS_TYPE {
            by_name = Some((*id, info.layout()));
        } else if info.layout() == mirror {
            // Sem nomes de debug no build do editor: candidato pelo layout.
            by_layout.push(*id);
        }
    }
    let candidates: Vec<ComponentId> = match by_name {
        Some((id, layout)) if layout == mirror => vec![id],
        Some((_, layout)) => {
            warn!(
                "vael camera: {SETTINGS_TYPE} tem layout {layout:?} (esperado {mirror:?}); \
                 velocidade à escala do mundo desligada"
            );
            world.resource_mut::<WorldScaleCamera>().disabled = true;
            return None;
        }
        None => by_layout,
    };
    let entity = world.get_entity(camera).ok()?;
    let plausible: Vec<ComponentId> = candidates
        .into_iter()
        .filter(|id| {
            entity.get_by_id(*id).is_ok_and(|ptr| {
                // SAFETY: o layout foi comparado com o do espelho acima.
                unsafe { ptr.deref::<CameraSettingsMirror>() }.plausible()
            })
        })
        .collect();
    if plausible.len() != 1 {
        warn!(
            "vael camera: {SETTINGS_TYPE} não encontrado ({} candidatos); velocidade à \
             escala do mundo desligada",
            plausible.len()
        );
        world.resource_mut::<WorldScaleCamera>().disabled = true;
        return None;
    }
    info!("vael camera: velocidade da câmara à escala do mundo ligada");
    world.resource_mut::<WorldScaleCamera>().component = Some(plausible[0]);
    Some(plausible[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perto_do_chao_fica_como_o_jackdaw() {
        assert_eq!(fly_factor(5.0), 1.0);
        assert_eq!(scroll_step(3.0), 1.0);
    }

    #[test]
    fn vista_geral_e_rapida_sem_atravessar() {
        assert!(fly_factor(2000.0) >= 60.0);
        let step = scroll_step(3000.0);
        assert!(step > 300.0 && step < 3000.0, "{step}");
    }
}

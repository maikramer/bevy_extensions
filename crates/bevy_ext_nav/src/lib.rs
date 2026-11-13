//! Navigation: a real navmesh, real pathfinding and real local avoidance.
//!
//! Before this stack the AI moved in a straight line toward whatever point it
//! wanted and wrote the result into `Transform`. Creatures walked through
//! houses, through walls and through each other; the only obstacle handling in
//! the whole repository was a per-script `if the distance stopped shrinking for
//! six seconds, pick somewhere else`. The `<NavMesh>` tag was parsed and thrown
//! away.
//!
//! The stack is three crates, all built against Bevy 0.20:
//!
//! * **`bevy_rerecast`** — Recast, the industry-standard navmesh voxeliser,
//!   ported to Rust. It asks a *backend* for the world geometry; ours is
//!   [`backend::nav_backend`], because neither the render meshes nor the
//!   streamed terrain colliders describe the world completely.
//! * **`landmass_rerecast`** — the bridge.
//! * **`bevy_landmass`** — A* over the navmesh, RVO avoidance between agents,
//!   and a cost multiplier per polygon type. That last one is the whole of
//!   "prefer roads": the road polygons are one node type, the ground is
//!   another, and a townsfolk simply prices the ground higher.
//!
//! Three properties are deliberate:
//!
//! * **One moving tile.** The world is 4 km wide and every script is gated to
//!   45 m around the hero, so a world navmesh would be expense without effect.
//!   See [`tile`].
//! * **No new API for the scripts.** `viber.move_towards` keeps its signature;
//!   [`agent`] intercepts the ask and returns a navigated one. All seventeen
//!   scripts in `examples/simple-rpg` improve without an edit.
//! * **Failure is the old behaviour.** No navmesh, agent off the mesh, no path,
//!   `VIBER_NAV=0` — the character beelines exactly as it used to. Navigation
//!   can make the world better, never worse.
//!
//! Wiring: `app.add_plugins(bevy_ext_nav::NavPlugin);`
//!
//! O terreno entra pelo contrato [`NavGround`] (o runtime do monólito publica
//! a implementação num [`NavGroundHandle`]) e a locomoção/marcadores pelo
//! `bevy_ext_core`; os pontos de ligação que o runtime tem de preencher estão
//! no README da crate.

use std::sync::Arc;

use bevy::prelude::*;
use bevy_ext_core::{AiLocomotionSystems, SceneLoad};
use bevy_landmass::prelude::*;
use bevy_landmass::{Agent3d, Character};
use bevy_rerecast::prelude::*;

pub mod agent;
pub mod backend;
pub mod tile;

pub use agent::{NavArchipelago, NavProfile};
pub use tile::{NavDirty, NavTile};

// ── contratos de terreno (interop com o runtime do mundo) ────────────────

/// Água: lago (círculo em [`NavWater::at`]) ou rio (polilinha em
/// [`NavWater::stations`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NavWaterKind {
    /// Lago — um círculo.
    Lake,
    /// Rio — polilinha de estações com raio constante.
    River,
}

/// Um corpo de água do mundo, com o raio que escava.
#[derive(Debug, Clone, PartialEq)]
pub struct NavWater {
    /// Centro do lago (ou da estação única de um rio de um nó).
    pub at: Vec2,
    /// Raio que a água escava.
    pub carve_radius: f32,
    pub kind: NavWaterKind,
    /// Estações da polilinha do rio (vazio num lago).
    pub stations: Vec<Vec2>,
}

/// Uma estrada do mundo: polilinha de estações com meia-largura por estação.
#[derive(Debug, Clone, PartialEq)]
pub struct NavRoad {
    /// Estações da polilinha (XZ).
    pub stations: Vec<Vec2>,
    /// Meia-largura por estação (m).
    pub half_width: Vec<f32>,
    /// Cota do tabuleiro, quando a estrada é uma ponte.
    pub deck_y: Option<f32>,
}

/// Um padrão pavimentado (praça): retângulo plano ao preço de estrada.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavPad {
    /// Centro (XZ).
    pub at: Vec2,
    /// Lado total do retângulo (m).
    pub size: Vec2,
    /// Cota do piso.
    pub height: f32,
}

/// Contrato de terreno para a navegação: o que o bake do tile e o push de
/// alvos precisam saber do mundo. O runtime do monólito implementa-o sobre o
/// `TerrainRuntime`; a navegação não sabe que o terreno é voxel.
///
/// TODO(viber-exit): a implementação sobre `TerrainRuntime` vive no monólito
/// (e é o único ponto de ligação que lhe falta para consumir esta crate).
pub trait NavGround: Send + Sync + 'static {
    /// `(x, z)` está dentro do heightfield?
    fn in_field(&self, x: f32, z: f32) -> bool;
    /// Cota da superfície RENDERIZADA (a que o herói pisa) em `(x, z)`.
    fn sample_surface(&self, x: f32, z: f32) -> f32;
    /// Cota do topo SÓLIDO do voxel em `(x, z)` — o chão que o navmesh assa.
    fn surface_top(&self, x: f32, z: f32) -> f32;
    /// Altura máxima esculpida do mundo (teto vertical do AABB do tile).
    fn max_height(&self) -> f32;
    /// Água do mundo, para furar o lattice.
    fn water(&self) -> &[NavWater];
    /// Estradas do mundo, para pintar [`tile::AREA_ROAD`].
    fn roads(&self) -> &[NavRoad];
    /// Padrões pavimentados (praças), também ao preço de estrada.
    fn pads(&self) -> &[NavPad];
    /// Falésia em `p`? Furos no chão — o lattice não emite o nó.
    fn is_cliff(&self, p: Vec2) -> bool;
}

/// Ponte para o terreno do runtime: quem monta o mundo insere
/// `NavGroundHandle(Arc::new(...))` com a implementação de [`NavGround`]
/// sobre o seu runtime. Sem o handle a navegação fica adormecida (não há
/// chão para assar) — o mesmo degradar elegante de `VIBER_NAV=0`.
#[derive(Resource, Clone)]
pub struct NavGroundHandle(pub Arc<dyn NavGround>);

/// Atributos declarados de `<NavMesh …>` no mundo — o assentamento do mundo
/// copia-os do `EngineConfigs` que já lê o XML.
///
/// TODO(viber-exit): o monólito insere-o junto de `EngineConfigs` no
/// `recipes::spawn::startup`.
#[derive(Resource, Debug, Default, Clone)]
pub struct NavMeshTag(pub Vec<(String, String)>);

/// Ancora de ordenação do assentamento do mundo: quem monta o mundo
/// (`recipes::spawn::startup` no monólito) junta-se a este set no `Startup`
/// e no `SceneLoad`, e a navegação aplica a `<NavMesh>` e cria o arquipélago
/// depois dele.
///
/// TODO(viber-exit): o monólito adiciona `.in_set(WorldSpawned)` ao sistema
/// de spawn nos dois schedules.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorldSpawned;

// ── config e plugin ──────────────────────────────────────────────────────

/// Default agent radius (m) — the fattest creature in the example plus a
/// little, so one navmesh serves every rig instead of one per species.
pub const DEFAULT_AGENT_RADIUS: f32 = 0.5;
/// Default agent height (m).
pub const DEFAULT_AGENT_HEIGHT: f32 = 1.9;
/// Default side of the navmesh tile (m).
///
/// 256 m around the hero covers the 45 m activation radius with room for the
/// hero to keep walking while the next tile bakes.
pub const DEFAULT_TILE_SIZE: f32 = 256.0;
/// The hero has to walk this far (m) from the tile centre before the next tile
/// is requested. Half the spare margin, so a generation never races the edge.
pub const DEFAULT_RETILE_MARGIN: f32 = 64.0;
/// Side (m) of a ground lattice cell handed to the voxeliser.
pub const DEFAULT_GROUND_CELL: f32 = 0.75;
/// What walking overland costs a civil profile, relative to a road.
pub const DEFAULT_OFFROAD_COST: f32 = 2.5;

/// Tunables of the navigation stack, filled from `<NavMesh …>` in the world
/// ([`NavMeshTag`]) and overridable per run through the environment.
#[derive(Resource, Debug, Clone, Copy, Reflect)]
#[reflect(Resource)]
// Preserva o TypePath do BRP/`viber.ecs` depois da extração.
#[type_path = "viber::nav"]
pub struct NavConfig {
    /// `VIBER_NAV=0` turns the whole stack off and restores the beeline.
    pub enabled: bool,
    pub agent_radius: f32,
    pub agent_height: f32,
    pub tile_size: f32,
    pub retile_margin: f32,
    pub ground_cell: f32,
    /// Maximum ledge a character walks up (m).
    pub walkable_climb: f32,
    /// Maximum walkable slope (degrees).
    pub max_slope_deg: f32,
    /// Cost multiplier of off-road ground for a [`NavProfile::Civil`] agent.
    pub offroad_cost: f32,
    /// How far ahead (s of travel) the locomotion ask is projected into a
    /// landmass destination.
    pub lookahead_secs: f32,
    /// Floor for that projection (m), so a slow walker still paths somewhere.
    pub min_lookahead: f32,
    /// Headroom landmass gets over the asked speed, for sidestepping.
    pub max_speed_factor: f32,
    /// How close (m) to its STATED destination a character has to be before a
    /// landmass "reached target" is believed.
    pub arrive_distance: f32,
    /// `VIBER_NAV_DEBUG=1`: census of agent states every frame, at `debug!`.
    /// The one question worth asking when characters stop moving is *why*
    /// landmass says no, and this is the cheapest way to ask it.
    pub debug: bool,
}

impl Default for NavConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            agent_radius: DEFAULT_AGENT_RADIUS,
            agent_height: DEFAULT_AGENT_HEIGHT,
            tile_size: DEFAULT_TILE_SIZE,
            retile_margin: DEFAULT_RETILE_MARGIN,
            ground_cell: DEFAULT_GROUND_CELL,
            walkable_climb: 0.5,
            max_slope_deg: 50.0,
            offroad_cost: DEFAULT_OFFROAD_COST,
            lookahead_secs: 2.0,
            min_lookahead: 3.0,
            max_speed_factor: 1.35,
            arrive_distance: 1.5,
            debug: false,
        }
    }
}

impl NavConfig {
    /// Reads the env overrides on top of the defaults. `VIBER_NAV=0` is the
    /// documented escape hatch.
    pub fn from_env() -> Self {
        let mut config = Self::default();
        if let Ok(value) = std::env::var("VIBER_NAV") {
            config.enabled = !matches!(value.trim(), "0" | "false" | "off");
        }
        if let Ok(value) = std::env::var("VIBER_NAV_DEBUG") {
            config.debug = !matches!(value.trim(), "0" | "false" | "off");
        }
        config
    }

    /// A janela de amostragem que assenta um ponto de query na navmesh — a
    /// MESMA que [`spawn_archipelago`] usa na criação do arquipélago. O debug
    /// bridge (`viber.nav_path`) reutiliza-a para que uma query responda o
    /// mesmo que um agente real leria naquele ponto.
    pub fn point_sample_distance(&self) -> bevy_landmass::PointSampleDistance3d {
        bevy_landmass::PointSampleDistance3d {
            horizontal_distance: self.agent_radius,
            distance_above: self.agent_height,
            distance_below: self.agent_height * 2.0,
            vertical_preference_ratio: 2.0,
            animation_link_max_vertical_distance: self.agent_height,
        }
    }

    /// Folds the attributes of a `<NavMesh …>` element into this config.
    ///
    /// Unknown attributes are ignored rather than fatal, and a value that does
    /// not parse leaves the default in place — a typo in a world file must not
    /// cost the world its navigation.
    pub fn apply_attrs(&mut self, attrs: &[(String, String)]) {
        for (key, value) in attrs {
            let Ok(number) = value.trim().parse::<f32>() else {
                if key == "enabled" {
                    self.enabled = !matches!(value.trim(), "0" | "false" | "off");
                }
                continue;
            };
            match key.as_str() {
                // `enabled="0"` parseia como f32 e caía no braço numérico
                // sem arm: a flag documentada era ignorada.
                "enabled" => self.enabled = number != 0.0,
                "agent-radius" => self.agent_radius = number.max(0.05),
                "agent-height" => self.agent_height = number.max(0.1),
                "tile-size" => self.tile_size = number.max(16.0),
                "retile-margin" => self.retile_margin = number.max(1.0),
                "cell-size" => self.ground_cell = number.max(0.1),
                "walkable-climb" => self.walkable_climb = number.max(0.0),
                "max-slope" => self.max_slope_deg = number.clamp(1.0, 89.0),
                "offroad-cost" => self.offroad_cost = number.max(1.0),
                _ => {}
            }
        }
        // A margin wider than the tile would never trigger a retile.
        self.retile_margin = self.retile_margin.min(self.tile_size * 0.5);
    }
}

/// System set of the navigation bridge, so ordering against landmass is stated
/// once instead of at every call site.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum NavSystems {
    /// Locomotion ask → landmass destination. Before landmass runs.
    Push,
    /// Landmass answer → locomotion ask. After landmass runs.
    Pull,
}

/// Registers the navmesh generator, the archipelago and the bridge.
#[derive(Default)]
pub struct NavPlugin;

impl Plugin for NavPlugin {
    fn build(&self, app: &mut App) {
        let config = NavConfig::from_env();
        if !config.enabled {
            info!("nav: VIBER_NAV=0 — navegação desligada, IA em linha recta");
            app.insert_resource(config);
            return;
        }
        app.insert_resource(config)
            .init_resource::<NavTile>()
            .init_resource::<tile::NavDirty>()
            // The tag lands with the world, in the world-spawn system —
            // PreStartup would run before any world exists and never see it.
            .add_systems(
                Startup,
                (apply_navmesh_tag, spawn_archipelago)
                    .chain()
                    .after(WorldSpawned),
            )
            .add_plugins((
                NavmeshPlugins::default(),
                // Landmass defaults to `FixedPreUpdate`, which in Bevy's main
                // schedule runs BEFORE `Update` — before any producer has
                // written this frame's locomotion ask. The bridge would then
                // read the ask that `apply_ai_locomotion` cleared at the end of
                // the previous frame, every agent would look motionless, and
                // landmass would (correctly) report every one of them Idle.
                // The whole pipeline therefore lives in `PostUpdate`, between
                // the producers and the single consumer of the ask.
                Landmass3dPlugin::default().in_schedule(bevy::app::PostUpdate),
                landmass_rerecast::LandmassRerecastPlugin::default()
                    .in_schedule(bevy::app::PostUpdate),
            ))
            .set_navmesh_backend(backend::nav_backend)
            // Troca de cena: a cena nova volta a ler a `<NavMesh>` e a criar o arquipélago
            // (o anterior foi apagado em `reset_for_scene`).
            .add_systems(
                SceneLoad,
                (apply_navmesh_tag, spawn_archipelago)
                    .chain()
                    .after(WorldSpawned),
            )
            .add_observer(on_navmesh_ready)
            // Ready BEFORE retile: the retile probe reads "not in flight and
            // still `generating`" as a failed bake, so success has to have
            // cleared the flag first.
            .add_systems(Update, (mark_tile_ready, tile::retile_navmesh).chain())
            .add_systems(Update, agent::attach_nav_agents);

        // One frame, one pipeline: the producers state an ask in `Update`, the
        // bridge turns it into a destination, landmass answers, the bridge
        // writes the answer back, and `apply_ai_locomotion` moves the
        // character — all before transforms propagate.
        app.configure_sets(
            bevy::app::PostUpdate,
            (
                NavSystems::Push.before(LandmassSystems::SyncExistence),
                NavSystems::Pull
                    .after(LandmassSystems::Output)
                    .before(AiLocomotionSystems),
            ),
        )
        .add_systems(
            bevy::app::PostUpdate,
            (
                agent::sync_hero_velocity
                    .in_set(NavSystems::Push)
                    .before(agent::push_nav_targets),
                agent::push_nav_targets.in_set(NavSystems::Push),
                agent::pull_nav_velocities.in_set(NavSystems::Pull),
            ),
        );
    }
}

/// Troca de cena: apaga o arquipélago e a ilha da cena anterior, repõe o tile e a config
/// (a `<NavMesh>` da cena nova é reaplicada no schedule `SceneLoad`).
pub fn reset_for_scene(world: &mut World) {
    if let Some(nav) = world.remove_resource::<NavArchipelago>() {
        for entity in [nav.island, nav.archipelago] {
            if let Ok(entity) = world.get_entity_mut(entity) {
                entity.despawn();
            }
        }
    }
    // Os autoloads PERSISTENTES (o herói, o HUD) sobrevivem à troca de cena e
    // mantêm `ArchipelagoRef3d` a apontar para o arquipélago que acabou de ser
    // despawnado: o landmass ignora refs mortas, o herói deixa de ser um
    // obstáculo para os agentes e os NPCs persistentes ficam sem path. Remover
    // o marcador faz o `attach_nav_agents` remontar o bundle contra o
    // arquipélago novo no Update seguinte (o mesmo frame de arranque da cena).
    let mut stale = world.query_filtered::<Entity, Or<(With<Agent3d>, With<Character<ThreeD>>)>>();
    let stale: Vec<Entity> = stale.iter(world).collect();
    for entity in stale {
        if let Ok(mut entity) = world.get_entity_mut(entity) {
            // O `Agent3d`/`Character` são os marcadores que o attach procura
            // (`Without<Agent3d>` / `Without<Character<ThreeD>>`); o resto do
            // bundle é substituído por inteiro quando ele corre.
            entity.remove::<Agent3d>();
            entity.remove::<Character<ThreeD>>();
        }
    }
    if world.contains_resource::<NavTile>() {
        world.insert_resource(NavTile::default());
    }
    if world.contains_resource::<NavConfig>() {
        world.insert_resource(NavConfig::from_env());
    }
}

/// Creates the single archipelago and its single island, and prices the two
/// node types.
fn spawn_archipelago(mut commands: Commands, config: Res<NavConfig>) {
    let mut options = ArchipelagoOptions::from_agent_radius(config.agent_radius);
    // The default sampling window is sized for a navmesh authored around the
    // agent (10 cm horizontally, 25 cm above). A character on carved voxel
    // terrain is nowhere near that precise: its Y comes from the rendered
    // surface sampler, the navmesh's from Recast's voxel spans, and the two
    // agree to roughly a cell. Too tight a window and every agent reads as
    // "not on the navmesh" — which is indistinguishable from having no
    // navigation at all. (Aqui e no debug bridge: `point_sample_distance`.)
    options.point_sample_distance = config.point_sample_distance();
    let mut archipelago = Archipelago3d::new(options);
    // Base prices are neutral: a road costs what ground costs. The preference
    // is per-agent (`AgentTypeIndexCostOverrides`), so a wolf and a merchant
    // can disagree about the same polygon.
    // `set_type_index_cost` refuses a non-positive cost; 1.0 never is, so the
    // Result carries no information worth branching on.
    let _ = archipelago.set_type_index_cost(agent::ROAD_TYPE_INDEX, 1.0);
    let _ = archipelago.set_type_index_cost(agent::GROUND_TYPE_INDEX, 1.0);
    let archipelago = commands.spawn(archipelago).id();
    // The island is born without a navmesh: the handle only exists once the
    // first tile is requested, and `landmass_rerecast::NavMeshHandle3d` is an
    // immutable component whose insert hook does the conversion wiring for us.
    let island = commands
        .spawn((
            Island,
            ArchipelagoRef3d::new(archipelago),
            Transform::default(),
            GlobalTransform::default(),
        ))
        .id();
    commands.insert_resource(NavArchipelago {
        archipelago,
        island,
    });
}

/// Clears the in-flight flag once the baked tile lands in the asset store.
///
/// The island itself needs no update: the rerecast handle is inserted once (by
/// [`tile::retile_navmesh`], on the first tile) and `regenerate` reuses it, so
/// `landmass_rerecast` re-converts in place and the agents never see a frame
/// without a navmesh.
fn mark_tile_ready(
    mut events: MessageReader<AssetEvent<bevy_rerecast::Navmesh>>,
    mut tile: ResMut<NavTile>,
) {
    for event in events.read() {
        let (AssetEvent::Added { id } | AssetEvent::Modified { id }) = event else {
            continue;
        };
        if tile
            .handle
            .as_ref()
            .is_some_and(|handle| handle.id() == *id)
            && tile.generating
        {
            tile.generating = false;
            tile.failures = 0;
            info!("nav: tile #{} pronto", tile.generations);
        }
    }
}

/// Clears the in-flight flag the moment `bevy_rerecast` reports a baked tile.
///
/// The trigger fires from the `PostUpdate` that polled the finished task, a
/// frame before the asset event reaches [`mark_tile_ready`] — which is what
/// lets [`tile::retile_navmesh`] tell a finished bake from a failed one.
fn on_navmesh_ready(ready: On<bevy_rerecast::prelude::NavmeshReady>, mut tile: ResMut<NavTile>) {
    if tile
        .handle
        .as_ref()
        .is_some_and(|handle| handle.id() == ready.0)
        && tile.generating
    {
        tile.generating = false;
        tile.failures = 0;
        info!("nav: tile #{} pronto", tile.generations);
    }
}

/// Lets `<NavMesh …>` in the world XML tune the stack.
///
/// The world spawn keeps the tag as raw data in [`NavMeshTag`]; this reads it
/// once, before the archipelago is built, so the agent radius the world asked
/// for is the one the archipelago is priced with.
fn apply_navmesh_tag(mut config: ResMut<NavConfig>, declared: Option<Res<NavMeshTag>>) {
    let Some(declared) = declared else {
        return;
    };
    config.apply_attrs(&declared.0);
    info!(
        "nav: <NavMesh> aplicada — raio {:.2} m, tile {:.0} m, custo fora-de-estrada {:.2}×",
        config.agent_radius, config.tile_size, config.offroad_cost
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Troca de cena: o herói (autoload persistente) mantinha o
    /// `ArchipelagoRef3d` a apontar para o arquipélago despawnado e deixava de
    /// ser um obstáculo para o landmass. O reset tem de o devolver ao estado
    /// "por montar" para o `attach_nav_agents` o religar ao novo.
    #[test]
    fn reset_for_scene_drops_stale_nav_bindings() {
        let mut world = World::new();
        let archipelago = world.spawn_empty().id();
        let island = world.spawn_empty().id();
        world.insert_resource(NavArchipelago {
            archipelago,
            island,
        });
        world.insert_resource(NavTile {
            generations: 3,
            ..NavTile::default()
        });
        let hero = world.spawn(Character::<ThreeD>::default()).id();
        let agent = world.spawn(Agent3d::default()).id();
        let plain = world.spawn_empty().id();

        reset_for_scene(&mut world);

        assert!(world.get::<Character<ThreeD>>(hero).is_none());
        assert!(world.get::<Agent3d>(agent).is_none());
        assert!(world.get_entity(plain).is_ok(), "entidades alheias ficam");
        assert!(world.get_resource::<NavArchipelago>().is_none());
        assert_eq!(
            world.resource::<NavTile>().generations,
            0,
            "o tile da cena antiga é reposto"
        );
    }
}

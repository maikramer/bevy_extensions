//! Ponte de integração com o host — o contrato mínimo entre o runtime de
//! terreno e os serviços que ficaram no monólito Viber.
//!
//! Esta crate é o runtime de terreno extraído; tudo o que ela lê/insere do
//! resto da engine está aqui, com a semântica EXATA do original (cópias
//! fiéis onde o item é pequeno e terreno-acoplado, contratos de leitura
//! onde o estado é do host). Cada item traz um `TODO(viber-exit)` com o
//! que o host tem de fazer na onda de integração:
//!
//! * **Cena** — [`SceneEntity`]/[`Persistent`]/[`SceneLoad`]/[`RootSnapshot`]
//!   mais os sistemas [`snapshot_roots`]/[`mark_bootstrap_roots`]: o
//!   `scene_loader` do host tem de ADOTAR estes tipos (um só tipo
//!   `SceneEntity`, senão o descarregamento da cena não vê as raízes do
//!   terreno) e correr os sistemas de snapshot à volta do bootstrap.
//! * **Âncoras de streaming** — [`EditorCamera`] + [`active_camera_eye`] e
//!   [`StreamingBoost`]: o host adota (reexporta) os seus.
//! * **Swimmers** — [`EnemyCreature`]: o host adota o tipo de `ai.rs` para
//!   o filtro de salpicos acompanhar as criaturas reais.
//! * **Colliders voxel** — [`VoxelCollider`]/[`ColumnColliderBake`]/…
//!   mantêm a API aqui; o `stream_voxel_colliders` do host passa a ler
//!   ESTES tipos.
//! * **Partículas** — [`ParticleSpec`]/[`AmbientFx`]/[`spawn_ambient_marker`]:
//!   o sistema de FX ambientes do host adota os tipos; os bursts passam
//!   pela fila [`TerrainBurstQueue`] até o host pontuar.
//! * **Céu/tempo** — [`TerrainSkyState`]: o host publica por frame a partir
//!   do seu `worldsys` (daylight, sol, horizonte, chuva).
//! * **Assets do mundo** — [`TerrainWorldAssets`]: o host publica as roots
//!   (`GameConfig::asset_roots`) e a pasta de texturas do pool quando o
//!   mundo carrega.
//! * **Mundo estático** — [`StaticWorld`]: o host publica o modo
//!   (`<world static="true">`).

use std::path::{Path, PathBuf};

use bevy::ecs::schedule::ScheduleLabel;
use bevy::prelude::*;

// ── cena (contratos do scene_loader) ─────────────────────────────────────
// TODO(viber-exit): o `scene_loader` do monólito define os MESMOS tipos —
// na integração ele passa a importá-los daqui (e a correr estes dois
// sistemas à volta do bootstrap), senão as raízes do terreno não morrem na
// troca de cena e duplicam-se a cada regresso.

/// Entidade (raiz) que pertence à cena carregada: morre no próximo descarregamento.
#[derive(Component, Debug, Default, Clone, Copy, Reflect)]
#[reflect(Component)]
#[type_path = "viber::scene_loader"]
pub struct SceneEntity;

/// Entidade que sobrevive a trocas de cena (autoloads, câmara de editor, sonda IBL…).
#[derive(Component, Debug, Default, Clone, Copy, Reflect)]
#[reflect(Component)]
#[type_path = "viber::scene_loader"]
pub struct Persistent;

/// O schedule que (re)carrega uma cena: os mesmos sistemas do `Startup` que montam o mundo.
#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
pub struct SceneLoad;

/// Set que o HOST preenche com o arranque do spawn declarativo — o análogo
/// de `recipes::spawn::startup` do monólito.
///
/// TODO(viber-exit): o Viber insere o seu `spawn::startup` neste set (e
/// publica o [`PendingTerrain`] antes dele). Sem o set registado, a
/// ordenação é um no-op e a ordem de listagem mantém-se — o comportamento
/// de apps mínimas de teste é o mesmo.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HostSpawn;

/// Raízes existentes antes do bootstrap do terreno (ver [`mark_bootstrap_roots`]).
#[derive(Resource, Debug, Default, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::scene_loader"]
pub struct RootSnapshot(pub std::collections::HashSet<Entity>);

/// Antes do bootstrap do terreno: fotografa as raízes.
pub fn snapshot_roots(world: &mut World) {
    let roots = world
        .query_filtered::<Entity, Without<ChildOf>>()
        .iter(world)
        .collect();
    world.insert_resource(RootSnapshot(roots));
}

/// Depois do bootstrap: tudo o que ele pôs na raiz (água, estradas, marcadores de FX da
/// espuma/névoa…) é da cena — sem isto cada regresso à cena duplicava-os.
pub fn mark_bootstrap_roots(world: &mut World) {
    let Some(mut snapshot) = world.get_resource_mut::<RootSnapshot>() else {
        return;
    };
    let before = std::mem::take(&mut snapshot.0);
    if before.is_empty() {
        return;
    }
    let fresh: Vec<Entity> = world
        .query_filtered::<Entity, (Without<ChildOf>, Without<Persistent>)>()
        .iter(world)
        .filter(|entity| !before.contains(entity))
        .collect();
    for entity in fresh {
        world.entity_mut(entity).insert(SceneEntity);
    }
}

/// O mundo carregado é estático (`<world static="true">`) — lido pelo spawn incremental
/// e pelo bootstrap do terreno (sem heightmap em mundo estático = erro, não procedural).
///
/// TODO(viber-exit): publicado pelo host a partir do projeto.
#[derive(Resource, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::recipes::spawn"]
pub struct StaticWorld(pub bool);

// ── âncoras de streaming ─────────────────────────────────────────────────

/// Marker da entidade câmara de editor (`Camera3d` + `Transform`).
///
/// TODO(viber-exit): cópia de `viber::editor_camera::EditorCamera` — o host
/// adota este tipo (o TypePath está congelado), senão o streaming de colunas
/// não vê a câmara de editor no modo `--camera free`.
#[derive(Debug, Component, Reflect)]
#[reflect(Component)]
#[type_path = "viber::editor_camera"]
pub struct EditorCamera;

/// O olho activo do momento: a câmara de editor quando existe (modo free),
/// senão a primeira `Camera3d` (a do jogo). Quem segue "a câmara" — streaming
/// de colunas, culling/LOD — TEM de usar isto: com `--camera free` há DUAS
/// `Camera3d` no mundo (a de jogo inactiva) e `iter().next()` devolve uma
/// arbitrária.
pub fn active_camera_eye<'w>(
    editor: &'w Query<&GlobalTransform, With<EditorCamera>>,
    cameras: &'w Query<&GlobalTransform, With<Camera3d>>,
) -> Option<&'w GlobalTransform> {
    editor.iter().next().or_else(|| cameras.iter().next())
}

/// Orçamentos de streaming (terreno, LOD de malhas) multiplicados — vale
/// enquanto uma cinemática espera em negro por um destino distante ou na
/// janela pós-respawn.
///
/// TODO(viber-exit): cópia de `viber::cinematic::StreamingBoost` — o host
/// adota (o TypePath está congelado); enquanto não adotar, os cortes de
/// cinemática não aceleram o streaming.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq, Reflect)]
#[reflect(Resource)]
#[type_path = "viber::cinematic"]
pub struct StreamingBoost(pub bool);

// ── swimmers (contrato de IA para os FX de água) ─────────────────────────

/// Behaviour state of one [`EnemyCreature`].
///
/// TODO(viber-exit): cópia de `viber::ai::EnemyState` — o host adota.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Reflect)]
#[type_path = "viber::ai"]
pub enum EnemyState {
    /// Patrol around `home`; the [`Default`].
    #[default]
    Wander,
    /// Viu ou ouviu: vira-se para o ponto, ainda sem correr.
    Alert,
    /// Run at the player while it can see them.
    Chase,
    /// Perdeu o alvo: vai ao último ponto e desiste.
    Search,
}

/// A spawned creature: authored stats plus its current behaviour state.
///
/// Só o TIPO interessa ao runtime (filtro de `tag_swimmers` nos FX de água);
/// os campos são a cópia fiel do `viber::ai::EnemyCreature` para o TypePath
/// e o `Default` baterem certo quando o host adota.
///
/// TODO(viber-exit): o host adota este tipo no seu sistema de IA.
#[derive(Debug, Clone, Component, Reflect)]
#[reflect(Component, Default)]
#[type_path = "viber::ai"]
pub struct EnemyCreature {
    /// Chase speed in m/s (wander runs below it).
    pub speed: f32,
    /// Player distance that flips Wander → Chase.
    pub aggro_radius: f32,
    /// Chase stops (attack range) once this close to the player.
    pub attack_radius: f32,
    /// Patrol anchor (XZ); `None` until the AI latches it to the spawn
    /// position on the first tick.
    pub home: Option<Vec2>,
    pub state: EnemyState,
}

impl Default for EnemyCreature {
    fn default() -> Self {
        Self {
            speed: 2.0,
            aggro_radius: 18.0,
            attack_radius: 2.2,
            home: None,
            state: EnemyState::Wander,
        }
    }
}

// ── colliders voxel (contrato do physics.rs do host) ─────────────────────
// TODO(viber-exit): o `stream_voxel_colliders` do monólito passa a ler ESTES
// tipos (`VoxelCollider` na coluna, `TerrainCollisionStatus` continua do
// lado do host).

/// Raio da banda de colisão em colunas (`chunk_size ×` isto, limitado à banda LOD 0).
pub const PHYSICS_CHUNK_RADIUS: f32 = 3.0;

/// Coluna voxel com trimesh de terreno assado — o collider É a mesh.
#[derive(Debug, Component, Reflect)]
#[reflect(Component)]
#[type_path = "viber::physics"]
pub struct VoxelCollider;

/// Banda de colisão em metros para um spec.
///
/// `chunk_size × PHYSICS_CHUNK_RADIUS` limitado à banda LOD 0
/// (`lod_distance`): dentro dela o ladder impõe piso de LOD 0 às colunas,
/// por isso "collider == o que se vê" é exato em todo o lado onde o player
/// consegue tocar — nunca há collider assado de geometria mais grosseira do
/// que a desenhada.
pub fn collision_keep_within(spec: &crate::spec::TerrainSpec) -> f32 {
    (spec.chunk_size * PHYSICS_CHUNK_RADIUS).min(spec.lod_distance())
}

/// Distância XZ de um ponto ao AABB de uma coluna.
///
/// Às coordenadas da grelha e não ao `Transform` da coluna: a entidade senta-se
/// na origem do mundo e a pilha de caixas é alta — medir em 3D ao centro
/// deixava cair o collider debaixo dos pés do herói quando ele está no topo.
pub fn column_xz_distance(p: Vec3, coords: bevy::math::UVec2, half: f32, edge: f32) -> f32 {
    let x0 = -half + coords.x as f32 * edge;
    let z0 = -half + coords.y as f32 * edge;
    let cx = p.x.clamp(x0, x0 + edge);
    let cz = p.z.clamp(z0, z0 + edge);
    ((p.x - cx) * (p.x - cx) + (p.z - cz) * (p.z - cz)).sqrt()
}

/// Menor distância XZ de QUALQUER âncora (todos os heróis) ao AABB da coluna.
///
/// O streaming escolhia só o primeiro player: em co-op o segundo não tinha
/// collider de terreno debaixo dos pés (caía pelo mundo) e o `ready` global
/// mentia — dizia "há chão" porque o primeiro estava numa coluna assada.
/// `INFINITY` quando não há âncoras.
pub fn nearest_column_distance(
    points: &[Vec3],
    coords: bevy::math::UVec2,
    half: f32,
    edge: f32,
) -> f32 {
    points
        .iter()
        .map(|point| column_xz_distance(*point, coords, half, edge))
        .fold(f32::INFINITY, f32::min)
}

/// Vértices/índices triangulados de um `Mesh` TriangleList (índices U16/U32
/// ou não-indexado — cada três vértices é um triângulo).
pub fn mesh_vertices_indices(mesh: &Mesh) -> Option<(Vec<Vec3>, Vec<[u32; 3]>)> {
    use bevy::mesh::VertexAttributeValues;
    use bevy::render::mesh::{Indices, PrimitiveTopology};

    if mesh.primitive_topology() != PrimitiveTopology::TriangleList {
        return None;
    }
    let vertices: Vec<Vec3> = match mesh.attribute(Mesh::ATTRIBUTE_POSITION)? {
        VertexAttributeValues::Float32x3(values) => values.iter().map(|v| Vec3::from(*v)).collect(),
        _ => return None,
    };
    let indices: Vec<[u32; 3]> = match mesh.indices() {
        Some(Indices::U16(idx)) => idx
            .as_chunks::<3>()
            .0
            .iter()
            .map(|i| [i[0] as u32, i[1] as u32, i[2] as u32])
            .collect(),
        Some(Indices::U32(idx)) => idx
            .as_chunks::<3>()
            .0
            .iter()
            .map(|i| [i[0], i[1], i[2]])
            .collect(),
        // Un-indexed: every three vertices are one triangle.
        None => (0..vertices.len() as u32 / 3)
            .map(|t| [t * 3, t * 3 + 1, t * 3 + 2])
            .collect(),
    };
    if indices.is_empty() {
        return None;
    }
    Some((vertices, indices))
}

/// Triângulos acumulados das caixas de UMA coluna voxel, prontos para assar.
///
/// Os buffers já existem — são exatamente os que o transvoxel devolve para
/// render ([`crate::voxel::build_box_mesh`]) — acumulá-los não paga
/// memória extra além do bake em curso. Posições são origin-relative nos
/// três eixos (o contracto de [`crate::voxel::spawn_box_entity`]),
/// por isso cada caixa entra com o próprio `origin` como offset.
#[derive(Debug, Default)]
pub struct ColumnColliderBake {
    vertices: Vec<Vec3>,
    indices: Vec<[u32; 3]>,
}

impl ColumnColliderBake {
    pub fn new() -> Self {
        Self::default()
    }

    /// Caminho quente: os buffers que o mesher acabou de produzir.
    pub fn add_mesh_data(&mut self, origin: Vec3, data: &bevy_voxel_terrain::ChunkMeshData) {
        let base = self.vertices.len() as u32;
        self.vertices
            .extend(data.positions.iter().map(|p| origin + Vec3::from(*p)));
        for tri in data.indices.as_chunks::<3>().0 {
            self.indices
                .push([tri[0] + base, tri[1] + base, tri[2] + base]);
        }
    }

    /// Caminho de reparação: ler de volta um `Mesh` já residente no asset
    /// store (CPU-resident por construção, `to_bevy_mesh`).
    pub fn add_bevy_mesh(&mut self, origin: Vec3, mesh: &Mesh) {
        let Some((vertices, indices)) = mesh_vertices_indices(mesh) else {
            return;
        };
        let base = self.vertices.len() as u32;
        self.vertices
            .extend(vertices.into_iter().map(|p| origin + p));
        for tri in indices {
            self.indices
                .push([tri[0] + base, tri[1] + base, tri[2] + base]);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len()
    }

    /// Assa o trimesh da coluna.
    ///
    /// `FIX_INTERNAL_EDGES` (que implica merge de vértices duplicados) povoa
    /// pseudo-normais: sem elas o parry devolve normal de contacto por
    /// triângulo e o character controller engancha em cada aresta interna —
    /// o clássico ghost collision de trimesh. Sem fallback convex-hull: um
    /// hull de uma coluna de 64 m seria um bloco mentiroso; bake falhado é
    /// warn + coluna sem collider, e a rede de segurança do player cobre.
    pub fn bake(&self) -> Option<bevy_rapier3d::prelude::Collider> {
        if self.is_empty() {
            return None;
        }
        match bevy_rapier3d::prelude::Collider::trimesh_with_flags(
            self.vertices.clone(),
            self.indices.clone(),
            bevy_rapier3d::prelude::TriMeshFlags::FIX_INTERNAL_EDGES
                | bevy_rapier3d::prelude::TriMeshFlags::DELETE_DEGENERATE_TRIANGLES,
        ) {
            Ok(collider) => Some(collider),
            Err(err) => {
                bevy::log::warn!(
                    "terrain collider: bake do trimesh da coluna falhou ({err}) — sem collider"
                );
                None
            }
        }
    }
}

// ── partículas (contrato do recipes/particles do host) ───────────────────

/// Emitter config de `<ParticleSystem>`: um preset mais os overrides
/// `particle-emitter="…"` encontrados no mundo.
///
/// TODO(viber-exit): cópia de `viber::recipes::ParticleSpec` (TypePath
/// congelado). O sistema de partículas do host adota ESTE tipo — senão os
/// emissores de espuma/névoa do terreno não são montados por ele.
#[derive(Debug, Clone, Reflect)]
#[type_path = "viber::recipes"]
pub struct ParticleSpec {
    pub preset: String,
    pub emission_rate: Option<f32>,
    pub life: Option<(f32, f32)>,
    pub speed: Option<(f32, f32)>,
    pub size: Option<(f32, f32)>,
    pub color: Option<[f32; 3]>,
    /// `shape-radius`: spawn spread around the emitter origin (meters).
    pub shape_radius: Option<f32>,
    pub looping: bool,
    /// Authored `world-space` flag (Viber emitters are always local — the
    /// emitters in this world are static, so behaviour is equivalent).
    pub world_space: bool,
}

/// Marca de emissor ambiente montável (Transform + spec). O sistema de FX
/// ambientes do host monta o bundle de render pela distância da câmara.
///
/// TODO(viber-exit): cópia de `viber::particles::AmbientFx` (TypePath
/// congelado) — o host adota.
#[derive(Component, Reflect)]
#[reflect(Component)]
#[type_path = "viber::particles"]
pub struct AmbientFx {
    pub spec: ParticleSpec,
}

/// Enfileira um burst de partículas pedido pelo runtime (salpicos de água).
///
/// TODO(viber-exit): ponte transitória com `viber::particles::spawn_burst`
/// (o pool de partículas é do host). O host registra um sistema que drena
/// esta fila e chama o seu `spawn_burst`; sem a ponte os bursts não emitem
/// (os anéis de ripple do terreno continuam a funcionar — são desenhados
/// aqui).
#[derive(Debug, Clone, Resource, Default)]
pub struct TerrainBurstQueue(pub Vec<TerrainBurst>);

/// Um burst pendente (ver [`TerrainBurstQueue`]).
#[derive(Debug, Clone)]
pub struct TerrainBurst {
    pub spec: ParticleSpec,
    pub position: Vec3,
    pub count: usize,
}

/// Assinatura compatível com `viber::particles::spawn_burst`: enfileira o
/// pedido em [`TerrainBurstQueue`] (auto-init, como o pool do host).
pub fn spawn_burst(commands: &mut Commands, spec: &ParticleSpec, position: Vec3, count: usize) {
    let burst = TerrainBurst {
        spec: spec.clone(),
        position,
        count,
    };
    commands.queue(move |world: &mut World| {
        if !world.contains_resource::<TerrainBurstQueue>() {
            world.init_resource::<TerrainBurstQueue>();
        }
        world.resource_mut::<TerrainBurstQueue>().0.push(burst);
    });
}

/// Spawna um marker de emissor ambiente: o host monta o bundle de render
/// (mesh reciclada do pool, material partilhado) a partir do [`AmbientFx`].
pub fn spawn_ambient_marker(world: &mut World, spec: &ParticleSpec, position: Vec3) -> Entity {
    world
        .spawn((
            Transform::from_translation(position),
            AmbientFx { spec: spec.clone() },
            Name::new("fx:ambient"),
        ))
        .id()
}

// ── céu/tempo (vista de leitura do worldsys do host) ─────────────────────

/// Vista de leitura do estado céu/tempo que o runtime consome: só os
/// valores que o tint de dia/noite e a chuva do terreno leem.
///
/// TODO(viber-exit): o host publica esta resource a cada frame a partir do
/// seu `worldsys` (`DayCycleState::visual_daylight`,
/// `AtmosphereState.sun_dir`/`horizon`, `WeatherState.rain`). Enquanto não
/// publicar, os sistemas aplicam os MESMOS fallbacks de antes (dia pleno,
/// sem publicação de atmosfera, seco) — o comportamento de um mundo sem
/// `<DayCycle>`/`<Weather>`.
#[derive(Debug, Clone, Resource, Reflect)]
#[reflect(Resource)]
pub struct TerrainSkyState {
    /// Fator de luz visual (0 = noite, 1 = dia pleno).
    pub daylight: f32,
    /// Direção PARA o sol (mundo, Y-up; a astronómica, pode estar abaixo).
    pub sun_dir: Vec3,
    /// Cor do céu no horizonte (linear).
    pub horizon: [f32; 3],
    /// Intensidade da chuva (0..1).
    pub rain: f32,
}

impl Default for TerrainSkyState {
    fn default() -> Self {
        Self {
            daylight: 1.0,
            sun_dir: Vec3::Y,
            horizon: [1.0, 1.0, 1.0],
            rain: 0.0,
        }
    }
}

// ── assets do mundo (vista do config.yaml do jogo) ───────────────────────

/// Recorte do `config.yaml` do jogo que o runtime de terreno lê: as roots
/// de assets (já resolvidas com a pasta do mundo e o `VIBER_ASSET_POOL`)
/// e a pasta das texturas do pool de terreno.
///
/// TODO(viber-exit): o host publica esta resource quando o mundo carrega —
/// análogo de `GameConfig::asset_roots(world_dir)` + `assets.terrain_textures_dir`.
/// Enquanto não publicar, `asset_roots()` devolve vazio (só a pasta do
/// mundo) e os paths de textura resolvem contra a pasta do mundo — igual ao
/// comportamento de uma app mínima sem config.yaml.
#[derive(Debug, Clone, Resource, Default, Reflect)]
#[reflect(Resource)]
pub struct TerrainWorldAssets {
    /// Roots em ordem de precedência: a pasta do mundo SEMPRE primeiro,
    /// depois as extras do config.
    pub roots: Vec<PathBuf>,
    /// `assets.terrain_textures_dir` — pasta do pool de texturas de terreno.
    pub textures_dir: PathBuf,
}

impl TerrainWorldAssets {
    /// Roots de asset para um mundo: a pasta do jogo/mundo primeiro (os
    /// overrides por-mundo), depois as extras.
    pub fn asset_roots(&self, world_dir: &Path) -> Vec<PathBuf> {
        let mut roots = vec![world_dir.to_path_buf()];
        for extra in &self.roots {
            if !roots.contains(extra) {
                roots.push(extra.clone());
            }
        }
        roots
    }

    /// Path de asset (relativo a uma pasta base) como string para o
    /// AssetServer. Diretório vazio NÃO pode gerar `"/x.ogg"` — o path
    /// absoluto fura a root e o asset nunca carrega.
    fn asset_string(dir: &Path, rel: &str) -> String {
        let dir = dir.to_string_lossy();
        let dir = dir.trim_end_matches('/');
        if dir.is_empty() {
            rel.to_string()
        } else {
            format!("{dir}/{rel}")
        }
    }

    /// Layer de terreno (`grass`) → path de albedo; None se o valor não é
    /// um alias mas um caminho cru de textura.
    pub fn terrain_albedo(&self, layer: &str) -> Option<String> {
        crate::splat::pool_albedo(layer).map(|rel| Self::asset_string(&self.textures_dir, &rel))
    }

    /// O mesmo para a normal map.
    pub fn terrain_normal(&self, layer: &str) -> Option<String> {
        crate::splat::pool_normal(layer).map(|rel| Self::asset_string(&self.textures_dir, &rel))
    }

    /// O mesmo para o height map (escalar, height-blend do chunk).
    pub fn terrain_height(&self, layer: &str) -> Option<String> {
        crate::splat::pool_height(layer).map(|rel| Self::asset_string(&self.textures_dir, &rel))
    }

    /// O mesmo para o AO map (escalar, oclusão por texel do chunk).
    pub fn terrain_ao(&self, layer: &str) -> Option<String> {
        crate::splat::pool_ao(layer).map(|rel| Self::asset_string(&self.textures_dir, &rel))
    }

    /// O mesmo para o roughness map (micro-variação do brilho especular).
    pub fn terrain_roughness(&self, layer: &str) -> Option<String> {
        crate::splat::pool_roughness(layer).map(|rel| Self::asset_string(&self.textures_dir, &rel))
    }

    /// O mesmo para o SMOOTHNESS map (algumas superfícies do pool publicam
    /// smoothness; o material inverte).
    pub fn terrain_smoothness(&self, layer: &str) -> Option<String> {
        crate::splat::pool_smoothness(layer).map(|rel| Self::asset_string(&self.textures_dir, &rel))
    }

    /// Fixture de teste: o layout do pool por omissão (`assets/textures`),
    /// sem roots extra — os testes de paths juntam as suas próprias roots.
    /// (O `GameConfig::fixture` do monólito usa o mesmo valor.)
    pub fn fixture() -> Self {
        Self {
            roots: Vec::new(),
            textures_dir: PathBuf::from("assets/textures"),
        }
    }
}

// ── escrita atómica (utilitário do scene::doc do host) ───────────────────

/// Escrita atómica com backup: escreve num `.tmp` ao lado (com `sync_all` —
/// o mesmo padrão do save e do lease de sessão), roda o `.bak` anterior e faz
/// rename — um crash nunca deixa um ficheiro a meio.
/// TODO(viber-exit): cópia de `viber::scene::doc::write_atomic`.
pub fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    // Sufixo no NOME COMPLETO, não `with_extension`: `world.xml` e
    // `world.json` na mesma pasta partilhavam `world.tmp`/`world.bak` e uma
    // escrita podia pisar a outra.
    let with_suffix = |suffix: &str| {
        let mut name = path
            .file_name()
            .map(std::ffi::OsStr::to_os_string)
            .unwrap_or_default();
        name.push(suffix);
        path.with_file_name(name)
    };
    let tmp = with_suffix(".tmp");
    // `std::fs::write` não sincroniza: o rename podia ser durável com o
    // conteúdo ainda só em cache de página — a queda a seguir deixava o
    // ficheiro novo truncado/zero com o .bak bom. Um crash durante a escrita
    // do `.tmp` também não pode o deixar órfão a meio.
    let write_tmp = || -> anyhow::Result<()> {
        // `with_context` do anyhow é ambíguo com o `ContextExt` do prelude
        // do bevy 0.20 — o contexto vai por `map_err`.
        let mut file = std::fs::File::create(&tmp)
            .map_err(|e| anyhow::anyhow!("a criar {}: {e}", tmp.display()))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| anyhow::anyhow!("a escrever {}: {e}", tmp.display()))?;
        Ok(())
    };
    if let Err(error) = write_tmp() {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    let bak = with_suffix(".bak");
    if path.exists() {
        let _ = std::fs::rename(path, &bak);
    }
    if let Err(error) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(error).map_err(|e| {
            anyhow::anyhow!("a promover {} → {}: {e}", tmp.display(), path.display())
        });
    }
    Ok(())
}

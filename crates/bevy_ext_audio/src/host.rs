//! Contratos com o HOST — o que no monólito vinha da engine e aqui entra por
//! recursos que o host publica.
//!
//! O crate não conhece o jogo nem o monólito: `config::GameConfig`
//! (`sfx_dir`), `worldsys::WeatherState` (chuva), `scene_loader::CurrentScene`
//! (id da cena), `ambient::CurrentBiome` (bioma) e `terrain::runtime::
//! TerrainRuntime` (corpos de água) tornaram-se [`AudioPaths`],
//! [`AudioWorldState`] e [`HostWaterBodies`].
//!
//! TODO(viber-exit): o adaptador do Viber — publicar [`AudioWorldState`] por
//! frame a partir do `WeatherState`/`CurrentScene`/`CurrentBiome`, construir
//! o [`WaterBodies`] sobre o `TerrainRuntime` e marcar a câmara de editor com
//! [`AudioEar`] — é uma peça pequena que fica na engine quando ela adoptar
//! este crate.

use bevy::camera::Camera3d;
use bevy::prelude::*;

// ── paths de assets ─────────────────────────────────────────────────────

/// Id da cena principal (`bevy_world_xml::project::MAIN_SCENE`). Uma cena com
/// este id não injeta a sua layer de BGM — a zona manda é a posição no mundo
/// aberto (ver [`crate::music::scene_zone`]).
pub const MAIN_SCENE: &str = "main";

/// Diretório dos clips SFX do jogo (o `assets.sfx_dir` do `config.yaml`).
///
/// No Viber era lido do `GameConfig`; aqui o host insere o recurso (o plugin
/// semeia o default histórico `assets/audio/sfx`). [`crate::sfx::SfxHandles`]
/// e [`crate::music::script_sound_loops`] resolvem os paths contra ele.
#[derive(Debug, Clone, Resource, Reflect)]
#[reflect(Resource)]
pub struct AudioPaths {
    /// Diretório base dos clips, relativo às roots de assets.
    pub sfx_dir: String,
}

impl Default for AudioPaths {
    fn default() -> Self {
        Self {
            sfx_dir: "assets/audio/sfx".into(),
        }
    }
}

impl AudioPaths {
    /// Path de asset (relativo ao `sfx_dir`) como string para o AssetServer.
    ///
    /// Diretório vazio (`sfx_dir: ""`, áudio na raiz do jogo) NÃO pode gerar
    /// `"/x.ogg"` — o path absoluto fura a root e o asset nunca carrega.
    fn asset_string(dir: &str, rel: &str) -> String {
        let dir = dir.trim_end_matches('/');
        if dir.is_empty() {
            rel.to_string()
        } else {
            format!("{dir}/{rel}")
        }
    }

    /// Clip SFX (`combat/swing.ogg`, relativo ao `sfx_dir`) → path
    /// AssetServer.
    pub fn sfx_path(&self, clip: &str) -> String {
        Self::asset_string(&self.sfx_dir, clip)
    }
}

// ── estado do mundo publicado pelo host ─────────────────────────────────

/// Entradas do mundo que o áudio lê e que vivem no host.
///
/// No Viber eram três recursos da engine (`worldsys::WeatherState.rain`,
/// `scene_loader::CurrentScene.id`, `ambient::CurrentBiome.id`); o host
/// publica o espelho neste recurso. **Não é inicializado pelo plugin** — a
/// ausência é o sinal "sem clima/cena/bioma" e trava o loop de chuva e a
/// zona de bioma tal como os `Option<Res<…>>` originais.
#[derive(Debug, Clone, Resource, Default, Reflect)]
#[reflect(Resource)]
pub struct AudioWorldState {
    /// Intensidade contínua da chuva (`WeatherState.rain`, 0..1).
    pub rain: f32,
    /// Id da cena carregada (`CurrentScene.id`; `None` = fora de projeto).
    pub scene_id: Option<String>,
    /// Bioma atual (`CurrentBiome.id`; `None` = vale central).
    pub biome_id: Option<String>,
}

// ── corpos de água (trait) ──────────────────────────────────────────────

/// Tipo de corpo de água (o `terrain::WaterKind` do Viber, reduzido ao que o
/// áudio distingue: lago OU rio).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaterKind {
    Lake,
    River,
}

/// Uma cachoeira registada: o caldeirão (world XZ) e a queda (m).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaterfallSpot {
    pub at: [f32; 2],
    pub drop: f32,
}

/// Consulta aos corpos de água do terreno, sem o crate depender do runtime
/// do terreno.
///
/// O host (Viber: o adaptador do `TerrainRuntime`; Vael: o seu mundo) publica
/// uma implementação em [`HostWaterBodies`]. Sem recurso os loops de água não
/// nascem nem mexem — igual aos apps sem terreno.
pub trait WaterBodies: Send + Sync + 'static {
    /// Menor distância (m, ≥ 0) à linha de água entre os corpos do tipo
    /// `kind`, a partir de `(x, z)`; `None` = nenhum corpo desse tipo.
    fn min_distance_to_waterline(&self, kind: WaterKind, x: f32, z: f32) -> Option<f32>;

    /// Cachoeiras com loop próprio (`WaterBody.cascades` com `waterfall`
    /// no Viber — uma por queda grande, já com a posição do caldeirão).
    fn waterfall_spots(&self) -> &[WaterfallSpot];
}

/// A implementação de [`WaterBodies`] do host (`None` = mundo sem terreno).
#[derive(Clone, Resource, Default)]
pub struct HostWaterBodies(pub Option<std::sync::Arc<dyn WaterBodies>>);

impl std::fmt::Debug for HostWaterBodies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("HostWaterBodies")
            .field(&self.0.is_some())
            .finish()
    }
}

// ── ouvido ──────────────────────────────────────────────────────────────

/// Marker da câmara PREFEFENCIAL para posição do ouvinte (no Viber, a câmara
/// de editor do modo `--camera free`).
///
/// Com o modo editor há DUAS `Camera3d` no mundo (a de jogo inactiva) e
/// `iter().next()` devolve uma arbitrária — a atenuação de SFX e os loops de
/// script mediriam a distância à câmara errada. [`ear_position`] prefere a
/// entidade com este marker; sem marker nenhum (jogo normal, uma câmara) o
/// comportamento é o de sempre.
///
/// TODO(viber-exit): o Viber insere este marker na `EditorCamera`.
#[derive(Debug, Clone, Copy, Component, Reflect)]
#[reflect(Component)]
pub struct AudioEar;

/// A posição do ouvinte do momento: a câmara com [`AudioEar`] quando existe,
/// senão a primeira `Camera3d` (a do jogo). Quem atenua por distância —
/// [`crate::sfx::sfx_player_system`], [`crate::music::script_sound_loops`] —
/// TEM de usar isto.
pub fn ear_position<'w>(
    ear: &'w Query<&GlobalTransform, With<AudioEar>>,
    cameras: &'w Query<&GlobalTransform, With<Camera3d>>,
) -> Option<&'w GlobalTransform> {
    ear.iter().next().or_else(|| cameras.iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O gotcha histórico do `asset_string`: diretório vazio não pode gerar
    /// `/x.ogg` (path absoluto fura a root e o asset nunca carrega) e a
    /// barra final do diretório não duplica.
    #[test]
    fn test_sfx_path_empty_dir_and_trailing_slash() {
        let empty = AudioPaths {
            sfx_dir: String::new(),
        };
        assert_eq!(empty.sfx_path("combat/swing.ogg"), "combat/swing.ogg");
        let slash = AudioPaths {
            sfx_dir: "assets/audio/sfx/".into(),
        };
        assert_eq!(
            slash.sfx_path("combat/swing.ogg"),
            "assets/audio/sfx/combat/swing.ogg"
        );
        // default histórico do config.yaml do Viber
        assert_eq!(
            AudioPaths::default().sfx_path("combat/swing.ogg"),
            "assets/audio/sfx/combat/swing.ogg"
        );
    }
}

//! Predefinições de cerca: o que um [`FenceSpec`] referencia por chave.
//!
//! Cada predefinição é um [`FenceStyle`] — uma entidade de
//! `world/settings/fences.bsn` sem `WorldNode`, editável campo a campo no
//! inspector do Jackdaw. As dez pré-definidas aqui são os valores com que esse
//! ficheiro é gerado; o jogo só precisa das chaves.
//!
//! As medidas seguem a cerca que o Vael já tinha (`farm/fence_segment`:
//! 3,0 m entre postes, 1,2 m de altura, duas travessas) e a arte de
//! *Ofício Talhado* (`docs/ART.md`): madeira desbastada a enxó com chanfros
//! francos, nada de arestas derretidas.

use std::sync::OnceLock;

use bevy::prelude::*;
use bevy_ext_scene::FenceSpec;

/// Tipo de cerca — que peças a linha gera.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[reflect(Default, Debug, Clone, Hash, PartialEq)]
#[type_path = "bevy_ext_fence"]
pub enum FenceKind {
    /// Postes e travessas horizontais encastradas (pastagem).
    #[default]
    Rail,
    /// Travessas com tábuas verticais pregadas, topos desiguais.
    Plank,
    /// Estacas estreitas de topo em ponta.
    Picket,
    /// Troncos apertados, mais altos, com topos irregulares.
    Palisade,
    /// Estacas com varas tecidas à frente e atrás (curral, horta).
    Wattle,
    /// Moldura com ripas diagonais cruzadas.
    Lattice,
    /// Travessas com diagonais em X por vão.
    Crossbuck,
    /// Postes curtos com cordas em catenária.
    Rope,
    /// Dois postes e uma folha de tábuas com travessa diagonal.
    Gate,
}

impl FenceKind {
    /// Todos os tipos, pela ordem das predefinições.
    pub const ALL: [FenceKind; 9] = [
        FenceKind::Rail,
        FenceKind::Plank,
        FenceKind::Picket,
        FenceKind::Palisade,
        FenceKind::Wattle,
        FenceKind::Lattice,
        FenceKind::Crossbuck,
        FenceKind::Rope,
        FenceKind::Gate,
    ];

    /// A chave do `fence-kind`.
    pub fn key(self) -> &'static str {
        match self {
            FenceKind::Rail => "rail",
            FenceKind::Plank => "plank",
            FenceKind::Picket => "picket",
            FenceKind::Palisade => "palisade",
            FenceKind::Wattle => "wattle",
            FenceKind::Lattice => "lattice",
            FenceKind::Crossbuck => "crossbuck",
            FenceKind::Rope => "rope",
            FenceKind::Gate => "gate",
        }
    }

    /// Lê a chave do `fence-kind` (sem distinção de maiúsculas).
    pub fn from_key(key: &str) -> Option<Self> {
        let key = key.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|kind| kind.key() == key)
    }
}

/// Secção de uma peça de madeira.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[reflect(Default, Debug, Clone, Hash, PartialEq)]
#[type_path = "bevy_ext_fence"]
pub enum TimberShape {
    /// Quadrada com os cantos chanfrados a enxó (oito facetas).
    #[default]
    Hewn,
    /// Tronco descascado com `sides` facetas.
    Round,
}

/// Topo de uma tábua ou de um poste.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[reflect(Default, Debug, Clone, Hash, PartialEq)]
#[type_path = "bevy_ext_fence"]
pub enum BoardTop {
    #[default]
    Flat,
    /// Cortado em bisel.
    Chamfer,
    /// Ponta de estaca.
    Point,
}

/// Travessa diagonal por vão.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[reflect(Default, Debug, Clone, Hash, PartialEq)]
#[type_path = "bevy_ext_fence"]
pub enum Brace {
    #[default]
    Off,
    /// Uma diagonal, do lado do gonzo para cima.
    Single,
    /// Duas diagonais em X.
    Cross,
}

/// Como as peças horizontais seguem o terreno entre dois postes.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[reflect(Default, Debug, Clone, Hash, PartialEq)]
#[type_path = "bevy_ext_fence"]
pub enum SlopeFollow {
    /// Seguem a rampa (até ~15°); em degraus quando a rampa é forte.
    #[default]
    Rake,
    /// Sempre em degraus: horizontais, à cota do poste mais baixo.
    Step,
}

/// Uma peça de madeira: a secção e o tamanho.
///
/// `width` é a dimensão ao longo de `across` da peça (postes e tábuas: a
/// direção da cerca; travessas: a vertical) e `depth` a espessura de através.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Default)]
#[reflect(Default, Debug, Clone)]
#[type_path = "bevy_ext_fence"]
pub struct Timber {
    pub shape: TimberShape,
    /// Largura da secção (m); em [`TimberShape::Round`] é o diâmetro.
    pub width: f32,
    /// Profundidade da secção (m).
    pub depth: f32,
    /// Chanfro como fracção do menor lado (só em [`TimberShape::Hewn`]).
    pub chamfer: f32,
    /// Facetas de um tronco (só em [`TimberShape::Round`]).
    pub sides: u32,
}

impl Timber {
    pub const fn hewn(width: f32, depth: f32, chamfer: f32) -> Self {
        Self {
            shape: TimberShape::Hewn,
            width,
            depth,
            chamfer,
            sides: 0,
        }
    }

    pub const fn round(diameter: f32, sides: u32) -> Self {
        Self {
            shape: TimberShape::Round,
            width: diameter,
            depth: diameter,
            chamfer: 0.0,
            sides,
        }
    }
}

/// Predefinição de cerca: todas as regras, em metros.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_fence"]
pub struct FenceStyle {
    /// Chave que o `fence-style` referencia.
    pub key: String,
    /// Nome legível (inspector, painel do editor).
    pub label: String,
    pub kind: FenceKind,
    /// Altura ao topo do poste, acima do chão.
    pub height: f32,
    /// Distância máxima entre postes; o último vão divide-se por igual.
    pub spacing: f32,
    pub post: Timber,
    /// Postes de canto, de ponta e de portão mais fortes (×).
    pub corner_scale: f32,
    /// Quantos centímetros o poste desce abaixo do chão (não abre vão em rampa).
    pub bury: f32,
    /// Inclinação e rotação ao acaso de cada poste (graus).
    pub post_tilt_deg: f32,
    /// Travessas horizontais por vão — é este o número que manda
    /// ([`FenceStyle::rail_fractions`]).
    pub rails: u32,
    pub rail: Timber,
    /// Altura de cada travessa como fracção da altura do poste. Com menos
    /// travessas do que alturas ficam as primeiras; com mais, distribuem-se por
    /// igual entre a mais baixa e a mais alta daqui.
    pub rail_heights: Vec<f32>,
    /// Quanto a travessa sai da face do poste (espigão e morta).
    pub tenon: f32,
    pub slope_follow: SlopeFollow,
    /// Tábuas verticais por vão; 0 = caberem pela largura e pelo vão.
    pub boards: u32,
    pub board: Timber,
    /// Comprimento da tábua, mínimo e máximo: cada tábua sorteia dentro.
    pub board_length: Vec2,
    pub board_top: BoardTop,
    /// Vão entre tábuas ou entre troncos da paliçada.
    pub board_gap: f32,
    /// Travessa de remate no topo do tapume.
    pub cap_rail: bool,
    pub brace: Brace,
    /// Ripas da treliça.
    pub slat: Timber,
    /// Distância entre ripas paralelas da treliça.
    pub slat_pitch: f32,
    /// Inclinação das ripas (graus).
    pub slat_angle_deg: f32,
    /// Estacas das varas entretecidas.
    pub stake: Timber,
    /// Distância entre estacas.
    pub stake_pitch: f32,
    /// As varas tecidas.
    pub rod: Timber,
    /// Varas empilhadas.
    pub rod_rows: u32,
    /// A corda (diâmetro); a altura de cada corda vem de `rail_heights`.
    pub rope: Timber,
    /// Flecha da corda como fracção do vão (0,08–0,15).
    pub rope_sag: f32,
    /// Folha do portão aberta (graus; 0 = fechada).
    pub gate_open_deg: f32,
    /// Conjunto de texturas do pool (`assets/textures/<conjunto>/`).
    pub material: String,
    /// Conjunto das varas e cordas; vazio = o mesmo.
    pub secondary_material: String,
    /// Metros por repetição da textura, ao longo do veio.
    pub tile_m: f32,
    /// Altura do desgaste de base (terra e líquen).
    pub weather_height: f32,
    /// Tinta do desgaste de base (multiplica o albedo).
    pub weather_tint: Color,
    /// Variação de tom entre peças (±, linear).
    pub tint_spread: f32,
    /// Semente base da variação; soma à chave da cerca.
    pub seed: u32,
}

impl Default for FenceStyle {
    fn default() -> Self {
        pasture_rail()
    }
}

fn post() -> Timber {
    Timber::hewn(0.14, 0.14, 0.12)
}

fn rail() -> Timber {
    Timber::hewn(0.10, 0.07, 0.14)
}

/// Pastagem: duas travessas de carvalho encastradas — a cerca do Vale.
pub fn pasture_rail() -> FenceStyle {
    FenceStyle {
        key: "pasture_rail".into(),
        label: "Travessas de pastagem".into(),
        kind: FenceKind::Rail,
        height: 1.2,
        spacing: 3.0,
        post: post(),
        corner_scale: 1.25,
        bury: 0.35,
        post_tilt_deg: 2.0,
        rails: 2,
        rail: rail(),
        rail_heights: vec![0.38, 0.78],
        tenon: 0.05,
        slope_follow: SlopeFollow::Rake,
        boards: 0,
        board: Timber::hewn(0.15, 0.028, 0.1),
        board_length: Vec2::new(1.0, 1.15),
        board_top: BoardTop::Chamfer,
        board_gap: 0.012,
        cap_rail: false,
        brace: Brace::Off,
        slat: Timber::hewn(0.05, 0.02, 0.1),
        slat_pitch: 0.18,
        slat_angle_deg: 45.0,
        stake: Timber::round(0.055, 7),
        stake_pitch: 0.5,
        rod: Timber::round(0.028, 5),
        rod_rows: 8,
        rope: Timber::round(0.03, 6),
        rope_sag: 0.1,
        gate_open_deg: 0.0,
        material: "wood_oak_hewn".into(),
        secondary_material: String::new(),
        tile_m: 1.2,
        weather_height: 0.28,
        weather_tint: Color::srgb(0.62, 0.58, 0.47),
        tint_spread: 0.07,
        seed: 51119,
    }
}

/// Curral: três travessas mais baixas e postes mais chegados.
pub fn paddock_3rail() -> FenceStyle {
    FenceStyle {
        key: "paddock_3rail".into(),
        label: "Curral de três travessas".into(),
        height: 1.25,
        spacing: 2.6,
        rails: 3,
        rail_heights: vec![0.3, 0.58, 0.86],
        ..pasture_rail()
    }
}

/// Tapume: tábuas verticais pregadas em duas travessas, topos desiguais e
/// remate no topo.
pub fn plank_board() -> FenceStyle {
    FenceStyle {
        key: "plank_board".into(),
        label: "Tapume de tábuas".into(),
        kind: FenceKind::Plank,
        height: 1.3,
        boards: 18,
        board_length: Vec2::new(0.95, 1.12),
        ..pasture_rail()
    }
}

/// Estacas de topo em ponta sobre duas travessas.
pub fn picket() -> FenceStyle {
    FenceStyle {
        key: "picket".into(),
        label: "Estacas".into(),
        kind: FenceKind::Picket,
        height: 1.0,
        board: Timber::hewn(0.075, 0.022, 0.1),
        board_length: Vec2::new(0.82, 0.9),
        board_top: BoardTop::Point,
        board_gap: 0.055,
        ..pasture_rail()
    }
}

/// Paliçada de pinheiro escuro: troncos apertados com topos em ponta.
pub fn palisade() -> FenceStyle {
    FenceStyle {
        key: "palisade".into(),
        label: "Paliçada".into(),
        kind: FenceKind::Palisade,
        height: 1.9,
        spacing: 2.4,
        post: Timber::round(0.17, 8),
        rails: 2,
        rail_heights: vec![0.3, 0.8],
        board: Timber::round(0.14, 8),
        board_gap: 0.012,
        board_top: BoardTop::Point,
        material: "wood_pine_dark".into(),
        ..pasture_rail()
    }
}

/// Esteira de varas: estacas de aveleira com varas tecidas.
pub fn wattle() -> FenceStyle {
    FenceStyle {
        key: "wattle".into(),
        label: "Varas entretecidas".into(),
        kind: FenceKind::Wattle,
        height: 1.0,
        spacing: 2.2,
        post: Timber::round(0.09, 7),
        rails: 0,
        material: "wood_hazel_rod".into(),
        secondary_material: "wood_hazel_rod".into(),
        ..pasture_rail()
    }
}

/// Treliça lavada a cal para hortas e canteiros.
pub fn lattice() -> FenceStyle {
    FenceStyle {
        key: "lattice".into(),
        label: "Treliça".into(),
        kind: FenceKind::Lattice,
        height: 1.5,
        spacing: 2.0,
        rails: 2,
        rail_heights: vec![0.08, 0.97],
        material: "wood_lime_washed".into(),
        ..pasture_rail()
    }
}

/// Cruzetas de pinho de abrigo: travessas com diagonais em X.
pub fn crossbuck() -> FenceStyle {
    FenceStyle {
        key: "crossbuck".into(),
        label: "Cruzetas".into(),
        kind: FenceKind::Crossbuck,
        height: 1.3,
        brace: Brace::Cross,
        material: "wood_shelter_pine".into(),
        ..pasture_rail()
    }
}

/// Cordas de cânamo entre postes curtos.
pub fn rope() -> FenceStyle {
    FenceStyle {
        key: "rope".into(),
        label: "Cordas".into(),
        kind: FenceKind::Rope,
        height: 0.95,
        spacing: 2.4,
        rails: 2,
        rail_heights: vec![0.7, 0.94],
        material: "wood_shelter_pine".into(),
        secondary_material: "rope_hemp".into(),
        ..pasture_rail()
    }
}

/// Portão: dois postes mais altos e a folha de tábuas com travessa diagonal.
pub fn gate() -> FenceStyle {
    FenceStyle {
        key: "gate".into(),
        label: "Portão".into(),
        kind: FenceKind::Gate,
        height: 1.6,
        boards: 9,
        board: Timber::hewn(0.13, 0.03, 0.1),
        material: "wood_oak_hewn".into(),
        ..pasture_rail()
    }
}

/// As dez pré-definidas, pela ordem do painel.
pub fn presets() -> Vec<FenceStyle> {
    vec![
        pasture_rail(),
        paddock_3rail(),
        plank_board(),
        picket(),
        palisade(),
        wattle(),
        lattice(),
        crossbuck(),
        rope(),
        gate(),
    ]
}

/// As pré-definidas do crate, partilhadas (uma alocação).
fn builtin() -> &'static [FenceStyle] {
    static PRESETS: OnceLock<Vec<FenceStyle>> = OnceLock::new();
    PRESETS.get_or_init(presets)
}

/// A predefinição de uma chave (sem distinção de maiúsculas): primeiro as do
/// mundo, depois as do crate — um mundo sem `fences.bsn` ainda resolve.
pub fn find<'a>(styles: &'a [FenceStyle], key: &str) -> Option<&'a FenceStyle> {
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    styles
        .iter()
        .find(|style| style.key.eq_ignore_ascii_case(key))
        .or_else(|| builtin().iter().find(|style| style.key.eq_ignore_ascii_case(key)))
}

impl FenceStyle {
    /// As alturas (fracção da altura da cerca) das travessas que a cerca tem.
    ///
    /// `rails` é o número e `rail_heights` as alturas da predefinição. Um
    /// `fence-rails="3"` numa predefinição de duas alturas dá três travessas —
    /// distribuídas por igual entre a mais baixa e a mais alta — e não duas:
    /// o número pediu-se a sério. Sem alturas não há travessas.
    pub fn rail_fractions(&self) -> Vec<f32> {
        let count = self.rails.max(1) as usize;
        let heights = &self.rail_heights;
        if heights.len() < 2 || heights.len() >= count {
            return heights.iter().copied().take(count).collect();
        }
        let (first, last) = (heights[0], heights[heights.len() - 1]);
        (0..count)
            .map(|index| first + (last - first) * index as f32 / (count - 1) as f32)
            .collect()
    }
}

/// A predefinição com as sobreposições da cerca (`fence-kind`, `fence-height`,
/// `fence-rails`, `fence-boards`, `fence-board-length`, `fence-spacing`,
/// `fence-material`, `fence-seed`, `fence-open`).
pub fn resolve(spec: &FenceSpec, style: &FenceStyle) -> FenceStyle {
    let mut out = style.clone();
    if let Some(kind) = spec.kind.as_deref().and_then(FenceKind::from_key) {
        out.kind = kind;
    }
    if let Some(height) = spec.height {
        out.height = height.max(0.2);
    }
    if let Some(rails) = spec.rails {
        out.rails = rails.min(8);
    }
    if let Some(boards) = spec.boards {
        out.boards = boards.min(200);
    }
    if let Some(length) = spec.board_length {
        let (min, max) = (length.x.max(0.05), length.y.max(0.05));
        out.board_length = Vec2::new(min, max.max(min));
    }
    if let Some(spacing) = spec.spacing {
        out.spacing = spacing.max(0.3);
    }
    if let Some(material) = &spec.material {
        out.material = material.clone();
    }
    if let Some(seed) = spec.seed {
        out.seed = seed;
    }
    // A folha do portão: cada instância abre como quiser (0–150°).
    if let Some(open) = spec.open {
        out.gate_open_deg = open.clamp(0.0, 150.0);
    }
    out
}

/// Definições globais das cercas (uma por mundo).
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "bevy_ext_fence"]
pub struct FenceSettings {
    /// Chave usada quando a cerca não traz `fence-style`.
    pub default_style: String,
    /// Raiz dos conjuntos de texturas (URL do pool).
    pub materials_root: String,
    /// Ferro batido das ferragens do portão.
    pub iron_color: Color,
    /// Quantas cercas gerar por frame no máximo.
    pub batch: u32,
}

impl Default for FenceSettings {
    fn default() -> Self {
        Self {
            default_style: "pasture_rail".into(),
            materials_root: "/assets/textures/".into(),
            iron_color: Color::srgb(0.16, 0.165, 0.17),
            batch: 8,
        }
    }
}

impl FenceSettings {
    /// URL de um mapa de um conjunto (`albedo`, `normal`, `ao`, `roughness`).
    pub fn texture(&self, set: &str, map: &str) -> String {
        format!("{}{}/{map}.ktx2", self.materials_root, set.trim().trim_end_matches('/'))
    }
}

/// Número como o DSL o escreve (`1`, `0.45`) — o texto canónico dos pontos.
pub fn num(value: f32) -> String {
    format!("{value}")
}

/// Regista os tipos para reflexão (jogo, loader e editor).
pub struct FenceSettingsPlugin;

impl Plugin for FenceSettingsPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<FenceStyle>()
            .register_type::<FenceSettings>()
            .register_type::<FenceKind>()
            .register_type::<TimberShape>()
            .register_type::<BoardTop>()
            .register_type::<Brace>()
            .register_type::<SlopeFollow>()
            .register_type::<Timber>()
            .register_type::<Vec<f32>>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cada_preset_tem_a_chave_e_o_tipo_do_nome() {
        let all = presets();
        assert_eq!(all.len(), 10);
        let kinds: std::collections::HashSet<FenceKind> = all.iter().map(|style| style.kind).collect();
        assert_eq!(kinds.len(), FenceKind::ALL.len(), "os nove tipos têm predefinição");
        for style in &all {
            assert_eq!(FenceKind::from_key(style.kind.key()), Some(style.kind));
        }
        // O curral é a segunda travessas: o portão fecha a lista.
        assert_eq!(all[1].kind, FenceKind::Rail);
        assert_eq!(all[9].kind, FenceKind::Gate);
        assert_eq!(FenceKind::from_key("picket "), Some(FenceKind::Picket));
        assert_eq!(FenceKind::from_key("muro"), None);
        assert!(FenceKind::from_key("pasture_rail").is_none(), "a chave da predefinição não é o tipo");
        // Sem `fences.bsn` no mundo, as chaves do crate resolvem na mesma.
        for style in &all {
            assert!(find(&[], &style.key).is_some());
        }
        assert!(find(&[], "").is_none());
        let custom = FenceStyle {
            key: "curral".into(),
            ..paddock_3rail()
        };
        assert_eq!(find(std::slice::from_ref(&custom), "CURRAL").unwrap().key, "curral");
    }

    #[test]
    fn as_sobreposicoes_da_cerca_ganham() {
        let style = pasture_rail();
        let spec = FenceSpec {
            points: "0 0 3 0".into(),
            kind: Some("WATTLE".into()),
            height: Some(2.0),
            rails: Some(1),
            board_length: Some(Vec2::new(1.4, 0.6)),
            spacing: Some(0.1),
            material: Some("wood_swamp_tar".into()),
            ..Default::default()
        };
        let resolved = resolve(&spec, &style);
        assert_eq!(resolved.kind, FenceKind::Wattle);
        assert_eq!(resolved.height, 2.0);
        assert_eq!(resolved.rails, 1);
        // Mínimo acima do máximo: o máximo sobe com ele; o vão nunca é absurdo.
        assert_eq!(resolved.board_length, Vec2::new(1.4, 1.4));
        assert_eq!(resolved.spacing, 0.3);
        assert_eq!(resolved.material, "wood_swamp_tar");
        assert_eq!(resolved.key, style.key);
    }

    #[test]
    fn o_numero_de_travessas_manda_nas_alturas() {
        let near = |got: Vec<f32>, wanted: &[f32]| {
            assert_eq!(got.len(), wanted.len(), "{got:?}");
            assert!(got.iter().zip(wanted).all(|(a, b)| (a - b).abs() < 1e-4), "{got:?}");
        };
        // Mais travessas do que alturas na predefinição: por igual entre a mais baixa e a mais alta.
        let mut spec = FenceSpec { points: "0 0 3 0".into(), rails: Some(3), ..Default::default() };
        near(resolve(&spec, &pasture_rail()).rail_fractions(), &[0.38, 0.58, 0.78]);
        spec.rails = Some(5);
        near(resolve(&spec, &pasture_rail()).rail_fractions(), &[0.38, 0.48, 0.58, 0.68, 0.78]);
        // Menos: ficam as primeiras.
        spec.rails = Some(1);
        near(resolve(&spec, &pasture_rail()).rail_fractions(), &[0.38]);
        // Igual à predefinição: não muda nada.
        near(paddock_3rail().rail_fractions(), &[0.3, 0.58, 0.86]);
        // Sem alturas não há travessas.
        assert!(FenceStyle { rail_heights: Vec::new(), ..pasture_rail() }.rail_fractions().is_empty());
    }

    #[test]
    fn a_abertura_do_portao_e_da_instancia() {
        // Encontrado ao vivo: dois portões com o estilo `gate` nasciam iguais —
        // o ângulo só existia na predefinição.
        let mut spec = FenceSpec { points: "0 0 2.6 0".into(), open: Some(95.0), ..Default::default() };
        assert_eq!(resolve(&spec, &gate()).gate_open_deg, 95.0);
        spec.open = Some(400.0);
        assert_eq!(resolve(&spec, &gate()).gate_open_deg, 150.0, "tem de parar antes de bater no poste");
        spec.open = None;
        assert_eq!(resolve(&spec, &gate()).gate_open_deg, 0.0);
    }

    #[test]
    fn sem_sobreposicao_e_a_predefinicao() {
        let style = wattle();
        let spec = FenceSpec::default();
        assert_eq!(resolve(&spec, &style), style);
    }

    #[test]
    fn o_url_do_mapa_segue_a_convencao_do_pool() {
        let settings = FenceSettings::default();
        assert_eq!(
            settings.texture("wood_oak_hewn", "normal"),
            "/assets/textures/wood_oak_hewn/normal.ktx2"
        );
    }
}

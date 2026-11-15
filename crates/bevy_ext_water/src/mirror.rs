//! Espelhos CPU do campo de ondas: o detalhe das normais (`wave_height_at`)
//! e o deslocamento visível do vértice (`surface_height_at`).

use bevy::math::Vec2;

// ── Espelho CPU do campo de ondas (buoyancy estilo bevy_water) ─────────────
//
// O pacote bevy_water expõe `get_wave_point` — a MESMA função de onda da GPU
// em CPU, para flutuação de corpos. O Viber segue o mesmo contrato: estas
// funções replicam linha a linha o `wave_height`/`fbm`/`value_noise`/
// `hash21` do `water.wgsl` (os parâmetros são os do `WaterSurfaceConfig`),
// para a engine saber onde está a lâmina REAL. Qualquer mudança nas ondas do
// shader TEM de se espelhar aqui — os testes de propriedade abaixo e o
// comentário "espelho linha a linha" são a única guarda (sem harness GPU).
//
// São DOIS espelhos, e não são a mesma cota:
// - [`wave_height_at`] replica o `wave_height` do shader — a família de senos
//   finos que alimenta as NORMAIS/detalhe de superfície;
// - [`surface_height_at`] replica o `gerstner_swell().y` — o deslocamento
//   VERTICAL que o vértice da malha recebe. É este que um corpo flutuante
//   tem de ler: o outro tem amplitudes e fases diferentes e o barco subia
//   numa onda que a GPU não desenha.

/// Hash inteiro do `water.wgsl` — aritmética u32 wrapping; `q` é a célula
/// da grelha (floor em i32, bitcast dois-complementos).
fn wave_hash21(q: [i32; 2]) -> f32 {
    let mut h = (q[0] as u32).wrapping_mul(0x27d4_eb2d) ^ (q[1] as u32).wrapping_mul(0x1656_67b1);
    h = (h ^ (h >> 15)).wrapping_mul(0x2c1b_3c6d);
    h = (h ^ (h >> 13)).wrapping_mul(0x297a_2d39);
    h ^= h >> 16;
    (h & 0x00ff_ffff) as f32 * (1.0 / 16_777_216.0)
}

pub fn wave_value_noise(p: Vec2) -> f32 {
    let i = Vec2::new(p.x.floor(), p.y.floor());
    let f = p - i;
    // Quintic (smootherstep) — igual ao shader.
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let a = wave_hash21([i.x as i32, i.y as i32]);
    let b = wave_hash21([i.x as i32 + 1, i.y as i32]);
    let c = wave_hash21([i.x as i32, i.y as i32 + 1]);
    let d = wave_hash21([i.x as i32 + 1, i.y as i32 + 1]);
    let ab = a + (b - a) * u.x;
    let cd = c + (d - c) * u.x;
    ab + (cd - ab) * u.y
}

pub fn wave_fbm(p: Vec2, drift: Vec2) -> f32 {
    let mut q = p;
    let mut amp = 0.5f32;
    let mut sum = 0.0;
    let mut norm = 0.0;
    for i in 0..4 {
        sum += (wave_value_noise(q + drift * (1.0 + i as f32 * 0.35)) - 0.5) * amp;
        norm += amp;
        // rot = mat2x2(0.80, 0.60, -0.60, 0.80) (colunas), ×2.03 + offset.
        let x = 0.80 * q.x - 0.60 * q.y;
        let y = 0.60 * q.x + 0.80 * q.y;
        q = Vec2::new(x, y) * 2.03 + Vec2::new(11.7, 5.3);
        amp *= 0.5;
    }
    sum / norm
}

/// Altura da onda num ponto do mundo — o `get_wave_point` do bevy_water,
/// espelho CPU do `wave_height` do shader. `wind`/`wind_strength`/`wave_amp`
/// são os valores do `WaterSurfaceConfig` (que também especializam as consts
/// CFG_* do WGSL). Não considera o desvanecimento de margem (mask) — quem
/// flutua está dentro do corpo.
pub fn wave_height_at(p: Vec2, t: f32, wind: [f32; 2], wind_strength: f32, wave_amp: f32) -> f32 {
    let wdir = (Vec2::new(wind[0], wind[1]) + Vec2::new(1e-4, 0.0)).normalize();
    let strength = wind_strength.clamp(0.05, 3.0);
    let speed = 0.7 + 0.5 * strength;

    // DOMAIN WARP — o MESMO do `wave_height` do shader (espelho linha a
    // linha: cristas curvadas em vez de famílias de linhas paralelas).
    let warp = Vec2::new(
        wave_value_noise(p * 0.11 + Vec2::new(t * 0.03, 0.0)),
        wave_value_noise(p * 0.13 - Vec2::new(0.0, t * 0.025)),
    ) * 2.2
        - Vec2::splat(1.1);
    let q = p + warp;

    // Direções giradas em relação ao vento (graus): 0, +34, −51, +73.
    let d0 = wdir;
    let d1 = Vec2::new(
        wdir.x * 0.829 - wdir.y * 0.559,
        wdir.x * 0.559 + wdir.y * 0.829,
    );
    let d2 = Vec2::new(
        wdir.x * 0.629 + wdir.y * 0.777,
        -wdir.x * 0.777 + wdir.y * 0.629,
    );
    let d3 = Vec2::new(
        wdir.x * 0.292 - wdir.y * 0.956,
        wdir.x * 0.956 + wdir.y * 0.292,
    );

    let (k0, k1, k2, k3) = (0.897f32, 1.366, 2.094, 3.396);
    let mut h = 0.0;
    h += (q.dot(d0) * k0 - t * k0.sqrt() * 1.35 * speed).sin() * 0.22;
    h += (q.dot(d1) * k1 + t * k1.sqrt() * 1.10 * speed).sin() * 0.18;
    h += (q.dot(d2) * k2 - t * k2.sqrt() * 0.95 * speed).sin() * 0.12;
    h += (q.dot(d3) * k3 + t * k3.sqrt() * 0.80 * speed).sin() * 0.07;

    let drift = wdir * t * 0.35 * speed;
    // 0.32 — o MESMO peso do termo FBM no `water.wesl` (`wave_height`). Com
    // 0.62 o espelho CPU devolvia quase o dobro da amplitude que a GPU
    // desenha, e um flutuador (a API `get_wave_point`) andava até ~0.2·amp
    // acima/abaixo da lâmina visível.
    h += wave_fbm(q * 0.9 + drift, drift * 0.4) * 0.32 * (0.6 + 0.4 * strength.min(2.0));
    h * wave_amp
}

/// Suavização de Hermite (a mesma do WGSL, para os espelhos não divergirem
/// num ponto de rampa).
fn mirror_smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Altura da SUPERFÍCIE visível num ponto — o espelho CPU do
/// `gerstner_swell(...).y` do `water.wesl`, i.e. o deslocamento vertical que o
/// VÉRTICE da malha de água recebe. É esta a cota que um corpo flutuante sente.
///
/// Não confundir com [`wave_height_at`]: aquele replica o `wave_height` do
/// shader (família de senos e ganho diferentes) e serve o detalhe das normais.
/// Usá-lo para flutuação punha o barco a subir/descer numa onda que a malha
/// não desenha.
///
/// `depth` é a profundidade local da coluna (`surface − bed`, m): a rampa
/// `smoothstep(0.38, 1.35, depth)` é a `deep_swell` do shader — as ondas
/// morrem na praia em vez de balançar um barco encalhado. O `dist_damp` da
/// câmara e a máscara de bordo do mesh ficam de fora (quem flutua está dentro
/// do corpo e perto da câmara). O termo interactivo do solver (`wave_tex`)
/// também não é espelhável sem readback; a amplitude dele é ≤0,10 m.
pub fn surface_height_at(
    p: Vec2,
    t: f32,
    wind: [f32; 2],
    wind_strength: f32,
    wave_amp: f32,
    depth: f32,
) -> f32 {
    let wdir = (Vec2::new(wind[0], wind[1]) + Vec2::new(1e-4, 0.0)).normalize();
    // O shader usa `strength` cru na velocidade e só clampa no ganho.
    let strength = wind_strength;
    let speed = 0.68 + 0.45 * strength;

    // DOMAIN WARP do `gerstner_swell` (0.085/0.095 e 1.8) — diferente do warp
    // do `wave_height`.
    let warp = Vec2::new(
        wave_value_noise(p * 0.085 + Vec2::new(t * 0.025, 0.0)),
        wave_value_noise(p * 0.095 - Vec2::new(0.0, t * 0.022)),
    ) * 1.8
        - Vec2::splat(0.9);
    let q = p + warp;

    let d0 = wdir;
    let d1 = Vec2::new(
        wdir.x * 0.829 - wdir.y * 0.559,
        wdir.x * 0.559 + wdir.y * 0.829,
    );
    let d2 = Vec2::new(
        wdir.x * 0.629 + wdir.y * 0.777,
        -wdir.x * 0.777 + wdir.y * 0.629,
    );
    let d3 = Vec2::new(
        wdir.x * 0.292 - wdir.y * 0.956,
        wdir.x * 0.956 + wdir.y * 0.292,
    );

    // `VERTEX_WAVE_GAIN` (0.18) × o ganho de vento e a amplitude do mundo.
    let gain = 0.18 * (0.45 + 0.40 * strength.clamp(0.2, 2.2)) * wave_amp;
    let (k0, k1, k2, k3) = (0.58_f32, 0.96, 1.54, 2.48);
    let g = 9.81_f32;
    let mut h = 0.0;
    h += (q.dot(d0) * k0 - t * (g * k0).sqrt() * 0.42 * speed).sin() * (0.28 * gain);
    h += (q.dot(d1) * k1 + t * (g * k1).sqrt() * 0.36 * speed).sin() * (0.20 * gain);
    h += (q.dot(d2) * k2 - t * (g * k2).sqrt() * 0.32 * speed).sin() * (0.13 * gain);
    h += (q.dot(d3) * k3 + t * (g * k3).sqrt() * 0.29 * speed).sin() * (0.08 * gain);

    // O vertex clampa o swell a ±0,12 e aplica 0,78; a máscara `deep_swell`
    // vem da profundidade assada (aqui, a profundidade real da coluna).
    h.clamp(-0.12, 0.12) * 0.78 * mirror_smoothstep(0.38, 1.35, depth)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O espelho CPU das ondas (buoyancy) é determinístico, limitado, depende
    /// da posição/tempo/vento e anula-se sem amplitude — propriedades que
    /// seguram o `wave_height_at` contra deriva silenciosa do shader.
    #[test]
    fn test_wave_height_at_properties() {
        let wind = [0.7, 0.25];
        let strength = 1.5;
        let amp = 1.0;

        // Determinístico e limitado (|trens| ≤ 0.69 + fbm ≤ ~1.2).
        for t in [0.0f32, 1.7, 42.0] {
            for (x, z) in [(0.0f32, 0.0f32), (13.3, -7.1), (-50.0, 50.0)] {
                let h = wave_height_at(Vec2::new(x, z), t, wind, strength, amp);
                assert!(h.is_finite() && h.abs() < 1.3, "bounded: {h} at {t}");
                assert_eq!(
                    h,
                    wave_height_at(Vec2::new(x, z), t, wind, strength, amp),
                    "deterministic"
                );
            }
        }
        // Depende da posição, do tempo e do vento.
        let base = wave_height_at(Vec2::new(3.0, 4.0), 0.0, wind, strength, amp);
        assert!(
            (base - wave_height_at(Vec2::new(3.5, 4.0), 0.0, wind, strength, amp)).abs() > 1e-4,
            "varies with position"
        );
        assert!(
            (base - wave_height_at(Vec2::new(3.0, 4.0), 0.5, wind, strength, amp)).abs() > 1e-4,
            "varies with time"
        );
        assert!(
            (base - wave_height_at(Vec2::new(3.0, 4.0), 0.0, [0.7, -0.9], strength, amp)).abs()
                > 1e-4,
            "varies with wind"
        );
        // Amplitude zero → lâmina parada; escala linear da amplitude.
        assert_eq!(
            wave_height_at(Vec2::new(3.0, 4.0), 1.0, wind, strength, 0.0),
            0.0
        );
        assert!(
            (wave_height_at(Vec2::new(3.0, 4.0), 1.0, wind, strength, 2.0)
                - 2.0 * wave_height_at(Vec2::new(3.0, 4.0), 1.0, wind, strength, 1.0))
            .abs()
                < 1e-5,
            "amplitude scales linearly"
        );
        // Vento zero é autoral válido: não pode dar NaN (guard 1e-4 do shader).
        assert!(
            wave_height_at(Vec2::new(3.0, 4.0), 1.0, [0.0, 0.0], strength, amp).is_finite(),
            "zero wind stays finite"
        );
    }

    /// O peso do termo FBM tem de ser o do shader (0.32, `water.wesl`,
    /// `wave_height`). O espelho CPU esteve em 0.62 — quase o dobro da
    /// amplitude DENSENHADA — e a lâmina que um flutuador lia não era a que a
    /// GPU desenha. A soma dos quatro senos é reproduzida aqui à mão para
    /// isolar o resíduo.
    #[test]
    fn test_wave_height_fbm_matches_the_shader_weight() {
        let (wind, strength, amp) = ([0.7, 0.25], 1.5f32, 1.0f32);
        let (x, z, t) = (13.3f32, -7.1f32, 2.7f32);
        let p = Vec2::new(x, z);
        let wdir = (Vec2::new(wind[0], wind[1]) + Vec2::new(1e-4, 0.0)).normalize();
        let speed = 0.7 + 0.5 * strength.clamp(0.05, 3.0);
        let warp = Vec2::new(
            wave_value_noise(p * 0.11 + Vec2::new(t * 0.03, 0.0)),
            wave_value_noise(p * 0.13 - Vec2::new(0.0, t * 0.025)),
        ) * 2.2
            - Vec2::splat(1.1);
        let q = p + warp;
        let d0 = wdir;
        let d1 = Vec2::new(
            wdir.x * 0.829 - wdir.y * 0.559,
            wdir.x * 0.559 + wdir.y * 0.829,
        );
        let d2 = Vec2::new(
            wdir.x * 0.629 + wdir.y * 0.777,
            -wdir.x * 0.777 + wdir.y * 0.629,
        );
        let d3 = Vec2::new(
            wdir.x * 0.292 - wdir.y * 0.956,
            wdir.x * 0.956 + wdir.y * 0.292,
        );
        let (k0, k1, k2, k3) = (0.897f32, 1.366, 2.094, 3.396);
        let mut sines = 0.0;
        sines += (q.dot(d0) * k0 - t * k0.sqrt() * 1.35 * speed).sin() * 0.22;
        sines += (q.dot(d1) * k1 + t * k1.sqrt() * 1.10 * speed).sin() * 0.18;
        sines += (q.dot(d2) * k2 - t * k2.sqrt() * 0.95 * speed).sin() * 0.12;
        sines += (q.dot(d3) * k3 + t * k3.sqrt() * 0.80 * speed).sin() * 0.07;
        let drift = wdir * t * 0.35 * speed;
        let fbm_term =
            wave_fbm(q * 0.9 + drift, drift * 0.4) * 0.32 * (0.6 + 0.4 * strength.min(2.0));
        let h = wave_height_at(p, t, wind, strength, amp);
        assert!(
            (h - (sines + fbm_term)).abs() < 1e-5,
            "CPU mirror = shader: {h} vs {} (o peso do FBM divergiu)",
            sines + fbm_term
        );
    }

    /// O espelho do DESLOCAMENTO visível (`gerstner_swell().y`) é a cota que
    /// um flutuador sente: limitado a ±0,12·0,78, linear em `wave_amp` antes
    /// do clamp, morto na praia (`depth <= 0.38`) e diferente do espelho das
    /// normais (outra família de senos) — a confusão entre os dois punha o
    /// barco a navegar numa onda que a malha não desenha.
    #[test]
    fn test_surface_height_mirrors_vertex_swell_not_normal_detail() {
        let (wind, strength, amp) = ([0.7, 0.25], 1.5f32, 1.0f32);
        let p = Vec2::new(13.3, -7.1);
        let t = 2.7;

        let deep = surface_height_at(p, t, wind, strength, amp, 3.0);
        assert!(deep.abs() <= 0.12 * 0.78 + 1e-5, "swell limitado: {deep}");
        assert_eq!(
            surface_height_at(p, t, wind, strength, 0.0, 3.0),
            0.0,
            "sem amplitude o espelho é plano"
        );
        assert_eq!(
            surface_height_at(p, t, wind, strength, amp, 0.30),
            0.0,
            "seco/raso (abaixo de 0.38 m) não tem onda"
        );
        assert_eq!(
            surface_height_at(p, t, wind, strength, amp, 0.38),
            0.0,
            "a rampa começa exatamente em 0.38 m"
        );
        let shallow = surface_height_at(p, t, wind, strength, amp, 0.8);
        assert!(
            shallow.abs() < deep.abs(),
            "a rampa de praia atenua: {shallow} vs {deep}"
        );

        // Linear em amplitude enquanto o clamp não morde.
        let a = surface_height_at(p, t, wind, strength, 0.02, 3.0);
        let b = surface_height_at(p, t, wind, strength, 0.04, 3.0);
        assert!((b - a * 2.0).abs() < 1e-6, "linear em amp: {a} / {b}");

        // Determinístico e distinto do espelho das normais.
        assert_eq!(deep, surface_height_at(p, t, wind, strength, amp, 3.0));
        let normal_mirror = wave_height_at(p, t, wind, strength, amp);
        assert!(
            (deep - normal_mirror).abs() > 1e-4,
            "o deslocamento visível não é o detalhe das normais: {deep} vs {normal_mirror}"
        );
    }
}

//! Ponte do material de água da crate `bevy_ext_water` para o runtime de
//! terreno.
//!
//! O material (dois passes Multiply/Add), os uniforms, o template
//! `water.wesl` e o sync de ambiente vivem na crate. Aqui fica apenas o
//! re-export dos tipos históricos — o TypePath do `WaterMaterial` é o nome
//! do asset que o jogo registra, por isso o caminho público mantém-se.
//!
//! O walk do IR declarativo (`<DayCycle>`/`<Weather>`/`<Lake>`/`<Sky>` →
//! [`WaterSurfaceConfig`]) vive no HOST (`viber::terrain_host`) — o IR de
//! parsing não é dependência desta crate.

pub use bevy_ext_water::material::{
    WATER_WGSL, WaterConfigResource, WaterExtension, WaterMaterial, WaterSurfaceConfig,
    WaterUniforms,
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Defaults sem `<DayCycle>`/`<Weather>` no mundo: o glint não é
    /// conduzido pelo relógio e o vento é o default.
    #[test]
    fn test_water_config_defaults() {
        let bare = WaterSurfaceConfig::default();
        assert!(!bare.drive);
        assert_eq!(bare.wind_strength, 1.5);
        assert_eq!(bare.wave_amp, 1.0);
    }

    /// O shader gerado substitui o bloco CONFIG e mantém o resto intacto.
    #[test]
    fn test_render_world_shader_specializes_config() {
        let config = WaterSurfaceConfig {
            drive: true,
            clock_start: 1140.0,
            wind_strength: 2.5,
            ..WaterSurfaceConfig::default()
        };
        let shader = config.render_world_shader();
        assert!(shader.contains("const CFG_DRIVE: f32 = 1;"));
        assert!(shader.contains("const CFG_CLOCK_START: f32 = 1140;"));
        assert!(shader.contains("const CFG_WIND_STRENGTH: f32 = 2.5;"));
        // O corpo do shader sobrevive à substituição.
        assert!(shader.contains("fn fragment"));
        assert!(shader.contains("wave_normal"));
        assert_eq!(shader.matches("// === END WORLD CONFIG ===").count(), 1);
    }

    /// O template compilado não pode levar overlays de debug. Um split de
    /// diagnóstico esquecido no WGSL passa despercebido em `cargo test` e em
    /// `cargo clippy` — só aparece na janela do jogo, e como o shader gerado
    /// só é reescrito no ARRANQUE, uma instância já aberta continua a
    /// mostrá-lo mesmo depois do fix. Esta guarda é o que trava isso.
    #[test]
    fn test_shader_template_has_no_debug_overlay() {
        for marker in ["DEBUG SPLIT", "DEBUG OVERLAY", "viewport.z / 3.0"] {
            assert!(
                !WATER_WGSL.contains(marker),
                "overlay de debug (`{marker}`) esquecido em water.wesl"
            );
        }
        // O caminho normal termina no PBR, não num write directo de cor.
        assert!(WATER_WGSL.contains("out.color = main_pass_post_lighting_processing"));
        // O vertex com deslocamento de onda (estilo bevy_water) TEM de
        // existir — sem ele a extensão devolvia o vertex base e as cristas
        // perdiam o parallax em silêncio.
        assert!(
            WATER_WGSL.contains("fn vertex(vertex_no_morph: Vertex) -> VertexOutput")
                && WATER_WGSL.contains("VERTEX_WAVE_GAIN"),
            "vertex shader da água com deslocamento de onda ausente"
        );
    }
}

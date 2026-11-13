//! Defaults DSL das features de terreno.
//!
//! O parser do Viber aplica o default do spec quando um attr está ausente;
//! esta tabela expõe esses defaults para a comparação semântica
//! (ausente ⇔ default) usada pelo `bsn_equiv` e pelo gate da migração
//! (`bevy_ext_world::bsn::terrain`). Os valores vêm sempre dos defaults dos
//! componentes — uma fonte de verdade.

use crate::voxel::{VoxelArch, VoxelCave, VoxelChamber, VoxelCliff, VoxelShaft};

/// O mesmo formato canónico do loader (`format_f32` do `bevy_ext_world`):
/// seis casas e zeros à direita cortados (`8.0` → `8`).
fn f32_text(value: f32) -> String {
    if !value.is_finite() || value == 0.0 {
        return "0".to_string();
    }
    let mut text = format!("{value:.6}");
    if text.contains('.') {
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    if text == "-0" {
        text = "0".to_string();
    }
    text
}

fn cave_default() -> VoxelCave {
    VoxelCave::default()
}

/// Valor DSL de um attr de feature ausente, ou `None` quando o attr é
/// obrigatório (`path`, `at`) ou herda do pai (`depth` de sala/chaminé).
pub fn feature_attr_default(tag: &str, key: &str) -> Option<String> {
    match (tag, key) {
        ("cave", "name") => Some(String::new()),
        ("cave", "radius") => {
            let radius = cave_default().radius;
            Some(
                radius
                    .iter()
                    .map(|value| f32_text(*value))
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        }
        ("cave", "depth") => Some(f32_text(cave_default().depth)),
        ("cave", "open-ends") => Some(cave_default().open_ends.to_string()),
        ("cave", "mouth-flare") => Some(f32_text(cave_default().mouth_flare)),
        ("cave", "mouth-fraction") => Some(f32_text(cave_default().mouth_fraction)),
        ("cave", "mound-height") => Some(f32_text(cave_default().mound_height)),
        ("cave", "mound-radius") => Some(f32_text(cave_default().mound_radius)),
        ("cave", "mouth-lift") => Some(cave_default().mouth_lift.to_string()),
        ("chamber", "name") => Some(String::new()),
        ("chamber", "radius") => Some(f32_text(VoxelChamber::default().radius)),
        ("chamber", "height") => Some(f32_text(VoxelChamber::default().height)),
        ("shaft", "name") => Some(String::new()),
        ("shaft", "radius") => Some(f32_text(VoxelShaft::default().radius)),
        // `span` do arco é `Option`: ausente ⇔ ausente (sem default), e o
        // `width` do DSL é o mesmo attr (normalizado no comparador).
        ("arch", "name") => Some(String::new()),
        ("arch", "at") => Some(vec2_text(VoxelArch::default().at)),
        ("arch", "height") => Some(f32_text(VoxelArch::default().height)),
        ("arch", "thickness") => Some(f32_text(VoxelArch::default().thickness)),
        ("arch", "depth") => Some(f32_text(VoxelArch::default().depth)),
        ("arch", "yaw") => Some(f32_text(VoxelArch::default().yaw)),
        ("arch", "spans") => Some(VoxelArch::default().spans.to_string()),
        ("arch", "profile") => Some("portal".to_string()),
        ("cliff", "name") => Some(String::new()),
        ("cliff", "width") => Some(f32_text(VoxelCliff::default().width)),
        ("cliff", "profile") => Some(VoxelCliff::default().profile),
        ("cliff", "side") => Some(VoxelCliff::default().side),
        ("cliff", "noise") => Some(f32_text(VoxelCliff::default().noise)),
        ("cliff", "gullies") => Some(f32_text(VoxelCliff::default().gullies)),
        ("cliff", "notches") => Some(f32_text(VoxelCliff::default().notches)),
        ("cliff", "talus") => Some(VoxelCliff::default().talus.to_string()),
        ("cliff", "talus-angle") => Some(f32_text(VoxelCliff::default().talus_angle)),
        ("cliff", "seed") => Some(VoxelCliff::default().seed.to_string()),
        _ => None,
    }
}

fn vec2_text([x, z]: [f32; 2]) -> String {
    format!("{} {}", f32_text(x), f32_text(z))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_mirror_the_spec() {
        assert_eq!(feature_attr_default("cave", "radius").as_deref(), Some("3"));
        assert_eq!(feature_attr_default("cave", "depth").as_deref(), Some("8"));
        assert_eq!(
            feature_attr_default("cave", "open-ends").as_deref(),
            Some("true")
        );
        assert_eq!(
            feature_attr_default("cave", "mouth-fraction").as_deref(),
            Some("0.18")
        );
        assert_eq!(
            feature_attr_default("cave", "mound-height").as_deref(),
            Some("0")
        );
        assert_eq!(
            feature_attr_default("cave", "mouth-lift").as_deref(),
            Some("false")
        );
        assert_eq!(
            feature_attr_default("chamber", "radius").as_deref(),
            Some("8")
        );
        assert_eq!(
            feature_attr_default("shaft", "radius").as_deref(),
            Some("1.5")
        );
        // Obrigatórios/herdados: sem default comparável.
        assert_eq!(feature_attr_default("cave", "path"), None);
        assert_eq!(feature_attr_default("chamber", "at"), None);
        assert_eq!(feature_attr_default("chamber", "depth"), None);
        // Arco e cliff espelham os specs do host.
        assert_eq!(feature_attr_default("arch", "height").as_deref(), Some("6"));
        assert_eq!(
            feature_attr_default("arch", "thickness").as_deref(),
            Some("2.5")
        );
        assert_eq!(feature_attr_default("arch", "at").as_deref(), Some("0 0"));
        assert_eq!(
            feature_attr_default("arch", "profile").as_deref(),
            Some("portal")
        );
        assert_eq!(feature_attr_default("arch", "span"), None);
        assert_eq!(feature_attr_default("cliff", "width").as_deref(), Some("6"));
        assert_eq!(
            feature_attr_default("cliff", "noise").as_deref(),
            Some("0.15")
        );
        assert_eq!(
            feature_attr_default("cliff", "talus").as_deref(),
            Some("false")
        );
        assert_eq!(
            feature_attr_default("cliff", "talus-angle").as_deref(),
            Some("36")
        );
        assert_eq!(feature_attr_default("cliff", "height"), None);
    }
}

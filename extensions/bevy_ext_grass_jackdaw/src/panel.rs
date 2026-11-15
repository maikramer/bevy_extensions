//! Painel "Relva": ligar/desligar, densidade e paletas por bioma.
//!
//! Os valores são componentes `Reflect` em `world/settings/grass.bsn`; cada
//! escrita passa pelo operador `component.set` (undo + gravação no `.bsn`).
//! O jogo a correr relê o documento (é a mesma via das definições de
//! ambiente/flora).

use bevy::prelude::*;
use jackdaw_extension::op::warn_caller;
use jackdaw_extension::prelude::*;

use bevy_ext_grass::{GrassSettingsSpec, GrassStyleSpec};

use crate::ui;

pub const WINDOW_ID: &str = "bevy_ext_grass.panel";

const TYPE_SETTINGS: &str = "bevy_ext_grass::GrassSettingsSpec";
const TYPE_STYLE: &str = "bevy_ext_grass::GrassStyleSpec";

#[derive(Component)]
pub(crate) struct GrassStatus;

pub(crate) fn build(world: &mut World, parent: Entity) {
    ui::column(world, parent, |root| {
        ui::heading(root, "Relva (bevy_ext_grass)");
        ui::status(root, GrassStatus);
        ui::row(root, &[("Ligar/desligar", "bevy_ext_grass.toggle")]);
        ui::row(
            root,
            &[
                ("Densidade −", "bevy_ext_grass.density_down"),
                ("Densidade +", "bevy_ext_grass.density_up"),
            ],
        );
        ui::row(
            root,
            &[("Nova paleta de bioma", "bevy_ext_grass.add_style")],
        );
        ui::label(
            root,
            "Cada paleta (`GrassStyleSpec`) tem um id de bioma, cor/altura/\
             densidade e plantas extra. O id 0 é o Vale; 1..4 são os quadrantes \
             quando o mundo tem `<BiomeRegion>`. Grava (Ctrl+S) e o jogo a \
             correr segue.",
        );
    });
}

pub(crate) fn refresh(
    settings: Query<&GrassSettingsSpec>,
    styles: Query<&GrassStyleSpec>,
    mut status: Query<&mut Text, With<GrassStatus>>,
) {
    let Ok(mut status) = status.single_mut() else {
        return;
    };
    let count = styles.iter().count();
    let text = match settings.iter().next() {
        None => "Sem GrassSettingsSpec nesta cena (abre world/settings/grass.bsn).".to_string(),
        Some(settings) => format!(
            "{} · densidade ×{:.2} · qualidade {:?} · {} paleta(s)",
            if settings.enabled {
                "ligada"
            } else {
                "desligada"
            },
            settings.density_scale,
            settings.quality,
            count
        ),
    };
    if status.0 != text {
        status.0 = text;
    }
}

#[operator(
    id = "bevy_ext_grass.toggle",
    label = "Relva: ligar/desligar",
    allows_undo = false
)]
pub(crate) fn toggle(_params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    let Some(entity) = ui::settings_entity::<GrassSettingsSpec>(world) else {
        return ui::missing::<GrassSettingsSpec>(world);
    };
    let enabled = world
        .get::<GrassSettingsSpec>(entity)
        .map(|settings| settings.enabled)
        .unwrap_or(true);
    ui::set_field(
        world,
        entity,
        TYPE_SETTINGS,
        "enabled",
        if enabled { "false" } else { "true" },
    )
}

fn scale_density(world: &mut World, factor: f32) -> OperatorResult {
    let Some(entity) = ui::settings_entity::<GrassSettingsSpec>(world) else {
        return ui::missing::<GrassSettingsSpec>(world);
    };
    let current = world
        .get::<GrassSettingsSpec>(entity)
        .map(|settings| settings.density_scale)
        .unwrap_or(1.0);
    let next = (current * factor).clamp(0.05, 4.0);
    ui::set_field(
        world,
        entity,
        TYPE_SETTINGS,
        "density_scale",
        format!("{next}"),
    )
}

#[operator(
    id = "bevy_ext_grass.density_down",
    label = "Relva: menos densidade",
    allows_undo = false
)]
pub(crate) fn density_down(_params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    scale_density(world, 0.8)
}

#[operator(
    id = "bevy_ext_grass.density_up",
    label = "Relva: mais densidade",
    allows_undo = false
)]
pub(crate) fn density_up(_params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    scale_density(world, 1.25)
}

/// Nova paleta como filha da entidade `GrassSettingsSpec` (grupo +
/// `component.add`, com undo e gravada no `.bsn`).
#[operator(
    id = "bevy_ext_grass.add_style",
    label = "Relva: nova paleta",
    allows_undo = false
)]
pub(crate) fn add_style(_params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    let Some(parent) = ui::settings_entity::<GrassSettingsSpec>(world) else {
        return ui::missing::<GrassSettingsSpec>(world);
    };
    let next_biome = {
        let mut query = world.query::<&GrassStyleSpec>();
        query
            .iter(world)
            .map(|style| style.biome)
            .max()
            .unwrap_or(0)
            + 1
    };
    let history = CallOperatorSettings {
        creates_history_entry: true,
        ..default()
    };
    let name = format!("Bioma {next_biome}");
    if let Err(err) = world
        .operator("entity.add.group")
        .param("name", name.clone())
        .param("parent", parent)
        .settings(history)
        .call()
    {
        warn_caller(world, format!("relva: {err}"));
        return OperatorResult::Cancelled;
    }
    let children: Vec<Entity> = world
        .get::<Children>(parent)
        .map(|children| children.iter().collect())
        .unwrap_or_default();
    let Some(entity) = children.into_iter().rev().find(|child| {
        world
            .get::<Name>(*child)
            .is_some_and(|child_name| child_name.as_str() == name)
    }) else {
        warn_caller(world, "relva: o grupo criado não apareceu");
        return OperatorResult::Cancelled;
    };
    if let Err(err) = world
        .operator("component.add")
        .param("entity", entity)
        .param("type_path", TYPE_STYLE.to_string())
        .settings(history)
        .call()
    {
        warn_caller(world, format!("relva: {err}"));
        return OperatorResult::Cancelled;
    }
    ui::set_field(world, entity, TYPE_STYLE, "biome", format!("{next_biome}"))
}

pub(crate) fn register(ctx: &mut ExtensionRegistrar<'_>) {
    ctx.register_operator::<ToggleOp>()
        .register_operator::<DensityDownOp>()
        .register_operator::<DensityUpOp>()
        .register_operator::<AddStyleOp>();
}

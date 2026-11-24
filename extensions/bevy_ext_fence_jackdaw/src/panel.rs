//! Painel "Cercas": estado da cena e um botão por predefinição que cria uma
//! linha nova com esse estilo.
//!
//! As predefinições são componentes `Reflect` em `world/settings/fences.bsn`
//! (`FenceStyle`); a linha nova é um `entity.add.empty` + `component.add` +
//! `component.set`, tudo por operadores do editor (undo + `.bsn` gravado).

use bevy::prelude::*;
use jackdaw_extension::op::warn_caller;
use jackdaw_extension::prelude::*;

use crate::preview::SPEC;
use crate::ui;

pub const WINDOW_ID: &str = "bevy_ext_fence.panel";

#[derive(Component)]
pub(crate) struct FenceStatus;

pub(crate) fn build(world: &mut World, parent: Entity) {
    ui::column(world, parent, |root| {
        ui::heading(root, "Cercas (bevy_ext_fence)");
        ui::status(root, FenceStatus);
        ui::row(root, &[("Travessas de pastagem", "bevy_ext_fence.new.pasture_rail")]);
        ui::row(root, &[("Curral de três travessas", "bevy_ext_fence.new.paddock_3rail")]);
        ui::row(root, &[("Tapume de tábuas", "bevy_ext_fence.new.plank_board")]);
        ui::row(root, &[("Estacas", "bevy_ext_fence.new.picket")]);
        ui::row(root, &[("Paliçada", "bevy_ext_fence.new.palisade")]);
        ui::row(root, &[("Varas entretecidas", "bevy_ext_fence.new.wattle")]);
        ui::row(root, &[("Treliça", "bevy_ext_fence.new.lattice")]);
        ui::row(root, &[("Cruzetas", "bevy_ext_fence.new.crossbuck")]);
        ui::row(root, &[("Cordas", "bevy_ext_fence.new.rope")]);
        ui::row(root, &[("Portão", "bevy_ext_fence.new.gate")]);
        ui::label(
            root,
            "Cada botão cria uma linha com a predefinição, ao nível de topo. A \
             linha é o `points` do `FenceSpec` (\"x z; x z\", local à \
             entidade); os restantes campos sobrepõem a predefinição — altura, \
             travessas, tábuas, comprimento da tábua. As predefinições \
             editam-se em world.bsn (pasta Definições → Cercas).",
        );
    });
}

pub(crate) fn refresh(previews: Res<crate::preview::FencePreviews>, mut status: Query<&mut Text, With<FenceStatus>>) {
    let Ok(mut status) = status.single_mut() else {
        return;
    };
    if !previews.status.is_empty() && status.0 != previews.status {
        status.0 = previews.status.clone();
    }
}

/// Uma linha nova com a predefinição `style`, por operadores do editor.
fn new_fence(world: &mut World, style: &str) -> OperatorResult {
    let history = CallOperatorSettings {
        creates_history_entry: true,
        ..default()
    };
    let name = format!("cerca-{style}");
    if let Err(err) = world
        .operator("entity.add.empty")
        .param("name", name.clone())
        .settings(history)
        .call()
    {
        warn_caller(world, format!("cercas: {err}"));
        return OperatorResult::Cancelled;
    }
    // O nó criado é o último com este nome.
    let mut query = world.query_filtered::<Entity, With<Name>>();
    let found = query
        .iter(world)
        .filter(|entity| world.get::<Name>(*entity).is_some_and(|node| node.as_str() == name))
        .max();
    let Some(entity) = found else {
        warn_caller(world, "cercas: o nó criado não apareceu");
        return OperatorResult::Cancelled;
    };
    if let Err(err) = world
        .operator("component.add")
        .param("entity", entity)
        .param("type_path", SPEC.to_string())
        .settings(history)
        .call()
    {
        warn_caller(world, format!("cercas: {err}"));
        return OperatorResult::Cancelled;
    }
    // O portão mede o vão dele (2,6 m); o resto é uma linha de 6 m.
    let points = if style == "gate" { "0.0 0.0; 2.6 0.0" } else { "0.0 0.0; 6.0 0.0" };
    let points = ui::set_field(world, entity, SPEC, "points", format!("\"{points}\""));
    if matches!(points, OperatorResult::Cancelled) {
        return points;
    }
    ui::set_field(world, entity, SPEC, "style", format!("Some(\"{style}\")"))
}

/// Um operador por predefinição (os botões só levam o id do operador). Os
/// nomes das funções não levam dígitos: o operador vira `<PascalCase(fn)>Op`.
// `$id:tt`: um fragmento `literal` não chega como `id = "..."` ao atributo.
macro_rules! new_fence {
    ($($function:ident, $id:tt, $caption:tt, $style:tt;)*) => {
        $(
            #[operator(id = $id, label = $caption, allows_undo = false)]
            pub(crate) fn $function(_params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
                new_fence(world, $style)
            }
        )*
    };
}

new_fence! {
    new_pasture, "bevy_ext_fence.new.pasture_rail", "Cercas: nova linha de travessas de pastagem", "pasture_rail";
    new_paddock, "bevy_ext_fence.new.paddock_3rail", "Cercas: nova linha de curral", "paddock_3rail";
    new_plank, "bevy_ext_fence.new.plank_board", "Cercas: nova linha de tapume", "plank_board";
    new_picket, "bevy_ext_fence.new.picket", "Cercas: nova linha de estacas", "picket";
    new_palisade, "bevy_ext_fence.new.palisade", "Cercas: nova linha de paliçada", "palisade";
    new_wattle, "bevy_ext_fence.new.wattle", "Cercas: nova linha de varas entretecidas", "wattle";
    new_lattice, "bevy_ext_fence.new.lattice", "Cercas: nova linha de treliça", "lattice";
    new_crossbuck, "bevy_ext_fence.new.crossbuck", "Cercas: nova linha de cruzetas", "crossbuck";
    new_rope, "bevy_ext_fence.new.rope", "Cercas: nova linha de cordas", "rope";
    new_gate, "bevy_ext_fence.new.gate", "Cercas: novo portão", "gate";
}

pub(crate) fn register(ctx: &mut ExtensionRegistrar<'_>) {
    ctx.register_operator::<NewPastureOp>()
        .register_operator::<NewPaddockOp>()
        .register_operator::<NewPlankOp>()
        .register_operator::<NewPicketOp>()
        .register_operator::<NewPalisadeOp>()
        .register_operator::<NewWattleOp>()
        .register_operator::<NewLatticeOp>()
        .register_operator::<NewCrossbuckOp>()
        .register_operator::<NewRopeOp>()
        .register_operator::<NewGateOp>();
}

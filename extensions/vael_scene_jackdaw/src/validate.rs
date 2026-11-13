//! Validação ao gravar.
//!
//! O editor não deixa vetar uma gravação (o `SceneSaved` chega depois do
//! ficheiro escrito). Por isso há duas peças:
//! - um observer de `SceneSaved` que revê a cena (o índice desta extensão e o
//!   `vael.analyze.run` do `viber-tools`, se estiver instalado) e avisa;
//! - o operador "Vael: validar e gravar", que recusa gravar com problemas.

use bevy::prelude::*;
use jackdaw_extension::document::SceneSaved;
use jackdaw_extension::op::warn_caller;
use jackdaw_extension::prelude::*;

use crate::index::SceneIndex;
use crate::panel::problems;

/// Resultado da última validação (mostrado no painel).
#[derive(Resource, Default)]
pub struct LastValidation {
    pub summary: Option<String>,
}

pub fn on_saved(saved: On<SceneSaved>, mut commands: Commands) {
    let path = saved.path.clone();
    commands.queue(move |world: &mut World| {
        let found = problems(world.resource::<SceneIndex>());
        let mut summary = format!(
            "Gravado {}: {} problema(s) da cena",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
            found.len()
        );
        // O `viber analyze` do viber-tools (painel "Vael: Problems").
        match world.operator("vael.analyze.run").call() {
            Ok(_) => summary.push_str(" · analyze corrido (ver Vael: Problems)"),
            Err(_) => summary.push_str(" · analyze indisponível"),
        }
        if let Some((_, first)) = found.first() {
            warn_caller(world, format!("vael: {summary}. Primeiro: {first}"));
        } else {
            info!("vael: {summary}");
        }
        world.resource_mut::<LastValidation>().summary = Some(summary);
    });
}

#[operator(
    id = "vael.scene.validate_and_save",
    label = "Vael: validar e gravar",
    description = "Grava a cena só se a verificação da cena não encontrar problemas \
                   (force=true grava na mesma).",
    allows_undo = false,
    params(force(bool, doc = "Gravar mesmo com problemas."))
)]
pub fn validate_and_save(params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    let force = params.as_bool("force").unwrap_or(false);
    let found = problems(world.resource::<SceneIndex>());
    if !found.is_empty() && !force {
        warn_caller(
            world,
            format!(
                "vael: não gravado — {} problema(s); o primeiro: {}. \"Próximo problema\" no \
                 painel Vael: Cena, ou force=true.",
                found.len(),
                found[0].1
            ),
        );
        return OperatorResult::Cancelled;
    }
    match world.operator("scene.save").call() {
        Ok(OperatorResult::Finished) => OperatorResult::Finished,
        Ok(_) => {
            warn_caller(world, "vael: a gravação não terminou (ver o aviso do scene.save)");
            OperatorResult::Cancelled
        }
        Err(err) => {
            warn_caller(world, format!("vael: gravar falhou: {err}"));
            OperatorResult::Cancelled
        }
    }
}

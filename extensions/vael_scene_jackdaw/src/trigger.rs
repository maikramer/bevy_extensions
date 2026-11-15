//! Espelho do gatilho de cinemática do jogo.
//!
//! O tipo verdadeiro é `vael::cinematic_trigger::CinematicTrigger`, no crate
//! do jogo — que esta extensão não compila. Os tipos que a extensão compila
//! são registados pelo `jd` (`jackdaw_register_types`) e o loader da cena
//! aplica-os **vivos** na entidade; os tipos que só vêm no schema do jogo
//! ficam só no documento e não se podem ler por ECS. Para o inspector, o
//! ícone, o anel do raio e as verificações terem o gatilho, a extensão declara
//! esta cópia com o **mesmo caminho de tipo** e os mesmos campos.
//!
//! Os campos têm de coincidir com `Vael/src/cinematic_trigger.rs`: mudar um
//! lado sem o outro parte a leitura no editor (o `.bsn` guarda só o caminho e
//! os valores, por isso o jogo continua a ler o seu tipo).

use bevy::prelude::*;

/// `vael::cinematic_trigger::CinematicTrigger` (cópia de autoria).
#[derive(Component, Clone, Debug, PartialEq, Reflect)]
#[reflect(Component, Default, Debug, Clone)]
#[type_path = "vael::cinematic_trigger"]
pub struct CinematicTrigger {
    /// `id` da `<Cinematic>`.
    pub cinematic: String,
    /// Raio de entrada (m).
    pub radius: f32,
    /// Só a primeira vez (por save).
    pub once: bool,
}

impl Default for CinematicTrigger {
    fn default() -> Self {
        Self {
            cinematic: String::new(),
            radius: 14.0,
            once: true,
        }
    }
}

//! BGM driver — shim para o [`bevy_ext_audio::music`].
//!
//! O driver BGM (loops kira por `<MusicLayer>`, crossfade por zona, buses
//! tipados) foi extraído para o `bevy_ext_audio` — os TypePaths históricos
//! (`viber::music::…`) estão congelados lá. O caminho `crate::music::…`
//! mantém-se para os consumidores do monólito não mudarem; o
//! [`AudioMixerSettings`] é contrato do `bevy_ext_core` (o crate de áudio
//! importa-o a privado, por isso o re-export aqui repõe o caminho antigo).

pub use bevy_ext_audio::music::*;
pub use bevy_engine_core::AudioMixerSettings;

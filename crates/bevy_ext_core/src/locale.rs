//! Textos do jogo por idioma.
//!
//! Ficheiros `locale/<lang>.json` (mapa chave → string). Uma string `@chave`
//! resolve; sem entrada, fica a string original para o jogo em PT não partir.
//! `VIBER_LANG` escolhe o idioma (default `pt`).
//!
//! Definição CANÓNICA: o monólito (`viber::locale`) e o host de UI
//! declarativa (`bevy_ext_ui::ui::host`) re-exportam este módulo — o jogo e
//! a UI leem O MESMO recurso `Locale`.

use std::collections::HashMap;
use std::path::Path;

use bevy::prelude::*;

use crate::dialogue::localized;

#[derive(Resource, Clone, Debug, Default, Reflect)]
#[reflect(Resource)]
// Preserva o TypePath do BRP/`viber.ecs` depois da extração.
#[type_path = "viber::locale"]
pub struct Locale {
    pub lang: String,
    pub table: HashMap<String, String>,
    /// Pasta dos `locale/<lang>.json` (para a troca em runtime).
    pub dir: std::path::PathBuf,
}

impl Locale {
    pub fn resolve(&self, text: &str) -> String {
        localized(text, Some(&self.table))
    }

    pub fn load(dir: &Path, lang: &str) -> Self {
        let path = dir.join(format!("{lang}.json"));
        let table = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<HashMap<String, String>>(&text).ok())
            .unwrap_or_default();
        Self {
            lang: lang.to_string(),
            table,
            dir: dir.to_path_buf(),
        }
    }

    /// Troca o idioma sem reiniciar. Um ficheiro ausente deixa a tabela
    /// vazia (as strings originais ficam) — o jogo não parte.
    pub fn set_lang(&mut self, lang: &str) {
        if lang.is_empty() || lang == self.lang {
            return;
        }
        *self = Self::load(&self.dir, lang);
    }
}

/// Idioma pedido por `VIBER_LANG`; vazio/só espaços é OMISSO (`pt`). Um
/// `VIBER_LANG= viber run` (variável vazia num script) selecionava
/// `locale/.json`, que não existe, e deixava a tabela vazia — todas as chaves
/// `@…` apareciam cruas, sem aviso nenhum.
pub fn lang_from_env() -> String {
    pick_lang(crate::envs::var_lossy("LANG").as_deref())
}

fn pick_lang(raw: Option<&str>) -> String {
    raw.map(str::trim)
        .filter(|lang| !lang.is_empty())
        .unwrap_or("pt")
        .to_string()
}

pub struct LocalePlugin {
    pub dir: std::path::PathBuf,
}

impl Plugin for LocalePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Locale::load(&self.dir, &lang_from_env()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::reflect::TypePath;

    #[test]
    fn unknown_language_file_is_an_empty_table() {
        let locale = Locale::load(Path::new("/no/such/locale"), "pt");
        assert_eq!(locale.resolve("Olá"), "Olá");
        assert_eq!(locale.resolve("@hi"), "@hi");
    }

    /// `VIBER_LANG` vazio (ou só espaços) cai no default `pt`, nunca no
    /// ficheiro inexistente `locale/.json`.
    #[test]
    fn empty_env_language_falls_back_to_pt() {
        assert_eq!(pick_lang(None), "pt");
        assert_eq!(pick_lang(Some("")), "pt");
        assert_eq!(pick_lang(Some("   ")), "pt");
        assert_eq!(pick_lang(Some(" en ")), "en");
    }

    /// Contrato da extração: o TypePath histórico fica congelado — o BRP e
    /// os saves não podem ver um caminho novo depois do tipo mudar de crate.
    #[test]
    fn reflected_types_keep_the_historical_viber_paths() {
        assert_eq!(Locale::type_path(), "viber::locale::Locale");
    }
}

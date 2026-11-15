//! Texto localizado e o contrato de apresentação de falas — SEM intérprete
//! de grafos.
//!
//! Os grafos de diálogo (`dialogues/*.json`) são CONTEÚDO e correm no jogo
//! (Lua, `scripts/townsfolk.lua`): o script lê o JSON com `viber.load_json`,
//! guarda o cursor em `viber.game()` e escreve com `viber.say`. A engine só
//! resolve chaves `@nome` (locale) e desenha o balão.
//!
//! Definição CANÓNICA: o monólito (`viber::dialogue`) re-exporta este
//! módulo. A parte de BALÃO nativo (`show_balloon`) fica no monólito —
//! escreve no `HudBalloon` do HUD, um tipo de serviço que o core não pode
//! ter.

use std::collections::HashMap;

use bevy::prelude::*;

/// O jogo apresenta as falas de `viber.say` sozinho.
///
/// Com este recurso presente a engine continua a resolver o locale, a baixar
/// a música e a publicar o `ScriptSay` do host Luau, mas NÃO escreve o balão
/// nativo do HUD (nem avisa que ele falta). É o que deixa um jogo desenhar
/// balões no mundo, uma caixa de diálogo ou legendas sem ficar com a linha
/// duplicada no HUD.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct ExternalSayPresenter;

/// Resolve um texto `@chave` contra a tabela de locale; sem tabela devolve o original.
pub fn localized(text: &str, table: Option<&HashMap<String, String>>) -> String {
    let Some(key) = text.strip_prefix('@') else {
        return text.to_string();
    };
    table
        .and_then(|t| t.get(key).cloned())
        .unwrap_or_else(|| text.to_string())
}

/// Resolve `@chave` linha a linha (balões multi-linha e `viber.say`).
pub fn localized_lines(text: &str, table: Option<&HashMap<String, String>>) -> String {
    text.split('\n')
        .map(|line| localized(line, table))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localized_resolves_keys_and_keeps_plain_text() {
        let mut table = HashMap::new();
        table.insert("hi".to_string(), "Olá".to_string());
        assert_eq!(localized("@hi", Some(&table)), "Olá");
        assert_eq!(localized("Olá", Some(&table)), "Olá");
        assert_eq!(localized("@missing", Some(&table)), "@missing");
        assert_eq!(localized("@hi", None), "@hi");
    }

    #[test]
    fn localized_lines_resolves_each_line() {
        let mut table = HashMap::new();
        table.insert("a".to_string(), "um".to_string());
        table.insert("b".to_string(), "dois".to_string());
        assert_eq!(localized_lines("@a\nmeio\n@b", Some(&table)), "um\nmeio\ndois");
    }
}

//! A linha da cerca: `fence-points` em pontos.
//!
//! `"x z; x z; …"` (à cota autorada da entidade) ou `"x y z; …"` (cota própria
//! de cada ponto). Também se aceita a lista plana sem `;`, sempre aos pares
//! `x z`. Espaços a mais e um `;` final são indiferentes.

use bevy::prelude::*;

fn number(value: &str) -> Option<f32> {
    value.parse::<f32>().ok().filter(|number| number.is_finite())
}

fn parse_point(text: &str) -> Option<Vec3> {
    let values: Vec<f32> = text.split_whitespace().map(number).collect::<Option<_>>()?;
    match values[..] {
        [x, z] => Some(Vec3::new(x, 0.0, z)),
        [x, y, z] => Some(Vec3::new(x, y, z)),
        _ => None,
    }
}

/// Os pontos da linha, no espaço local da entidade. `None` se o texto não for
/// uma lista de pontos (um número a mais, um valor que não é número, …).
pub fn parse_points(text: &str) -> Option<Vec<Vec3>> {
    if text.contains(';') {
        let mut points = Vec::new();
        for part in text.split(';') {
            if part.trim().is_empty() {
                continue;
            }
            points.push(parse_point(part)?);
        }
        (!points.is_empty()).then_some(points)
    } else {
        let values: Vec<f32> = text.split_whitespace().map(number).collect::<Option<_>>()?;
        if values.len() < 2 || !values.len().is_multiple_of(2) {
            return None;
        }
        Some(
            values
                .as_chunks::<2>()
                .0
                .iter()
                .map(|[x, z]| Vec3::new(*x, 0.0, *z))
                .collect(),
        )
    }
}

/// O texto canónico de uma lista de pontos (`"x z; x z"`, à cota zero) — o que
/// o `tools/fence_migrate.py` escreve no `fence-points`.
pub fn format_points(points: &[Vec2]) -> String {
    points
        .iter()
        .map(|point| format!("{} {}", crate::specs::num(point.x), crate::specs::num(point.y)))
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_as_tres_formas() {
        let expected = vec![Vec3::new(-24.6, 0.0, 0.0), Vec3::new(5.9, 0.0, 0.5)];
        assert_eq!(parse_points("-24.6 0; 5.9 0.5").as_deref(), Some(expected.as_slice()));
        assert_eq!(parse_points("-24.6 0;5.9 0.5;").as_deref(), Some(expected.as_slice()));
        assert_eq!(parse_points("  -24.6 0   5.9 0.5 ").as_deref(), Some(expected.as_slice()));
        assert_eq!(
            parse_points("-24.6 8.2 0; 5.9 8.3 0.5").as_deref(),
            Some([Vec3::new(-24.6, 8.2, 0.0), Vec3::new(5.9, 8.3, 0.5)].as_slice())
        );
    }

    #[test]
    fn recusa_o_que_nao_e_uma_linha() {
        assert_eq!(parse_points(""), None);
        assert_eq!(parse_points("1 2 3"), None, "lista plana é aos pares");
        assert_eq!(parse_points("1 2; 3"), None);
        assert_eq!(parse_points("1 2; a b"), None);
        assert_eq!(parse_points("1 2; 3 4 5 6"), None);
        assert_eq!(parse_points("1 2; inf 4"), None);
        // Um ponto só é uma lista válida: quem diz que é curta é o layout.
        assert_eq!(parse_points("1 2").as_deref(), Some([Vec3::new(1.0, 0.0, 2.0)].as_slice()));
    }

    #[test]
    fn o_texto_canonico_volta_igual() {
        let text = format_points(&[Vec2::new(-24.6, 0.0), Vec2::new(5.9, 0.5)]);
        assert_eq!(text, "-24.6 0; 5.9 0.5");
        let back = parse_points(&text).unwrap();
        assert_eq!(back[1], Vec3::new(5.9, 0.0, 0.5));
    }
}

//! Helpers de teste que no monólito vinham de `crate::` (ex.: `vael_root`).

/// Checkout do jogo Vael, irmão deste repo.
///
/// `VAEL_ROOT` ganha. Sem a variável, procura `../Vael` a partir da raiz do
/// crate. Testes que leem o mundo do jogo saltam quando isto é `None`
/// (CI da engine sozinha).
#[cfg(test)]
pub(crate) fn vael_root() -> Option<std::path::PathBuf> {
    let candidate = match std::env::var_os("VAEL_ROOT") {
        Some(value) => std::path::PathBuf::from(value),
        None => std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../Vael"),
    };
    let candidate = candidate.canonicalize().unwrap_or(candidate);
    candidate.is_dir().then_some(candidate)
}

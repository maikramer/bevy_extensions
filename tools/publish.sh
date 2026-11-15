#!/usr/bin/env bash
# Publica a família `bevy_ext_*` na ordem de dependência.
#
#   tools/publish.sh --dry-run   # valida os independentes + package list
#   tools/publish.sh             # publica (precisa de token do crates.io)
#
# `bevy_ext_save` depende de `bevy_ext_world`; o script espera pelo índice
# antes do dependente. `bevy_ext_terrain` fica para 0.1.1 (contrato de cena
# ainda em estabilização); o `bevy_voxel_terrain` de que ele depende é
# publicado nesta ronda.
set -euo pipefail

cd "$(dirname "$0")/.."

mode="${1:-publish}"
if [[ "$mode" != "--dry-run" && "$mode" != "publish" ]]; then
  echo "uso: $0 [--dry-run]" >&2
  exit 2
fi

version=$(awk '
  /^\[workspace.package\]/ { in_pkg = 1; next }
  /^\[/ { in_pkg = 0 }
  in_pkg && /^version[[:space:]]*=/ {
    gsub(/"/, "", $3); print $3; exit
  }
' Cargo.toml)
if [[ -z "$version" ]]; then
  echo "não consegui ler a versão de [workspace.package] em Cargo.toml" >&2
  exit 1
fi

if [[ "$mode" == "publish" ]]; then
  if [[ -z "${CARGO_REGISTRY_TOKEN:-}" && ! -f "${CARGO_HOME:-$HOME/.cargo}/credentials.toml" ]]; then
    echo "sem token do crates.io: corre 'cargo login' ou define CARGO_REGISTRY_TOKEN" >&2
    exit 1
  fi
fi

# O dry-run corre numa árvore com WIP (o commit é o gate do publish real).
publish() {
  local crate="$1"
  if is_published "$crate"; then
    echo "== $crate $version: já está no índice =="
    return 0
  fi
  echo "== $crate $version =="
  if [[ "$mode" == "--dry-run" ]]; then
    cargo publish --dry-run --locked --allow-dirty -p "$crate"
  else
    cargo publish --locked -p "$crate"
  fi
}

# `cargo search` não tem API de versão; a linha é `crate = "x.y.z"`.
is_published() {
  cargo search "$1" --limit 1 2>/dev/null \
    | grep -q "^$1 = \"$version\""
}

wait_for() {
  local crate="$1"
  for _ in $(seq 1 30); do
    if is_published "$crate"; then
      return 0
    fi
    sleep 10
  done
  echo "aviso: $crate $version ainda não aparece no índice; a continuar" >&2
}

publish bevy_ext_core
publish bevy_ext_grass
publish bevy_ext_world
publish bevy_ext_gltf
publish bevy_voxel_terrain

if [[ "$mode" == "--dry-run" ]]; then
  echo "== bevy_ext_save (package --list; dry-run só após bevy_ext_world no índice) =="
  cargo package --list -p bevy_ext_save
  echo "== bevy_ext_terrain (held; package --list) =="
  cargo package --list -p bevy_ext_terrain
  exit 0
fi

wait_for bevy_ext_world
publish bevy_ext_save

echo "OK: família $version publicada (5 pacotes; bevy_ext_terrain fica para 0.1.1)."
echo "Sugestão: tag v$version e 'gh release create v$version --generate-notes'."

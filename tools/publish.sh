#!/usr/bin/env bash
# Publica a família `bevy_ext_*` na ordem de dependência.
#
#   tools/publish.sh --dry-run   # valida os independentes + package list
#   tools/publish.sh             # publica (precisa de token do crates.io)
#
# O `bevy_ext_terrain` depende de `bevy_voxel_terrain` e o `bevy_ext_save` de
# `bevy_ext_world`; o script espera pelo índice antes dos dependentes.
set -euo pipefail

cd "$(dirname "$0")/.."

mode="${1:-publish}"
if [[ "$mode" != "--dry-run" && "$mode" != "publish" ]]; then
  echo "uso: $0 [--dry-run]" >&2
  exit 2
fi

if [[ "$mode" == "publish" ]]; then
  if [[ -z "${CARGO_REGISTRY_TOKEN:-}" && ! -f "${CARGO_HOME:-$HOME/.cargo}/credentials.toml" ]]; then
    echo "sem token do crates.io: corre 'cargo login' ou define CARGO_REGISTRY_TOKEN" >&2
    exit 1
  fi
fi

publish() {
  if [[ "$mode" == "--dry-run" ]]; then
    cargo publish --dry-run --locked "$@"
  else
    cargo publish --locked "$@"
  fi
}

wait_for() {
  local crate="$1"
  for _ in $(seq 1 30); do
    if cargo search "$crate" --limit 1 2>/dev/null | grep -q "^$crate "; then
      return 0
    fi
    sleep 10
  done
  echo "aviso: $crate ainda não aparece no índice; a continuar" >&2
}

echo "== bevy_ext_core =="
publish -p bevy_ext_core

echo "== bevy_ext_world =="
publish -p bevy_ext_world

echo "== bevy_ext_gltf =="
publish -p bevy_ext_gltf

echo "== bevy_voxel_terrain =="
publish -p bevy_voxel_terrain

if [[ "$mode" == "--dry-run" ]]; then
  echo "== bevy_ext_terrain / bevy_ext_save (package --list; dry-run só após as deps) =="
  cargo package --list -p bevy_ext_terrain
  cargo package --list -p bevy_ext_save
  exit 0
fi

wait_for bevy_voxel_terrain
echo "== bevy_ext_terrain =="
publish -p bevy_ext_terrain

wait_for bevy_ext_world
echo "== bevy_ext_save =="
publish -p bevy_ext_save

echo "OK: família 0.1.0 publicada."
echo "Sugestão: tag v0.1.0 e 'gh release create v0.1.0 --generate-notes'."

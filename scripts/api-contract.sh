#!/usr/bin/env bash
set -euo pipefail

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
mode=${1:-check}
case "$mode" in generate|check) ;; *) echo 'usage: bash scripts/api-contract.sh generate|check' >&2; exit 2 ;; esac
scratch=$(mktemp -d -t gitadel-contract.XXXXXXXX)
trap 'rm -r -- "$scratch"' EXIT

if [ -n "${GITADEL_OPENAPI_FILE:-}" ]; then
  cp -- "$GITADEL_OPENAPI_FILE" "$scratch/openapi.json"
elif [ -n "${GITADEL_OPENAPI_EXPORTER:-}" ]; then
  "$GITADEL_OPENAPI_EXPORTER" > "$scratch/openapi.json"
else
  cargo run --quiet --locked --offline --jobs "${CARGO_BUILD_JOBS:-2}" --package gitadel --features api-contract-export --bin gitadel > "$scratch/openapi.json"
fi

bun frontend/scripts/validate-openapi.ts "$scratch/openapi.json"

if [ "$mode" = generate ]; then
  cp -- "$scratch/openapi.json" api/openapi.json
  bun run --cwd frontend api:sdk
else
  cmp -- api/openapi.json "$scratch/openapi.json" || {
    echo 'Rust API contract drifted. Run just api-generate and review the API/SDK changes.' >&2
    exit 1
  }
  GITADEL_SDK_OUTPUT="$scratch/sdk" GITADEL_CONTRACTS_OUTPUT="$scratch/contracts.gen.ts" \
    bun run --cwd frontend api:sdk
  diff -r -- frontend/src/lib/api/generated "$scratch/sdk"
  cmp -- frontend/src/lib/api/contracts.gen.ts "$scratch/contracts.gen.ts"
fi

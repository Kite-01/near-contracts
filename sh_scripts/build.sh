#!/usr/bin/env bash
# Builds kite and factory to target/near/<name>/<name>.wasm and prints their code hashes.
#
# Usage: ./sh_scripts/build.sh [--all]   (--all also builds the test contracts the sandbox needs)

source "$(dirname "$0")/_lib.sh"
need cargo

targets=("${CONTRACTS[@]}")
[[ "${1:-}" == --all ]] && targets+=("${TEST_CONTRACTS[@]}")

for name in "${targets[@]}"; do
  info "Building $name"
  cargo near build non-reproducible-wasm --manifest-path "$(manifest_of "$name")" --no-abi
done

printf '\n%-14s %8s  %s\n' contract size "code hash"

for name in "${CONTRACTS[@]}"; do
  wasm="$(wasm_of "$name")"
  printf '%-14s %7sK  %s\n' "$name" "$(($(wc -c <"$wasm") / 1024))" "$(code_hash "$wasm")"
done

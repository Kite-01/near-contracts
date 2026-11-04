#!/usr/bin/env bash
# Publishes the kite contract as a global contract by hash (NEP-591).
# Publishing burns storage: about 10 NEAR per 100 KB of wasm, paid by the publisher.
# Usage: NEAR_NETWORK=testnet ./sh_scripts/publish-deposit.sh <publisher-account> [wasm]
#
# The factory keeps using its current implementation until the admin runs
#   ./sh_scripts/admin.sh set-implementation <code hash>

source "$(dirname "$0")/_lib.sh"
need near

publisher="${1:?usage: publish-deposit.sh <publisher-account> [wasm]}"
wasm="${2:-$(wasm_of kite)}"
[[ -f "$wasm" ]] || "$SCRIPTS/build.sh"

hash="$(code_hash "$wasm")"
size="$(($(wc -c <"$wasm") / 1024))"
info "kite: ${size} KB, code hash $hash"

if [[ -n "${KITE_FACTORY:-}" ]] && [[ "$(view "$KITE_FACTORY" is_implementation "{\"implementation\":\"$hash\"}" || true)" == true ]]; then
  info "The factory already accepts $hash; nothing to publish"
  exit 0
fi

guard_mainnet
confirm "Publish ${size} KB as $publisher on $NETWORK (about $((size / 10)) NEAR)?"

# shellcheck disable=SC2046
near contract deploy-as-global use-file "$wasm" as-global-hash "$publisher" \
  network-config "$NETWORK" $(signer_args) send

info "Next: ./sh_scripts/admin.sh set-implementation $hash"

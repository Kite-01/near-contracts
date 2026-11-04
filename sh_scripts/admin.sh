#!/usr/bin/env bash
# The factory admin's calls. The admin signs. The factory and admin come from KITE_FACTORY and
# KITE_ADMIN.
#
# Usage: NEAR_NETWORK=testnet ./sh_scripts/admin.sh <command> [args]
#   set-implementation <code hash>   make a published kite the one new transfers use
#   grant-operator <account>         let a Kite server operate (its NEAR operator account)
#   revoke-operator <account>
#   set-admin <account>              hand the factory to a new admin
#   withdraw <account> <NEAR>        send NEAR the factory holds, e.g. withdraw bob.testnet 1.5
#   upgrade [wasm]                   replace the factory's own code (state is kept)

source "$(dirname "$0")/_lib.sh"

cmd="${1:-}"
[[ -n "$cmd" ]] || {
  usage
  exit 0
}

need near jq

[[ $# -gt 0 ]] && shift
contract="$(factory)"
admin="$(factory_admin)"

json() { jq -nc "$@"; }

guard_mainnet
case "$cmd" in
set-implementation)
  hash="${1:?code hash is required}"
  call "$admin" "$contract" set_implementation "$(json --arg h "$hash" '{implementation: $h}')"
  ;;
grant-operator | revoke-operator)
  account="${1:?account is required}"
  call "$admin" "$contract" "${cmd//-/_}" "$(json --arg a "$account" '{account_id: $a}')"
  ;;
set-admin)
  account="${1:?account is required}"
  confirm "Hand $contract to $account? Only $account can call admin methods afterwards."
  call "$admin" "$contract" set_admin "$(json --arg a "$account" '{account_id: $a}')"
  ;;
withdraw)
  to="${1:?recipient is required}"
  amount="${2:?amount in NEAR is required}"
  yocto="$(python3 -c 'import sys; from decimal import Decimal; print(int(Decimal(sys.argv[1]) * 10**24))' "$amount")"
  call "$admin" "$contract" withdraw "$(json --arg to "$to" --arg amount "$yocto" '{to: $to, amount: $amount}')"
  ;;
upgrade)
  wasm="${1:-$(wasm_of factory)}"
  [[ -f "$wasm" ]] || "$SCRIPTS/build.sh"
  confirm "Replace $contract's code with $wasm ($(code_hash "$wasm"))?"
  # shellcheck disable=SC2046
  near contract call-function as-transaction "$contract" upgrade file-args "$wasm" \
    prepaid-gas '300 Tgas' attached-deposit '0 NEAR' \
    sign-as "$admin" network-config "$NETWORK" $(signer_args) send
  ;;
*)
  usage
  die "unknown command '$cmd'"
  ;;
esac

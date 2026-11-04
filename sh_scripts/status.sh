#!/usr/bin/env bash
# Shows the factory's live state on chain.
# Usage: NEAR_NETWORK=testnet ./sh_scripts/status.sh [operator account to check]

source "$(dirname "$0")/_lib.sh"
need near jq

contract="$(factory)"

admin="$(view "$contract" admin | jq -r .)"
implementation="$(view "$contract" implementation | jq -r .)"

echo "factory $contract on $NETWORK"
printf '  %-16s %s\n' admin "$admin"
printf '  %-16s %s\n' implementation "$implementation"

local_wasm="$(wasm_of kite)"
if [[ -f "$local_wasm" ]]; then
  local_hash="$(code_hash "$local_wasm")"
  if [[ "$local_hash" == "$implementation" ]]; then
    echo "  local build      matches the implementation"
  else
    echo "  local build      $local_hash (accepted: $(view "$contract" is_implementation "{\"implementation\":\"$local_hash\"}"))"
  fi
fi

if [[ $# -gt 0 ]]; then
  for operator in "$@"; do
    printf '  %-16s %s\n' "operator?" "$operator: $(view "$contract" is_operator "{\"account_id\":\"$operator\"}")"
  done
fi

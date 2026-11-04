#!/usr/bin/env bash
# Deploys the factory to an existing account and initialises it.
# Usage: NEAR_NETWORK=testnet ./sh_scripts/deploy-factory.sh <factory-account> <admin> <code hash> [operator]
#
# The factory account signs the deploy.

source "$(dirname "$0")/_lib.sh"
need near jq

factory_id="${1:?usage: deploy-factory.sh <factory-account> <admin> <code hash> [operator]}"
admin="${2:?admin account is required}"
implementation="${3:?kite code hash is required (./sh_scripts/publish-deposit.sh prints it)}"
operator="${4:-}"

wasm="$(wasm_of factory)"
[[ -f "$wasm" ]] || "$SCRIPTS/build.sh"

args="$(jq -nc --arg admin "$admin" --arg operator "$operator" --arg implementation "$implementation" \
  '{admin: $admin, operator: (if $operator == "" then null else $operator end), implementation: $implementation}')"

guard_mainnet
confirm "Deploy the factory to $factory_id on $NETWORK with $args?"

# shellcheck disable=SC2046
near contract deploy "$factory_id" use-file "$wasm" \
  with-init-call new json-args "$args" prepaid-gas '100 Tgas' attached-deposit '0 NEAR' \
  network-config "$NETWORK" $(signer_args) send

info "Factory deployed. Save the factory account, admin, implementation hash, and transaction hash from the output wherever you keep deployment records."

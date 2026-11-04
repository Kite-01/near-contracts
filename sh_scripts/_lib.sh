# shellcheck shell=bash

set -euo pipefail

SCRIPTS="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(dirname "$SCRIPTS")"
cd "$ROOT"

# shellcheck disable=SC2034
CONTRACTS=(kite factory)

# shellcheck disable=SC2034
TEST_CONTRACTS=(mock-ft mock-router publisher)

NETWORK="${NEAR_NETWORK:-}"

info() { printf '\033[1;34m==>\033[0m %s\n' "$*" >&2; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die() {
  printf '\033[1;31merror:\033[0m %s\n' "$*" >&2
  exit 1
}

need_network() {
  [[ -n "$NETWORK" ]] || die "NEAR_NETWORK is required for NEAR network operations"
}

need() {
  local cmd
  for cmd in "$@"; do
    command -v "$cmd" >/dev/null 2>&1 || die "$cmd is not installed (see ./sh_scripts/setup.sh)"
  done
}

manifest_of() {
  case "$1" in
  mock-*) echo "tests/mocks/${1#mock-}/Cargo.toml" ;;
  publisher) echo "tests/mocks/publisher/Cargo.toml" ;;
  *) echo "contracts/$1/Cargo.toml" ;;
  esac
}

wasm_of() { echo "target/near/${1//-/_}/${1//-/_}.wasm"; }

code_hash() {
  need python3
  python3 - "$1" <<'EOF'
import hashlib, sys
digest = hashlib.sha256(open(sys.argv[1], "rb").read()).digest()
alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
n = int.from_bytes(digest, "big")
out = ""
while n:
    n, r = divmod(n, 58)
    out = alphabet[r] + out
print("1" * (len(digest) - len(digest.lstrip(b"\0"))) + out)
EOF
}

factory() {
  [[ -n "${KITE_FACTORY:-}" ]] || die "KITE_FACTORY is required"
  echo "$KITE_FACTORY"
}

factory_admin() {
  [[ -n "${KITE_ADMIN:-}" ]] || die "KITE_ADMIN is required"
  echo "$KITE_ADMIN"
}

# How transactions are signed:
#   NEAR_PRIVATE_KEY=ed25519:…  a plaintext key (CI; it shows in the process list)
#   NEAR_SIGN=legacy            ~/.near-credentials, as the old JS near CLI saved them
#   otherwise                   the near CLI's keychain
signer_args() {
  if [[ -n "${NEAR_PRIVATE_KEY:-}" ]]; then
    echo sign-with-plaintext-private-key "$NEAR_PRIVATE_KEY"
  elif [[ "${NEAR_SIGN:-}" == legacy ]]; then
    echo sign-with-legacy-keychain
  else
    echo sign-with-keychain
  fi
}

# view <contract> <method> [json args]
view() {
  need_network
  need near
  local args="${3:-}"
  [[ -n "$args" ]] || args='{}'
  near contract call-function as-read-only "$1" "$2" json-args "$args" \
    network-config "$NETWORK" now 2>/dev/null
}

# call <signer> <contract> <method> <json args> [deposit] [gas]
call() {
  need_network
  need near
  # shellcheck disable=SC2046
  near contract call-function as-transaction "$2" "$3" json-args "$4" \
    prepaid-gas "${6:-100 Tgas}" attached-deposit "${5:-0 NEAR}" \
    sign-as "$1" network-config "$NETWORK" $(signer_args) send
}

usage() {
  awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$SCRIPTS/$(basename "$0")"
}

confirm() {
  [[ "${YES:-}" == 1 ]] && return
  read -r -p "$* [y/N] " answer
  [[ "$answer" == y || "$answer" == Y ]] || die "cancelled"
}

guard_mainnet() {
  need_network
  if [[ "$NETWORK" == mainnet ]]; then
    confirm "This sends a transaction on NEAR mainnet. Go on?"
  fi
}

#!/usr/bin/env bash
# Runs the tests.
# Usage: ./sh_scripts/test.sh [unit|sandbox|all] [extra cargo test args]
#   unit     types, kite and factory, natively (default)
#   sandbox  builds every contract, then runs tests/sandbox on a local nearcore sandbox
#   all      both

source "$(dirname "$0")/_lib.sh"
need cargo

suite="${1:-unit}"
[[ $# -gt 0 ]] && shift

unit() {
  info "Unit tests"
  cargo test -p types -p kite -p factory "$@"
}

sandbox() {
  "$SCRIPTS/build.sh" --all
  info "Sandbox tests"
  cargo test -p sandbox-tests "$@"
}

case "$suite" in
unit) unit "$@" ;;
sandbox) sandbox "$@" ;;
all)
  unit "$@"
  sandbox "$@"
  ;;
*) die "unknown suite '$suite': use unit, sandbox or all" ;;
esac

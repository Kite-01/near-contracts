#!/usr/bin/env bash
# Checks formatting and runs clippy, with warnings as errors.
# Usage: ./sh_scripts/lint.sh [--fix]

source "$(dirname "$0")/_lib.sh"
need cargo

if [[ "${1:-}" == --fix ]]; then
  info "cargo fmt"
  cargo fmt --all
  info "cargo clippy --fix"
  cargo clippy --workspace --all-targets --fix --allow-dirty --allow-staged
else
  info "cargo fmt --check"
  cargo fmt --all -- --check
fi

info "cargo clippy"
cargo clippy --workspace --all-targets -- -D warnings

if command -v shellcheck >/dev/null; then
  info "shellcheck"
  shellcheck -x "$SCRIPTS"/*.sh
fi

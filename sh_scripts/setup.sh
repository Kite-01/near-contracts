#!/usr/bin/env bash
# Installs what building, testing and deploying need: the wasm target, cargo-near and the near CLI.
#
# Usage: ./sh_scripts/setup.sh [--no-cli]

source "$(dirname "$0")/_lib.sh"

CARGO_NEAR_VERSION=0.22.0
NEAR_CLI_VERSION=0.30.1

need rustup cargo

info "Rust toolchain (rust-toolchain.toml)"
rustup show active-toolchain >/dev/null
rustup target add wasm32-unknown-unknown

if cargo near --version 2>/dev/null | grep -q "$CARGO_NEAR_VERSION"; then
  info "cargo-near $CARGO_NEAR_VERSION already installed"
else
  info "cargo-near $CARGO_NEAR_VERSION"
  cargo install --locked cargo-near --version "$CARGO_NEAR_VERSION"
fi

if [[ "${1:-}" != --no-cli ]]; then
  if near --version 2>/dev/null | grep -q "$NEAR_CLI_VERSION"; then
    info "near CLI $NEAR_CLI_VERSION already installed"
  elif command -v npm >/dev/null; then
    info "near CLI $NEAR_CLI_VERSION (npm)"
    npm install -g "near-cli-rs@$NEAR_CLI_VERSION"
  else
    info "near CLI $NEAR_CLI_VERSION (cargo)"
    cargo install --locked near-cli-rs --version "$NEAR_CLI_VERSION"
  fi
fi

command -v jq >/dev/null || warn "jq is missing; the deploy scripts need it"
info "Ready. Try ./sh_scripts/test.sh"

# Contributing

Thanks for helping with Kite. This repo holds the NEAR half of its on-chain code: the factory and the per transfer deposit accounts that receive, swap, settle and refund each payment. Because they hold funds, changes here get more scrutiny than elsewhere, and tests are not optional.

Found a security issue? Don't open an issue or pull request; follow [SECURITY.md](SECURITY.md).

## Before you start

For anything beyond a small fix, open an issue first and describe the change. Contract changes affect every Kite server that uses them, so it's worth agreeing on the approach before you write it.

## Setup

You need Rust (`rustup`; `rust-toolchain.toml` picks the version), Python 3 and jq. Then:

```sh
./sh_scripts/setup.sh     # wasm target, cargo-near, and the near CLI
./sh_scripts/test.sh      # unit tests
```

## Making a change

1. Fork the repo and branch from `main`.
2. Make the change, with tests that fail without it:
   - unit tests next to the code (`contracts/*/src/tests.rs`) for logic in one contract;
   - sandbox tests (`tests/sandbox/tests/sandbox.rs`) for anything that crosses contracts: deploy, settle, swaps, refunds, recovery and token registration.
3. Keep the interface stable where you can. Say so in the pull request if a change touches:
   - the methods, their arguments or the `kite` events: the [server](https://github.com/Kite-01/server) calls and parses them;
   - `Transfer`, `DepositState` or anything else in a `StateInit`: it changes every predicted address, so `vector::fixed_vector` and the server's `kiteNearContract` test must change together;
   - `kite`'s code at all: it needs a new global publish, which costs NEAR, and `set_implementation`.
4. Run `./sh_scripts/lint.sh` and `./sh_scripts/test.sh all`; both must pass.
5. Open a pull request saying what changed and why. Link the issue if there is one.

Write comments only where the code can't explain itself, such as an invariant or why steps run in a given order.

## License

By contributing, you agree your contributions are licensed under the [MIT License](LICENSE).

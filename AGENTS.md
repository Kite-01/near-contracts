# AGENTS.md

Notes for AI agents working in this repository, or reviewing it. Start with the [README](README.md): it explains how the contracts work, the NEAR account model, and every script.

## Commands

```sh
./sh_scripts/setup.sh
./sh_scripts/lint.sh
./sh_scripts/build.sh

# unit tests (types, kite, factory)
./sh_scripts/test.sh

# builds every contract, then runs tests/sandbox
./sh_scripts/test.sh sandbox

# the testnet deployment as the chain sees it (read-only)
KITE_FACTORY=<factory> NEAR_NETWORK=testnet ./sh_scripts/status.sh
```

- Run one test with `./sh_scripts/test.sh unit <name>` or `./sh_scripts/test.sh sandbox <name>`.
- The sandbox downloads nearcore and raises its open-file limit to 65535. If it stops right away, the container's limit is lower: say so rather than skipping the tests.
- `cargo near build` must run from the repository root or with `--manifest-path`; the scripts do that.

## Layout

- `contracts/types/` (crate `types`, used as `kite_types`): `Transfer`, `DepositState` and the address derivation, shared by both contracts. `vector::fixed_vector` pins a transfer to its `0s…` id.
- `contracts/kite/`: the per-transfer account, published once as a global contract. `settle` runs as resumable steps.
- `contracts/factory/`: admin, operators, allowed implementations, `predict_address`, and the operator calls (`deploy`, `settle`, `refund`, `recover`).
- `tests/sandbox/`: near-workspaces tests against the wasm in `target/near/`.
- `tests/mocks/`: test doubles: `ft` (a NEP-141 that also wraps NEAR), `router` (fixed-rate swaps), `publisher` (publishes a global contract, which near-workspaces can't).
- `sh_scripts/`: setup, build, test, lint, and deploying and administering a deployment.

## Rules

- **Secrets**: never commit keys, `.near-credentials` or seed phrases. The scripts sign through the near CLI's keychain, or `NEAR_PRIVATE_KEY` from the environment.
- **Don't send transactions** (`publish-deposit.sh`, `deploy-factory.sh`, `admin.sh`) unless the user asks. Publishing `kite` burns about 18 NEAR. `status.sh` only reads.
- **Addresses**: changing `Transfer`, `DepositState` or their borsh layout changes every deposit address. Update `vector::fixed_vector`, and tell the user that the server's `src/shared/__tests__/kiteNearContract.test.ts` pins the same vector.
- **Events and methods** are the server's interface. Keep their names and fields, or say what breaks.
- **Style**: `cargo fmt`, clippy clean, and comments as sparse as in the code around them.

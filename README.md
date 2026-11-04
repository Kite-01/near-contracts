# Kite NEAR Contracts

The NEAR smart contracts behind [Kite](https://github.com/Kite-01/server), an open-source crypto payment processor. Each payment gets its own NEAR account, at an address known before anyone pays. The account receives the customer's NEAR or tokens, swaps them if needed, pays fees and settles to the merchant, or refunds the customer.

Written in Rust with near-sdk 5.30.

## How it works

A Kite server creates a transfer: who gets paid, in which token, the minimum to receive, fees, a refund account, an expiry, and the swap route if the customer pays in another token.

1. **Before payment**: `factory.predict_address(transfer)` gives the account the customer pays into: `0s` followed by 40 hex characters. Nothing is deployed yet.
   - For a token payment, the operator calls `factory.register(transfer)`. A NEAR token only sends to accounts registered on it, so this registers the predicted account on the pay-in token, for about 0.00125 NEAR.
   - A payment in native NEAR needs nothing up front.
2. **Paying**: the customer sends the exact amount to the predicted account, from any wallet or exchange.
3. **Deploying**: once funds arrive, the operator calls `factory.deploy(transfer)`. It does three things:
   - creates the account with the `kite` contract in it;
   - funds the account with a 0.001 NEAR reserve, which pays the 1 yoctoNEAR that each token call needs attached;
   - registers the account on the tokens it will hold.
4. **Settling**: `factory.settle(transfer)` runs these steps in order:
   - check the balance;
   - swap through Rhea if the customer paid in another token, wrapping native NEAR first;
   - pay each fee;
   - send the rest to the merchant;
   - send back anything the customer overpaid.
5. **Refunding**: `factory.refund(transfer)` returns what the account holds to the refund account. Before expiry only the operator can call it. After expiry anyone can, so a customer is never stuck waiting on a server.
6. **Recovering**: `factory.recover(transfer, token, destination)` lets the operator retrieve tokens left behind, such as ones sent after settlement, or a refund the refund account couldn't accept.
7. **Closing**: once the transfer is settled or refunded, `factory.close(transfer)`:
   - unregisters the account from its tokens;
   - returns the reserve and the registration deposits to the operator, leaving 1 µNEAR in the account.

   Anything still owed to the customer stays in the account.

A transfer that's never paid is never deployed. It costs one registration for a token payment, and nothing for native NEAR.

### Settling takes several steps

On NEAR, a call to another contract runs later, in its own receipt, so a settlement can't happen in one atomic step. `settle` saves its progress in the account after each step:

- check the balance;
- wrap native NEAR;
- swap;
- read the output;
- unwrap;
- pay.

How a settlement that stops is handled:

- **A step before the swap fails**: the transfer goes back to `Open`, untouched, and can be settled or refunded later.
- **A step after the swap fails**: the transfer stays `Executing`. The next `settle` resumes from that step, so nothing is paid twice.
- **The swap's output hasn't landed**: Rhea sends it in its own receipt, which can arrive after the swap reports back. `settle` stops with `OutputPending`, and the next call reads the output again.
- **Gas runs out**: 300 Tgas doesn't always cover a native swap, so `settle` stops with `OutOfGas`. A native swap usually takes two calls.
- **Every stop** emits a `settle_stopped` event naming the step and the reason.
- **Locking**: while a settlement is in flight, the account refuses another `settle` for 10 minutes. This stops two calls from running the same steps at once. A settlement that stops normally clears the lock straight away. The 10 minutes only matter if a step never reports back.

### Details worth knowing

- Native NEAR and any NEP-141 token can be paid in or settled.
- **Native NEAR counts in full before deployment.** Sending NEAR to an account that doesn't exist yet creates a plain account with no code. `deploy` later installs the contract on that same account, and everything it holds counts as the payment.
- **Deploying twice is harmless.** A new account starts `Uninitialized`, and `fund` opens it in the same step that creates it. A second `deploy` changes nothing and sends its reserve back.
- **A refund that can't be delivered never blocks settlement.** For example, the refund account might not be registered on the token. The amount stays in the account for the operator to `recover`, and `refunded` reports what was actually sent.
- **Only the factory controls an account.** It can't be re-initialised, and nobody else can settle, refund early or recover from it. It has no access keys, so no one can sign transactions as it.

## The NEAR account model

If you know Ethereum, NEAR accounts take some getting used to. A short guide to what matters here:

- **Accounts have names.**
  - **Named accounts** like `kite-factory.near` or `alice.testnet` are created by their parent account.
  - **Implicit accounts** are 64 hex characters, derived from a public key, and exist once someone sends them NEAR. Kite's operator is an implicit account.
  - **Deterministic accounts** (`0s…`, [NEP-616](https://github.com/near/NEPs/pull/616)) are derived from the code and initial state they'll run, so their id is known in advance, much like CREATE2 on Ethereum. Each transfer gets one.
- **One account can run one contract**, and its code and storage live in the account itself. There's no separate "contract address": a contract is an account with code.
- **Access keys sign transactions.** An account can hold several keys, each either full access or limited to calling one contract. Deposit accounts have none, so only their contract's code moves their funds.
- **Storage costs NEAR.** An account must hold 1 NEAR per 100 KB it stores, locked rather than spent. Accounts using up to 770 bytes are exempt ("zero-balance accounts"). Most deposit accounts fit, and larger ones lock exactly what they use.
- **Tokens keep balances in their own contract** ([NEP-141](https://github.com/near/NEPs/blob/master/neps/nep-0141.md)). An account must register on a token before it can hold it, paying the token's storage deposit (usually 0.00125 NEAR). Registering doesn't need the account to exist, which is how a transfer is paid before it's deployed. Unregistering returns the deposit.
- **Contract code can be shared** ([NEP-591](https://github.com/near/NEPs/pull/591)). `kite` is published once as a global contract, by its code hash, and every deposit account runs it by reference. Publishing costs about 10 NEAR per 100 KB, burnt once per version. Each deposit account then stores only its own state.
- **Calls between contracts are asynchronous.** Each call runs in its own receipt, usually a block later, and the caller learns the result in a callback. That's why settling runs in steps and can resume.

## Contract interface

### `factory`

Deployed to a named account. It holds the admin, the operators and the `kite` versions it accepts.

| Method                                                                              | Who                           | What it does                                                                                                                                                                             |
| ----------------------------------------------------------------------------------- | ----------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `new(admin, operator?, implementation)`                                             | once                          | Initialises the factory with an admin, an optional first operator and the `kite` code hash to use.                                                                                       |
| `predict_address(transfer)`                                                         | view                          | The transfer's deposit account.                                                                                                                                                          |
| `register_deposit(transfer)`                                                        | view                          | The most `register` needs attached: 0.0025 NEAR for a token payment, 0 for NEAR.                                                                                                         |
| `deploy_deposit(transfer)`                                                          | view                          | What `deploy` needs attached: the reserve and 0.0025 NEAR per token, plus 0.03 NEAR for an unusually large transfer.                                                                     |
| `admin()`, `is_operator(account_id)`, `implementation()`, `is_implementation(hash)` | view                          | Roles and accepted versions.                                                                                                                                                             |
| `register(transfer)`                                                                | operator                      | Registers the predicted account on the pay-in token. The whole attached deposit, up to 0.0025 NEAR, goes to the token, so attaching the token's own minimum leaves nothing to refund.    |
| `deploy(transfer)`                                                                  | operator                      | Creates, funds and registers the account.                                                                                                                                                |
| `settle(transfer)`                                                                  | operator                      | Runs or resumes the settlement.                                                                                                                                                          |
| `refund(transfer)`                                                                  | operator; anyone after expiry | Refunds an open transfer.                                                                                                                                                                |
| `recover(transfer, token?, destination?)`                                           | operator                      | Sends leftover tokens, or NEAR when `token` is null, to `destination` or the refund account. Refused while a settlement is in flight, and for the transfer's own tokens while it's open. |
| `close(transfer)`                                                                   | operator                      | Unregisters a finished transfer's account and returns its deposits to the caller.                                                                                                        |
| `set_implementation(hash)`                                                          | admin                         | Accepts a newly published `kite` and uses it for new transfers.                                                                                                                          |
| `grant_operator(account_id)`, `revoke_operator(account_id)`                         | admin                         | Manages operators.                                                                                                                                                                       |
| `set_admin(account_id)`                                                             | admin                         | Hands over the factory.                                                                                                                                                                  |
| `withdraw(to, amount)`                                                              | admin                         | Sends NEAR the factory holds, such as storage deposits tokens refunded to it.                                                                                                            |
| `upgrade()`                                                                         | admin                         | Replaces the factory's code with the wasm passed as the call's input. Its state is kept.                                                                                                 |

### `kite`

The contract each deposit account runs. Only the factory calls its methods. It also has the views `get_factory`, `get_transfer`, `get_status` and `get_progress`.

A transfer's status is one of:

| Status          | Meaning                            |
| --------------- | ---------------------------------- |
| `Uninitialized` | Created, but not yet funded        |
| `Open`          | Waiting to be settled or refunded  |
| `Executing`     | Settling                           |
| `Settled`       | Finished: paid out                 |
| `Refunded`      | Finished: returned to the customer |

### Events

Both contracts log [NEP-297](https://github.com/near/NEPs/blob/master/neps/nep-0297.md) events with standard `kite`:

| Event             | When                                       |
| ----------------- | ------------------------------------------ |
| `deployed`        | the factory created an account             |
| `settled`         | everything is paid                         |
| `fee_paid`        | one fee was paid                           |
| `refunded`        | the account refunded the customer          |
| `excess_refunded` | an overpayment went back                   |
| `recovered`       | leftover tokens were sent out              |
| `closed`          | deposits were returned                     |
| `settle_stopped`  | a settlement paused or went back to `Open` |

The Kite server reads these to follow each transfer.

## Upgrades

- **`kite` code**:
  - publish the new version (`sh_scripts/publish-deposit.sh`);
  - then have the admin run `set_implementation` with its code hash.

  Each transfer names the version it runs (`transfer.implementation`), and the factory accepts every version it has used. So a transfer created before an upgrade still settles, refunds and recovers with its own code, and no upgrade can change the code of an account that already holds funds.

- **`factory` code**: the admin calls `upgrade` with the new wasm. Its address, roles and accepted versions stay the same.

A new `kite` version changes the predicted address of every new transfer, since the address commits to the code hash. `types` has a fixed test vector (`vector::fixed_vector`), and the server checks its own address derivation against the same transfer and id.

## Development

You need:

- Rust, through `rustup`; `rust-toolchain.toml` pins the version;
- Python 3;
- jq.

`sh_scripts/setup.sh` installs the rest.

```sh
# wasm target, cargo-near and the near CLI
./sh_scripts/setup.sh

# unit tests
./sh_scripts/test.sh

# build every contract, then test them on a local NEAR sandbox
./sh_scripts/test.sh sandbox

# rustfmt, clippy and shellcheck
./sh_scripts/lint.sh

# build kite and factory, and print their code hashes
./sh_scripts/build.sh
```

The sandbox tests run the real contracts on a local nearcore node that near-workspaces downloads. They use three test doubles in `tests/mocks/`:

- `ft`: a token anyone can mint, which also wraps NEAR;
- `router`: swaps at a fixed rate;
- `publisher`: publishes `kite` as a global contract, which near-workspaces can't do itself.

The sandbox raises its open-file limit to 65535 and stops if it can't, which some containers don't allow.

## Scripts

Everything in `sh_scripts/` runs from any directory and shares `_lib.sh`.

**Settings**, all read from the environment:

| Variable           | Used by                                       | Meaning                                                                             |
| ------------------ | --------------------------------------------- | ----------------------------------------------------------------------------------- |
| `NEAR_NETWORK`     | anything that reads or writes the chain       | `testnet` or `mainnet`. Required: there is no default.                              |
| `KITE_FACTORY`     | `admin.sh`, `status.sh`, `publish-deposit.sh` | The factory account.                                                                |
| `KITE_ADMIN`       | `admin.sh`                                    | The admin account, which signs.                                                     |
| `NEAR_PRIVATE_KEY` | anything that signs                           | An `ed25519:…` key to sign with. Handy in CI, but it's visible in the process list. |
| `NEAR_SIGN=legacy` | anything that signs                           | Sign with `~/.near-credentials`, where the old JavaScript near CLI kept keys.       |
| `YES=1`            | anything that signs                           | Skip the confirmation prompt. Mainnet still asks once.                              |

Without `NEAR_PRIVATE_KEY` or `NEAR_SIGN`, the scripts sign with the near CLI's keychain (`near account import-account` adds a key to it). Nothing in `sh_scripts/` records deployments: keep the accounts, code hashes and transaction ids somewhere you control.

### `setup.sh [--no-cli]`

Gets a machine ready:

- adds the `wasm32-unknown-unknown` target;
- installs cargo-near 0.22.0;
- installs near CLI 0.30.1 through npm, or cargo if npm is missing.

`--no-cli` skips the near CLI, which CI doesn't need. Running it again only installs what's missing.

### `build.sh [--all]`

Builds `kite` and `factory` with `cargo near build` into `target/near/<name>/<name>.wasm`, then prints each one's size and code hash. The code hash is the base58 SHA-256 of the wasm, which is what the factory accepts and what a global contract is published under. `--all` also builds the test doubles the sandbox needs.

Builds aren't reproducible yet: the toolchain and the checkout's path change the wasm, and so its code hash. Compare a deployment against the wasm you published, not a fresh build.

### `test.sh [unit|sandbox|all] [cargo test args]`

| Suite                | What it runs                                                              |
| -------------------- | ------------------------------------------------------------------------- |
| `unit` (the default) | The unit tests of `types`, `kite` and `factory`, natively and in seconds. |
| `sandbox`            | `build.sh --all`, then the sandbox tests in `tests/sandbox`.              |
| `all`                | Both.                                                                     |

Anything after the suite goes to `cargo test`, so `./sh_scripts/test.sh sandbox close_after_a_refund` runs one test.

### `lint.sh [--fix]`

- Checks formatting with `cargo fmt`.
- Runs clippy with warnings as errors.
- Runs shellcheck on the scripts, if it's installed.

`--fix` formats and applies clippy's suggestions first. CI runs it without `--fix`.

### `publish-deposit.sh <publisher> [wasm]`

Publishes `kite` as a global contract by its code hash, signed and paid for by `<publisher>`:

- **Cost**: about 10 NEAR per 100 KB of wasm, burnt; `kite` is about 185 KB.
- **Wasm**: defaults to `target/near/kite/kite.wasm`, building it if needed.
- **Nothing to do**: with `KITE_FACTORY` set, it first asks the factory, and stops if that version is already accepted.

Publishing doesn't change what the factory uses; run `admin.sh set-implementation` next.

```sh
NEAR_NETWORK=testnet KITE_FACTORY=kite-factory.testnet ./sh_scripts/publish-deposit.sh kite-admin.testnet
```

### `deploy-factory.sh <factory> <admin> <code hash> [operator]`

Deploys `factory` to an existing account and initialises it with the admin, the `kite` code hash and an optional first operator. The factory account signs.

Choices to make here:

- **Admin**: use a separate account that the server never holds, so a leaked server key can't change the factory.
- **Operator**: your Kite server's NEAR operator account. You can add it later with `admin.sh grant-operator`.

```sh
NEAR_NETWORK=testnet ./sh_scripts/deploy-factory.sh kite-factory.testnet kite-admin.testnet <code hash> <operator>
```

### `admin.sh <command> [args]`

The admin's calls, signed by `KITE_ADMIN` on `KITE_FACTORY`. Run it with no command for help.

| Command                          | What it does                                                                          |
| -------------------------------- | ------------------------------------------------------------------------------------- |
| `set-implementation <code hash>` | Use a published `kite` for new transfers. Earlier transfers keep their version.       |
| `grant-operator <account>`       | Let a Kite server operate: settle, refund, recover, register, deploy and close.       |
| `revoke-operator <account>`      | Take that back.                                                                       |
| `set-admin <account>`            | Hand the factory to a new admin. It asks first, since only the new admin can undo it. |
| `withdraw <account> <NEAR>`      | Send NEAR the factory holds, in NEAR (for example `1.5`).                             |
| `upgrade [wasm]`                 | Replace the factory's code. Defaults to `target/near/factory/factory.wasm`.           |

```sh
export NEAR_NETWORK=testnet KITE_FACTORY=kite-factory.testnet KITE_ADMIN=kite-admin.testnet
./sh_scripts/admin.sh grant-operator <operator>
./sh_scripts/admin.sh set-implementation <code hash>
```

### `status.sh [operator…]`

Reads the factory and sends nothing. It prints:

- the factory's admin;
- the `kite` version it uses;
- whether your local build matches that version, or whether the factory accepts it;
- whether each account you name is an operator.

```sh
NEAR_NETWORK=testnet KITE_FACTORY=kite-factory.testnet ./sh_scripts/status.sh <operator>
```

### Deploying from scratch

1. **Build**: `./sh_scripts/build.sh`.
2. **Publish `kite`**: `./sh_scripts/publish-deposit.sh <publisher>`. Note the code hash.
3. **Deploy the factory**: `./sh_scripts/deploy-factory.sh <factory> <admin> <code hash> [operator]`.
4. **Grant your Kite server's NEAR operator**: `./sh_scripts/admin.sh grant-operator <operator>`. Skip this if you named it in step 3.
5. **Configure the server**: put the factory account in its NEAR chain config. The server refuses a factory where its operator isn't one.
6. **Fund the operator**: it pays gas and deposits for every transfer, with most deposits coming back at `close`.

## CI

The **CI** workflow runs on every pull request and every push to `main`:

- **Lint**: `lint.sh`.
- **Unit tests**: `test.sh unit`.
- **Sandbox tests**: `test.sh sandbox`. This job uploads the built wasm as an artifact and puts the code hashes in the run summary.

Nothing is deployed from CI.

## Project layout

```text
contracts/
  types/       Transfer, DepositState and the address derivation, shared by both contracts
  kite/        the deposit account: settle, refund, recover, close
  factory/     roles, versions, predict_address, and the calls into deposit accounts
tests/
  sandbox/     end-to-end tests on a local NEAR node
  mocks/       ft, router and publisher test doubles
sh_scripts/    setup, build, test, lint, publish, deploy, admin and status
```

## Contributing

Contributions are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for setup and tests.

To report a vulnerability, follow [SECURITY.md](SECURITY.md) rather than opening an issue.

## License

[MIT](LICENSE)

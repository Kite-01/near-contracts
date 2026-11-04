# Security policy

These contracts hold and move funds, so we take reports seriously and will work with you to fix them.

## Reporting a vulnerability

Email [security@usekite.site](mailto:security@usekite.site) with:

- what the issue is and what an attacker could do with it
- the contract and method involved, and the commit, code hash or account you looked at
- steps or a test that reproduces it, if you have one

Please don't open a public issue, pull request or discussion about it, and don't exploit it on a live network beyond what's needed to show it exists.

## What happens next

- We reply within 3 working days to confirm we've received it.
- We keep you updated while we investigate and fix it.
- Once a fix is deployed, we publish the details and credit you, unless you'd rather stay anonymous.

## In scope

- `factory`, `kite` and `types` in `contracts/`
- The deployments recorded under `deployments/`, and the `0s…` deposit accounts they create
- `predict_address`: anything that lets two transfers share a deposit account, or a deposit account run code the factory didn't allow

The contracts in `tests/mocks/` are test doubles and out of scope. Issues in near-sdk, Rhea, `wrap.near` or a token contract should go to those projects, though we'd like to hear if one affects Kite.

## Known limits

These are by design; reports that only restate them are out of scope.

- `recover` lets the factory's operator send tokens that aren't the transfer's own anywhere.
- A settlement in flight locks the deposit for 10 minutes, after which another `settle` can take it over.
- A token transfer that is never paid leaves its 0.00125 NEAR registration on the token, since the account that could unregister never exists.

<!-- SPDX-License-Identifier: Apache-2.0 -->

# The suite contract

The vocabulary every part of the suite shares (Wave 4c §4.5): the engine,
the desk, Kvasir, the assistant and any registered app build against these
documents, and each carries a test that its own documents validate against
them.

`VERSION` is the contract version. **It changes only by a pull request that
says so**, in its title, and that bumps `VERSION` and adds a new `vN/`
directory beside the old one, which stays.

| version | documents | since |
|---|---|---|
| 1 | [`v1/`](v1/) | Wave 4c slice A7, 2026-09-09 |

## What version 1 fixes

| document | what it fixes | spec |
|---|---|---|
| `entitlements.schema.json` | the five entitlements, the engine's ladder of four and the orthogonal `assist`; the `roles` claim as plain strings | §5.2 |
| `headers.schema.json` | `X-Nils-Ceiling` (downgrade only), `X-Nils-Actor` and the actor object the engine records (`absent` as its own value), `Idempotency-Key` | §5.5, §6.3 |
| `purpose.schema.json` | a purpose registry entry; the content classes `catalog`, `rows`, `identifiers`; the localities; the refusal layers | §8.3 |
| `capabilities.schema.json` | the deployment capabilities document the desk builds: the engine's own document verbatim, Kvasir, the assistant, the apps, the person, the desk; the policy row | §4.3, §6.5 |
| `app.schema.json` | an app registry entry the desk proxies | §4.4 |
| `station.schema.json` | a station manifest; the proposal kinds; the terminal reasons | §9.4, §9.6 |
| `vectors/trust-list.json` | the trust list test vectors: two issuers, two keys, eight cases with their outcomes; the keys and JWKS documents beside it | §5.3 |

The vectors are data. An implementation mints each case with the named key
(RS256, the key id in the header, `exp` and `iat` at run time, in the past
for an expired case), presents it, and compares what happened with `expect`.
The engine runs them in `engine/crates/nils/tests/serve.rs`; Kvasir runs the
same file.

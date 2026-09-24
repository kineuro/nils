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
| 2 | [`v2/`](v2/) | 2026-09-15: grants and detail in place of the role ladder |
| 3 | [`v3/`](v3/) | 2026-09-24: version 2 with the grants of record 42 R7: the model registry's `models:see` and `models:work`, and the campaigns' `campaigns:see` and `campaigns:work` |

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

## What version 2 changes

Version 2 is version 1 with grants and detail in place of the role ladder. A
caller holds a set of grants, each naming a page and how far the caller goes
there (`see`, or `work`, which includes see; the assistant has `use`), and a
detail, `plain`, `quasi` or `sensitive`, that says how much of a record it
sees. The other documents and vectors are version 1's.

| document | what changes |
|---|---|
| `grants.schema.json` | replaces `entitlements.schema.json`: the grants, the detail and its order, and the ladder steps a ceiling or a binding still names |
| `capabilities.schema.json` | the engine's caller and the person carry `grants` and `detail`, and for one release `roles` as the ladder steps up to the detail; a policy row names its `grant`, one or an array meaning any of, with `also` for a second grant a door needs and `detail` for the lowest detail |
| `headers.schema.json` | the ceiling is still a ladder step: the caller keeps the grants its set holds and `assistant:use`, and detail is lowered to the step's |
| `vectors/grants.json` | the ladder's sets, and how a caller is resolved from a token's claims, narrowed by a ceiling, named by a token's list, and known by its principal, which only a trust entry that keeps subjects takes as the token spells it |

A ladder name stands for its set wherever one is still met: a `--role`
binding, a named token's list, a legacy entitlement, a ceiling. The engine
runs the grants vectors in `engine/crates/nils/tests/serve.rs` beside the
trust list vectors; Kvasir, the assistant and the desk run the same file.

## What version 3 changes

Version 3 is version 2 with four grants more, from record 42 R7.

`models:see` reads the registered models and their cards, and
`models:work` registers, admits, promotes and retires them. The reviewer's
set holds `models:see`, since a reviewer reads which model answered what;
the operator's and the admin's hold both.

`campaigns:see` reads a campaign, its items and its answers, and a label
set; `campaigns:work` makes a campaign, claims its items under a lease,
answers them, gives them back, posts an item's external metric and exports
its labels. Closing a campaign writes decisions, so it needs `review:work`
beside `campaigns:work`. A rater is not a reviewer of the whole queue, which
is why the grants are their own: no ladder set holds them but admin.

A derivative is not a grant of its own: its doors are the Pipelines page's
(`pipelines:see`, `pipelines:work`), as in version 2.

The other documents are version 2's.

| document | what changes |
|---|---|
| `grants.schema.json` | the four grants in the vocabulary, which is 28 |
| `capabilities.schema.json` | the engine's contracts name the model contract's version |
| `vectors/grants.json` | the sets and every expectation that holds one |

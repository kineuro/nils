<!-- SPDX-License-Identifier: Apache-2.0 -->

# The model contract

A registered model (D15, record 42): the card that says what it is, and the
lifecycle it passes through. The engine keeps the registry of models whose
answers become registry facts (encoders, heads, passes, segmenters), and
Kvasir keeps its own lifecycle for the language models it serves. The two
stores are separate; the idea is written once, here, so that both read the
same card and mean the same thing by each state.

`VERSION` is the contract version. **It changes only by a pull request that
says so**, in its title, and that bumps `VERSION` and adds a new `vN/`
directory beside the old one, which stays.

| version | documents | since |
|---|---|---|
| 1 | [`v1/`](v1/) | Record 42 slice S2, 2026-09-24 |

## What version 1 fixes

| document | what it fixes |
|---|---|
| `card.schema.json` | the card: identity by the digest of the canonical artifact, name and version, kind, task and slot, the encoders a head reads in order (by digest; record 43 lets a head read several), the threshold its proposals are staged at (record 43), the label set it was fitted on (by digest), the pack version, the image that runs it, metrics, preprocessing, parameters, intended use, limits and the runtime a check ran under |
| `lifecycle.schema.json` | the four states, the transitions and their events, the check record that admits a model, and a registered model as the engine answers it |

The rules both implementations keep:

1. A model is identified by its digest. Registering the same digest twice is refused, and so is a second model under a name and version already taken.
2. Admission records a check. One that passed moves a registered model to admitted and stays on it; one that failed is kept as an event and leaves the state as it was.
3. Promotion is refused unless the model was admitted. Promoting a model retires the one promoted before it in the same task and slot, so a slot has at most one promoted model.
4. Every transition is an event with who and when, and an audit row. Nothing is deleted.

In the engine a model's answer is a decision with `author_kind` model and the
model's id, refused unless the model is registered and admitted or promoted,
and staged until a person commits it (record 42 R6). The slots are `site`,
one site-wide model per task (R4), and `cohort:<name>`, a model of one
cohort in a slot of its own.

Kvasir adopts the card when it next touches its lifecycle, by adding the
digest of the weights file it already computes on download and emitting the
card; its slot is `backend:<name>`.

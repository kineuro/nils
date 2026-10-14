# 47 · Kvasir serves the group

Opened 2026-09-24. **Merged 2026-09-24**: kineuro/kvasir#33, kineuro/nils-desk#125, kineuro/nils#215, kineuro/nils-assistant#45; released in alpha.38 (Kvasir alpha.8), and Kvasir took over the group's model server on 2026-09-24 (slice M: the smoke test passed through the public URL with both keys, and a swap to the 27B and back works). Beside Wave 6, not on its critical path. The design is `studies/2026-09-24-kvasir-gateway/study.md`; what runs today is in the operators' notes.

## The ask

> Nima, 2026-09-23: Kvasir becomes an app that hands out an OpenAI- and Anthropic-style URL and key to any client, with two models on one key, their specs in the API, full context, and one GPU isolated from people.

> Nima, 2026-09-24: "we should be able to add, remove models and having the url and api token (automatically on nils-desk) see all available models where we can choose." And: "try to have it as new release version of kvasir."

modelgate, written on 2026-09-23 in the model server, does the serving today. It is an interim. This wave puts what it does into Kvasir, keeping the URL, the keys and the model names, so Droid and every other client need no change. It ships as a Kvasir release, and the group's install takes it through the ordinary update.

## Rulings

Nima's answers of 2026-09-24:
- Build freely: nothing of Kvasir changed.
- The stations stay on the 27B until Flash-Next passes the station bench.
- The NILS server gets its own key.
- Keys stay inside the group until Flash-Next's licence is checked for outside use.

The rest are my defaults, each reversible.

| | Ruling |
|---|---|
| R1 | One codebase, no mode switch. Kvasir inside an install routes the stations; Kvasir on the model server serves the card. They differ only in configuration |
| R2 | New doors forward a request as it is, changing only `model` and the thinking default, and read the usage for the ledger. Kvasir's own pi-messages door moves to `/v1/pi/messages`; the old path tells the two apart by their body for one release |
| R3 | An install uses a remote Kvasir, or any OpenAI-compatible server, as one backend of kind "model server", set up by URL and key. Its models and specs come from its `/v1/models`, and the admin ticks which to admit |
| R4 | The key lives only in Kvasir (D45), sealed like every credential. The desk never holds it |
| R5 | The NILS server's key reaches it through the release: `nils setup --model-server URL --model-key-file F` on update. modelgate's exception for the install's address is removed only after a station call has worked with the key |
| R6 | The server's own administration (client keys, the default model, the swap) lives in Kvasir's own CLI and a small page of its own, not in an install's desk. Admin and batch doors stay off the public route |
| R7 | Keys only for the group while Flash-Next is served (Qwen Community License, internal use). The 27B is Apache-2.0 |
| R8 | The ledger keeps 90 days of counts per key; no quotas yet |
| R9 | The Jev-style batch door (label log-probabilities over a shared prefix, for System 1's referee) waits for record 39's Phase 0 gate |

## Slices

| | Repo | What | Proven by |
|---|---|---|---|
| K1 | kvasir | **The doors as the client sent them.** OpenAI chat and completions with tools and images, a real Anthropic `/v1/messages` with `count_tokens`, `/v1/responses`; the thinking default; the context guard; usage into the ledger. pi-messages to `/v1/pi/messages` | modelgate's `smoke.sh` against Kvasir: model list, chat, a tool round trip, a stream, Anthropic messages; the assistant's turns still work |
| K2 | kvasir | **Client keys.** Keys per client in the database, scopes and revocation, the ledger per key; import of modelgate's hashes so today's keys keep working | Nima's existing key works unchanged after import; a revoked key is refused |
| K3 | kvasir | **The card.** A group of models sharing one GPU, one loaded at a time, with two drivers: Docker for SGLang (ported from modelgate, cold swap, return to the default after idle) and record 24's llama.cpp router. `/v1/models` in modelgate's shape plus a model card; `/health`; only the loaded model warms, which closes kineuro/kvasir#8 | A swap from Flash-Next to the 27B and back behind one request each; the desk no longer waits on a backend nobody warms |
| K4 | kvasir | **A remote server as a backend.** Kind "model server": URL and key, models and specs from its `/v1/models`, ticked models admitted one by one as today | An install's Kvasir lists the model server's models with specs and admits the 27B |
| D1 | nils-desk | **Add a model server.** Paste URL and key, see every model with its specs and whether it is loaded, tick, remove, map stations; one card per server | vitest on the dialog and cards; a headless walk against a throwaway Kvasir |
| E1 | nils | **Setup flags.** `nils setup --model-server URL --model-key-file F`, on install and update | The install matrix gains a case; a scratch `HOME` for every test |
| K5 | kvasir | **The server's own page.** Client keys, the default model, the loaded model and a swap | Behind the group's sign-in; not on the public route |
| M | the model server | **The move.** Kvasir beside modelgate, watching the containers without starting them; then the cutover, adopting the loaded container; the operators' procedure rewritten, with a rollback to modelgate | `smoke.sh` against the public URL; Droid unchanged; a station question from the group's install answered |

K6, the batch door for System 1, is a later slice (R9).

## Settled

- It ships as a Kvasir release (1.0.0-alpha.8) with the desk and engine releases that carry D1 and E1. The group's install takes all three from Settings > Parts.
- It runs beside waves 45 and 46; it shares nothing with them but the desk's Kvasir page.

## What the wave found

- **A review found a hole before merge.** A client key could read the model lifecycle door on the public listener, because a wildcard among the public doors also matched it. Now:
  - the public doors are exact paths;
  - the lifecycle is never public, whatever the configuration lists;
  - a test reads every door from the router and proves a client key opens none it should not.
- **Also fixed before merge:**
  - an error after early headers was lost on a stream;
  - a key reference could name any sealed credential;
  - `/health` showed Docker's error text to anyone;
  - a model server's list had no size limits.
- **Key names.** A key for a model server is staged as `server-staging:<name>` and moved onto the backend when it is admitted. Replacing it later goes under the backend's own id. The desk and `nils setup` both follow that rule.
- **The assistant** reads the new `/v1/pi` base. Kvasir keeps the old path for one release.

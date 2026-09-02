# 01 — Architecture: one core, optional apps

## The map

```
                          ┌─────────────────────────────────────────────┐
   optional apps          │                nils (engine)                │
                          │                                             │
 ┌─────────────┐  AST     │  core (Rust): walk · parse · digest ·       │
 │ nils-query  │─────────▶│  fingerprint · classify · anonymize · bids  │
 └─────────────┘          │                                             │
 ┌─────────────┐  API     │  registry: SQLite+DuckDB (standalone)       │
 │ nils-segment│─────────▶│            Postgres (server)                │
 └─────────────┘          │                                             │
 ┌─────────────┐  MCP     │  server (thin): jobs · contracts · UI       │
 │ nils-agent  │─────────▶│  contracts: API · query AST · review items  │
 └─────────────┘          │             · MCP · events                  │
 ┌─────────────┐          └─────────────────────────────────────────────┘
 │ future apps │──────────────────▲                     ▲ API (reader role)
 └─────────────┘                  │ pipelines (BIDS-Apps containers, D9)
                                  │ ML sidecars (body-part, future models)
                                  │                     │
                                  │        ┌────────────┴────────────┐  signed
                                  │        │ nils node (optional,    │◀───────▶ peers
                                  │        │ same binary, own token) │  requests
                                  │        └─────────────────────────┘  (14, D25)
        platform (not ours): Authentik (OIDC) · reverse proxy · container runtime
                             · WireGuard mesh or blind relay between nodes (14)
```

The node daemon is drawn on the engine's side of the line because it ships in the
binary, but it obeys the app rules: it reaches its own engine only through the
public contracts, under a reader role, and the engine has no code path that knows
it exists ([14](14-federation.md) §3.2, D25).

## The dependency rules (D1)

These are the constitution of the suite. Each is testable, and the build order in
[11-order.md](11-order.md) keeps them true from day one:

1. **The engine imports nothing from any app** — not code, not schemas, not compose
   fragments. It can be built, tested and released from its repo alone.
2. **Apps consume only the engine's public contracts** ([05-contracts.md](05-contracts.md)):
   the HTTP API, the query AST, review items, MCP, events. Never the database
   directly, never internal modules. (v0's read-only DB roles were the honest
   version of a shortcut; v1 removes the need for the shortcut.)
3. **Apps do not talk to each other.** If two apps need the same thing, that thing
   is an engine contract. (v0 already proved this doctrine — "data, not code" — the
   difference is that v1's engine offers rich enough doors that nobody is tempted
   around them.)
4. **Every app has an absence story written down.** What does the user see when the
   app is not deployed? The answer must be "nothing", or a static hint at most.
5. **Every judging step exposes its knobs** (C37, [15](15-ratification.md) §8):
   keywords, rules, thresholds and the identity rule are digest-scoped data, served
   with the step's diagnostics through the affordance API, so the engine runs the
   step alone and an agent, when present, tunes it and runs it again through review
   items. The engine still discovers nothing; it is discoverable to the last knob.

## Optionality matrix

| You deploy | You get | You do not get, and nothing breaks |
|---|---|---|
| `nils` binary alone | full pipeline via CLI, embedded registry, local review via CLI | web UI, multi-user, agents |
| + engine server | web UI, jobs, multi-user registry, review queues, contracts live | selections notebook, segmentation, agents |
| + nils-query | the notebook, saved selections, send-to | — |
| + nils-segment | annotation works, adjudication | — |
| + nils-agent | conversational access, agent-assisted review under policy | — |
| + any MCP client (Claude, etc.) | the same tools the agent uses, ad hoc | — |
| + `nils node` and at least one trusted peer | federated counts and aggregates, compute-to-data runs at peers, per-node availability in the catalog (14, D25) | individual-level data from anywhere else; nothing changes for local users |

## How apps find the engine

One configured base URL per app plus capability discovery: the engine's
`GET /api/capabilities` names its version, the contract versions it speaks, the
enabled auth mode, and the optional features present (ML sidecars, pipelines
runtime). Since C26 ([14](14-federation.md)) it also names the loaded pack
versions, the **registry epoch** (a counter advanced by every ingest batch and
classification run, so any result can say "as of"), and the federation endpoint
when a node daemon is configured, so a notebook shows a scope chip only where
there is a federation to scope to. Apps adapt to what is present; there is no
suite-wide manifest resolver.
What remains of v0's `nilsctl` is per-app: each repo ships its own compose file and
its own `make up`. Suite-level orchestration is a deployment concern (a compose file
in the deployment's own repo), not a product.

## Deployment shapes

- **Laptop**: the binary, a directory, done. Auth `off`.
- **Single server** (the 8-core/64 GB case): engine server + Postgres, auth `token`
  or OIDC, apps added as needed behind one reverse proxy with forward-auth.
- **Our deployment**: the above with Authentik, Traefik, and GPU sidecars — but
  nothing in the products knows our names.
- **A node in a federation** ([14](14-federation.md), D25): any of the server
  shapes plus `nils node serve` and a trust line per peer. The node is usually
  outbound-only behind a hospital firewall, so it joins a WireGuard mesh or talks
  through a blind relay; the server itself does not change.
- **A node in front of a cluster**: the same, with the runner's SLURM and
  Apptainer executor pointed at the site's scheduler (09, C31). Amsterdam's shape,
  in one of its two readings.

## Versioning

Each product versions independently (its own semver, changelog, releases). What is
shared is *contract* versions: the AST schema, the review-item schema, the MCP tool
surface. The engine declares which contract versions it serves; apps declare which
they need. There is no suite version. Between nodes the same rule holds one level
up: a request declares the contract and pack versions it was written for, and a
peer that cannot honour them refuses with a diff (14, D26).

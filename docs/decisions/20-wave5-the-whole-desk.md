# 20 — Wave 5, the whole desk: the rulings, the two studies, the design and the wave

Written 2026-09-10 from the talk that followed the close of Wave 4c: the Metabase
study of the same day (`studies/2026-09-10-metabase-ui-study/`), the proposal
that answered it (`studies/2026-09-10-desk-direction/`), and Nima's five answers
and two additions. Status: **ratified 2026-09-10** (Nima: "if this is ok you can
start writing the wave 5", after the five answers and the two additions below).
The specification written from it is `docs/specs/wave5-the-whole-desk.md` in the
public repository; this document is the record it cites, and the only place the
group's own hosts, pools, paths and domains are named.

Ids continue 19: decisions D53 onward, amendments C49 onward, rulings R12 to R14
(continuing 17 §1 and 19 §1), questions Q21 onward.

## 1. Nima's rulings, 2026-09-10 (R12 to R14)

| | Ruling | What it changes |
|---|---|---|
| R12 | The desk is a workbench designed toward one scene (§4), not a directory of doors. "Make desk something extraordinary and take it to the next level." | The shell is rebuilt around Home, objects with pages and one conversation rail; Operations is retired. |
| R13 | The five answers. (1) The ladder, yes; the identity provider decides what a person may do, the grant is the person's consent. (2) The supervisor: "do it instead of showing how to, but showing is also good to add as a guide." (3) The viewer: "we absolutely need to benchmark and find the best; we handle very big CT scans; it should be smooth; we have PCCT data in source; in the right order." (4) The sections liked; database management and places in Settings, with every path given a role, on any machine. (5) Teaching: yes. | D68, D76, D78, D74, D75, D72. |
| R14 | The look follows the website's design DNA and the mark of the family (Bifrost's, with the naudiz rune); the group's deployment goes on the website now so the wave is seen as it is built; and "nils is an application everyone should be able to use, not something specific to our setup or website: if I install nils on my laptop I should see the same thing and I shouldn't need an Asgard-like system." | The theme as a file (D53), the mark (D54), the site rule and the laptop rule (spec §4), and every slice's gate carrying the laptop test. |

## 2. What the Metabase reading settled

`studies/2026-09-10-metabase-ui-study/findings.md`: nine surfaces of the checkout
of the Metabase checkout read by nine parallel readers, 81 findings, three
verification lenses, one critic, 80 survived. Applied in this wave without a
second argument (spec §6.6): the stale veil with server-side refusal; blocked
controls that keep their reason; truncation as one word; the row count as a link
to `out`; `inert` veils; a shortcut registry; print and the methods paragraph; the
clipboard rule. Refused: values shipped to the model, an agent that navigates and
saves, drag-reordered clauses and hypothetical clauses, row detail with chevrons,
formatted export and per-column scale, the growth machinery. The critic's gaps
that became decisions: clipboard egress (spec §6.6), timezone in the declaration
(D64), the hash as a cache key (spec §12.8), audit read where it is written
(D77), undo against erasure (Q26, open), concurrency on a document (Q27, open).

## 3. What the previous prototype taught

Read from the 0.5.0 checkout and its documentation. The hub
(`features/dashboard`): counts of cohorts, running stages and completed cohorts,
and a table of cohorts by stage; a person saw what NILS held and walked in. The
cohort pipeline: anonymisation, extraction, sorting, BIDS export as stages with
status, the thing v1 renders as a batch's stage strip on the engine's own verbs
(digest, classify, review, release). Quality control: flags by axis and reason,
a priority score, a draft pattern, and a viewer on cornerstone3D
(`@cornerstonejs/core`, `dicom-image-loader`, `tools`). The agent (the separate
nils-agent, `08 §1`): per-user model access, skills, sandboxes, a
self-improvement loop, on a fork nobody could keep. What carries over is the
hub's honesty, the stage strip, the viewer beside the judgement, and the agent's
reach, each rebuilt on the engine's jobs and doors.

## 4. The scene

The Tuesday of `studies/2026-09-10-desk-direction/the-whole-desk.html`, kept here
as the wave's acceptance story (spec bar 2): a person arrives and Home says what
needs them; a batch is opened and its quarantine handled from Data; twelve items
are judged in Review with the evidence and, later, the viewer beside them, one
overlay adopted after its rehearsal; a question is built from nothing by clicking
and by talking, every step with its counts, the conversation kept; the release is
described in full and not made; the assistant is told the night's work and
restates it as three jobs and one proposal; a month later the group's
corrections have trained the local model behind two gates.

## 5. The design DNA (D53, D54)

From the group's site stylesheet, the values the desk's default theme
carries, under the product's own token names:

| The site | The desk's token |
|---|---|
| `--plum #4F0433`, `--plum-hover #7A1F5A`, `--plum-tint #EFE3EA` | `--n-brand`, `--n-brand-hover`, `--n-brand-soft` |
| `--ink #1C1A1F`, `--ink-soft #3E3944`, `--muted #6B6470` | `--n-text`, `--n-text-dim`, `--n-text-faint` |
| `--ground #F6F5F2`, `--panel #ECE9E4`, `--surface #FFFFFF`, `--rule #E4E1DC` | `--n-bg-page`, `--n-bg-sunken`, `--n-bg-raised`, `--n-line` |
| `--ok #1F6E43 / #E1F0E7`, `--warn #B9621C / #FBEEDF`, `--bad #A4232B / #F8E1E2` | the `ok`, `caution`, `blocked` intents |
| (none) | the `gated` intent, the product's own, a teal that is neither |
| IBM Plex Serif, IBM Plex Sans, IBM Plex Mono | the three faces, served from the desk's own origin |

The dark table is derived once and kept as a second full table, tested for the
same key set. The desk today uses `#4b2142` and `system-ui`; B1 replaces both.

The mark: the family's square ground (`#4F0433`) and cream stroke (`#F6F5F2`),
as Bifrost's `bifrost-mark.svg` (a 32 by 32 plum square with one cream arc),
carrying the rune naudiz. Nima wrote ᛅ; in Elder and Younger Futhark naudiz is ᚾ
(a stave with one descending stroke) and ᛅ is ár. Q21 asks which drawing is
meant; the desk reads `web/public/brand/nils-mark.svg` and nothing else.

## 6. The group's deployment (spec §4.2, slice W0)

The host, the guest, the mounts, the identity provider's application and its group
bindings, the portal entry, the deploy path and the places of the group's own
deployment are not part of this copy; they stay in the private record, as the
specification's §4.2 says. The roles of the places and the rules the engine checks
are the specification's §10.2 and D74; the group's deployment is one instance of
them and not the design.

## 7. The viewer study (S1)

To be written when it runs. The corpus is the group's own largest scans, on the
group's own machines, over the mount the group uses and from local scratch; where
they live is not part of this copy. The shape under test: a viewing pyramid at
digest, HTJ2K tiles of the current slab, GPU decode. The candidates: cornerstone3D
(v0's, WebGL and WebGPU, web-worker decoding, HTJ2K progressive loading), niivue,
vtk.js, a server-side render of the slab. The measures: time to first image,
scroll latency through the whole stack, memory at rest and at peak, MPR and
window-level at sixty frames, the precompute's cost. Nothing leaves the deployment.

## 8. Decisions (D53 to D78)

| | Decision |
|---|---|
| D53 | The theme is one file of tokens and the mark one file; a deployment replaces the look by replacing them; a lint refuses a literal colour elsewhere. |
| D54 | NILS has a mark in the family's shape carrying naudiz; it is the favicon, the shell's corner and the portal's icon. |
| D55 | Home: four bands, what the registry holds, what needs you, what is running, what changed; never a row of a person. |
| D56 | The sections: Home, Ask, Data, Review, Release, Pipelines, Assistant, Settings, plus apps; Operations retired. |
| D57 | Every registry noun has a page with one timeline and a stable address. |
| D58 | The rail: the assistant's conversation on every page, aware of the object, typed context. |
| D59 | One wait component re-timed for a twenty-second turn; one failure component with five engine-chosen outcomes; every non-success ending named with one next move. |
| D60 | A question starts from anything or nothing; the start-from resolver is a door. |
| D61 | Every set card carries its live funnel by clause group; rows only under disclosure, never stale. |
| D62 | One condition picker across every set; the move row; a sentence returns a diff on the document; both paths make one version. |
| D63 | A conversation is keyed by the document's lineage; runs, releases and promotions interleave with edits; no revert-in-place. |
| D64 | The declaration gains timezone and week start, the registry's, covered by the core hash. |
| D65 | Data: sources bound to `source` places, batches as stage strips, jobs queued from the page. |
| D66 | Review: items by cost if wrong, evidence beside the decision, closure panels fed by the dependency door, bulk decisions with one audit row each. |
| D67 | Release: the complete description of what leaves, custody beneath it, places above it. |
| D68 | The ladder: read; run what can be undone under a standing grant per person per verb; propose what cannot. The identity provider decides what a person may do; the grant is consent, stored in the assistant, revocable, visible to an admin. |
| D69 | Typed context contributors; the type is the allowlist; a test asserts no row payload. |
| D70 | The `operator` station plans a chain of jobs by rung; a scheduler fires a confirmed plan on an engine event or a time. |
| D71 | The inbox: what the assistant and the engine did for a person; a completion never leaves the desk in this wave. |
| D72 | Teaching: the loop visible, promotion refused until the bench and the admission suite are green. |
| D73 | Parts: every part with version, contracts, health, update; settings generated from capabilities and custody. |
| D74 | Places: named locations with a role and guarantees the engine checks; every path bound by role; the rules at the doors; a laptop's places are directories with declared guarantees. |
| D75 | Database in Settings: the place and the rule, size, backups, a restore rehearsal. |
| D76 | `nils supervise` as a subcommand: reports, watches a signed channel, verifies, applies, restarts; the update button with a closure panel and the hand command beside it; a guide. |
| D77 | Audit is read in Settings, as its own disclosure surface with a retention and a row per read. |
| D78 | The viewer is chosen by the study of §7 after the workbench and before Review. |

## 9. Amendments (C49, C50)

- **C49** (D11, 19 D44). "A station proposes and never decides" gains a rung: a
  station may queue a job that is idempotent and whose result is a new object
  beside the old, under a standing grant the person gave for that verb. The
  guarantee that no station applies a judgement of its own is unchanged, and the
  CI grep of 4c §9.16 now also refuses a rung-three door in any manifest. An
  optional sixth entitlement `assist-run` may enter `contracts/suite/v1` if the
  group wants group-level policy over rung two (Q23).
- **C50** (00, "one binary"). The binary gains `supervise`, a service that may
  replace the parts on its host; it obeys D1: it reaches the parts only through
  their contracts and the release channel, and the engine has no code path that
  knows it exists.

## 10. Questions carried into the wave (Q21 to Q27)

- **Q21** The rune on the mark: naudiz ᚾ as drawn, or the ᛅ of Nima's message. Settled by Nima against the drawing in B1.
- **Q22** When the deployment moves from the synthetic registry to the group's data: inside the wave, or after it closes.
- **Q23** Whether `assist-run` is wanted from the start.
- **Q24** The viewer, by S1's report.
- **Q25** Whether a completion may ever reach the group's chat channel, and under which words (the channel has no clearance for content).
- **Q26** Undo against erasure: a five-second way back for a withdrawn handle, and an irreversible erasure that reaches the linkage store and every released handle. Named by the Metabase critic; not designed here.
- **Q27** Two people on one document: a version token on save and apply, and a "this moved under you" state. Named by the critic; not designed here.

## 11. Order

Twenty slices, spec §15: A0 W0 B1 A1 A2 B2 A3 B3 D1 B4 A4 A5 B5 E1 D2 S1 A6 B6 C1 D3 W1. W0 is the first thing built after the record so that everything after it is seen on the portal the day it merges.

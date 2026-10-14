# 41 · What the protocols said

Opened 2026-09-24. The first wave of [40](40-the-road-through-wave-6.md), and so of master-plan Wave 6. **Merged 2026-09-24** as kineuro/nils#212; released with the alpha that closes it.

## The ask

Record 40, step 1: fix the rules that five scanner protocols proved wrong, make the engine record every rule's vote so a label model can read them, and close the small defects that have waited since the assistant's wave.

What the protocols said (record 39, "What the protocols measured"; `studies/2026-09-23-protocol-check/`): of 17,264 v0-clean stacks in protocol sessions, 3,063 carry a confirmed wrong label. Those were v0's labels. The first thing this wave does is measure v1's, because the pack has moved since v0 and a fix must be aimed at what v1 does, not at what v0 did.

## Slices

| | What | Proven by |
|---|---|---|
| S0 | **v1 on the protocol sessions.** alpha.37 digests and classifies the protocol sessions' files on the production host into a scratch registry: read only, niced, nothing leaves the production host but counts. Its labels are compared with the expected-label table exactly as v0's were, per axis and per error pattern. The harness stays, so each later slice and wave re-runs it | A count table in the same shape as v0's, kept in `studies/2026-09-24-v1-on-the-protocols/` |
| S1 | **The rules the protocols proved wrong**, as far as S0 shows v1 still has them. RESOLVE only from a readout-segmented sequence name (`re_b`), never from segmented k-space plus EPI, which Siemens writes on every EPI. FlowComp only from the header's scan options, not from words in a name. MEDIC's multi-echo GRE is not GRASE. A series' own body-part evidence beats the exam's `BodyPartExamined`. Anything else S0 finds is either fixed here or written down with its count | A synthetic case per fix in the pack's corpus, and S0 re-run with no known error pattern left |
| S2 | **Every rule's vote.** Classify records, for each stack and axis, every rule whose clause held and what it said, not only the first that fired; the winner is unchanged. `nils classify votes` writes the vote matrix (stack, axis, rule, value, tier) for the label model, counts and ids only | Verdicts identical before and after on the corpus; the matrix covers every decided axis |
| S3 | **The pack knows its own shape.** The loader reports values no rule and no valid combination reaches, and marks clauses that only restate another axis as implications of the schema rather than evidence | A report on `mri@` naming each unreachable value; a test pack with a planted one |
| S4 | **Small things that waited.** The ask door's four wrong refusals (#98 YAML reads `n` as a boolean; #99 a share measure without its denominator passes validation and fails with a 500; #100 the catalog lists kinds the caller may not use; #101 an inline list of literals read as a clause). A bounded sample of the text an axis matched against, for unresolved stacks (#94). `float_roundtrip` in the sidecar path. An ask fixture with a ruled-out stack (record 35) | Each issue's own reproduction as a test; the ask gate |

## Settled

- One wave, one pull request per repository, tried on the development laptop through the development channel, the corpus re-run, no tag.
- S0 runs first and alone decides S1's list; S2 to S4 run beside it.
- The protocol stacks are development data (record 40 R3). Nothing in this wave touches the sealed samples of Phase 0.
- The pack's version moves (`mri@0.3.0`), because S1 changes answers.

## What the wave found

**v1 inherited v0's errors, and did not know it.** S0 ran alpha.37 over the 805 protocol studies on the archive's host: 5,059,011 files, 23,136 stacks, compared with the expected-label table checked by hand against the printouts (`studies/2026-09-24-phase-0/check.md`). v1 made the same mistakes as v0, and **99.2 % of its errors sat on stacks that raised no review item**. A rule that is wrong is confidently wrong, which is record 39's argument in one number.

| | `mri@0.2.0` (alpha.37) | `mri@0.3.0` (this wave) |
|---|---:|---:|
| single-shot diffusion called RESOLVE | 2,916 | 0 |
| BOLD called RESOLVE | 462 | 0 |
| "0 flow comp" read as FlowComp (and its reformats) | 87 (+125) | 0 |
| MoCo copies and ASL subtractions as acquisitions | 480 | 0 |
| SWI reformats as construct SWI | 375 | 0 |
| brain series as spine, spinal series as brain | 36 (+6), 10 | 30 (+6), 6 |
| SWI-EPI with SWI off called SWIRecon | 514 | 514 |
| **stacks with at least one wrong label** | **4,269 (24.0 %)** | **550 (3.1 %)** |

What is left needs evidence one stack's header does not carry. The body part of 42 stacks whose series say nothing and whose exam was registered as the other part needs the image (wave 43's detector). The SWI-EPI provenance needs a session-level view, because with SWI on or off Siemens writes the same image type on magnitude and phase.

**On the corpus** (`studies/2026-09-24-wave41-corpus/`), 55 stacks left RESOLVE for diffusion, DSC perfusion, ASL and BOLD; 14 stacks whose own words say neck are now neck where their exam's registration had said brain or spine; review items 70 to 68. Nothing else moved.

**Found on the way:**
- **Canon's FASE.** v0's normalizer rewrote `fase` to `resolve`, so Canon's single-shot fast spin echo would be called RESOLVE. No series in the archive carries the word. The rewrite and both keywords are gone, because `fase` is also the Danish, Norwegian and Dutch word for phase; a site with Canon scanners adds it back to HASTE through its overlay.
- **The shape.** `nils pack shape` counts 67 clauses that restate another axis, not the 56 record 39 carried from the research; 67 is measured on `mri@0.3.0` and stands. Two values are unreachable: `construct.FatFraction` (kept for the vocabulary; its dead tests are gone) and `base.Unknown` (v0's internal marker).
- **The ask door.** #100 did not reproduce: the catalog already hides what a caller's detail does not open. It keeps a regression test.

- **Review of the pull request** found nothing that had to be fixed and five things that should be, all fixed before merging. A boolean ask option written as `no` or `yes` took its default in silence under the YAML 1.2 booleans; it is now refused. A combined head-neck-spine coil would have made a brain series spine; the coil rule now needs a spine coil and nothing else. GE writes flow compensation as `FC_SLICE_AX_GEMS` and `FC_FREQ_AX_GEMS`, which the header rule now reads. The signals door read every stack's owner on every request, and it ignored an adopted overlay. The harness still gives 550.
- **Left as a note:** without the segmented-k-space combination, a multi-shot EPI from a vendor whose sequence name does not say so (a Philips multishot diffusion, for one) is called single-shot DWI-EPI. No such stack is in the protocol sessions; the Phase 0 archive run will say how many there are.

Pull request kineuro/nils#212.

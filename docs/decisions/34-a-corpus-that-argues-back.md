# 34 · A corpus that argues back

Opened 2026-09-19. **Closed 2026-09-23**: the corpus is built (1,003 stacks on the development laptop) and every wave since runs against it. Its bar, "beta, if it survives", moved to [40](40-the-road-through-wave-6.md) R6.

## The ask

> Nima, 2026-09-19: "check the existing real metadata_db on the production server and find at least one session with cases that can cover all stacks variant like branches, techniques/modifier/struct/all axes, low confidence, conflict and missing classifications, close date, brain/spine sessions and so... the least number of subject session that can cover all is very good. then building the dataset and moving it to the development laptop and then we have something to test all part and see it in action. then we can call it beta."

## Why

Everything the engine has been tried against was made for it. `nils synth` is a yardstick, `awkward` is a set of repairs somebody wrote down, `reference` is one person with every release case and hand-verified answers. All three agree with the code by construction. The classifier, the session window, the release and the desk have never met an archive that argues with them, and until they have, a version number says nothing about whether this works.

The cheapest honest test is not a big corpus. It is the smallest one that still contains every case the code claims to handle.

## What it has to cover

- **Every branch of the classification** and, across the set, every technique, modifier and structure, on each axis.
- **The awkward answers**: a stack the classifier is unsure of, one where the evidence conflicts, one it cannot place at all.
- **The session window doing its work**: studies close enough in time to be one session, and a brain and a spine on different days that belong together.
- **Families whose point is inside the session**, where a single stack proves nothing: MP2RAGE, SyMRI, SWI, multi-echo. These sessions come whole.
- **The paths that only bad data reaches**: made-up patient names, so pseudonymisation is watched rather than assumed, and studies with no date.

Whole sessions only where the differentiation is inside them; elsewhere the stacks that carry the case. Fewest subjects and sessions that cover the list.

## How it is chosen

1. **Survey.** The production host's v0 metadata database, read only, to find candidates for each case above. What comes back is counts and characteristics: how many subjects have a session with both a brain and a spine two days apart, how many have a four-echo series, and so on. No names, no codes, no series names.
2. **The list.** A proposal with one line per candidate, what it covers, and how many files it brings. Nima picks.
3. **Build.** The chosen studies copied on the production host, the made-up names and missing dates applied there, and the result moved to the development laptop, into the source place its install reads.
4. **Watch it work.** Digest, classify, sessions, an ask, a release, and the desk, each recorded as what it did rather than whether it errored: what it classified, what it flagged, what it could not place, what the session window merged, what the release wrote.

## Where it lives

The development laptop's own disk. Nima, 2026-09-19: the laptop is used for work and the corpus is never moved off it. It is clinical data derived from the production host's archive, so what may be said about it outside the machine is counts, rates and failure classes, never a name, a code or a series name.

## What this is not

Not a measurement corpus: the two development corpora stay what they are. Not a replacement for `nils synth`, which is the ask's yardstick and has to stay generated and stable. This one exists to be difficult.

## What follows it

Beta, if it survives. That is the bar Nima set, and it is a better one than a count of alphas.

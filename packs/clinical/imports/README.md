<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# Import mappings

`nils clinical import --mapping FILE --file CSV` reads a CSV under one of
these mappings (`docs/specs/wave4a-engine-completes.md`, section 7.2). A
preview shows what would change and writes nothing; `--apply` writes under
your name; a re-run changes nothing, because every row is idempotent on the
key its target names.

A mapping says:

- `target`: `event`, `subject`, `cohort`, `cohort_member`, `subject_disease`
  or `subject_disease_type`;
- `subject`: which column names the subject, and how: `by: code` (the
  registry's own code) or `by: identifier` with `id_type` (an identifier the
  linkage store holds, the hospital's or the study's), which is the way that
  works without the key that made the codes;
- the reference the target needs, as a constant or `{column: ...}`:
  `observation_type` for an event, `cohort` for a membership, `disease` (and
  `disease_type`) for a diagnosis;
- `columns`: each field of the target from a `column`, a constant `value`,
  or both (the value fills an empty cell), with a `parser` (`text`, `int`,
  `float`, `bool`, `date`, `time`) and, for a date or a time, its `format`;
- `key`: the fields, beside the subject and the reference, that make a row
  the same row as one the registry holds (an event's default is its date);
- `on_existing`: `skip` (the default, and what makes a re-run change
  nothing), `update` in place, or `supersede`, which writes a new row and
  marks the old one superseded by it;
- `source`: a label recorded on every event written.

A date is read under its declared format and never guessed: `01/02/2022`
is the first of February under `%d/%m/%Y` and the second of January under
`%m/%d/%Y`, and a birth date read the wrong way round is an age that is wrong
for a lifetime.

| file | target | what it shows |
|---|---|---|
| `edss.yml` | `event` | a numeric scale, one kind for the whole file, the subject by an identifier |
| `treatment.yml` | `event` | a text value, the kind read per row |
| `demographics.yml` | `subject` | birth date and sex; a disagreement with the registry is a review item and the registry's value stands |
| `cohort.yml`, `membership.yml` | `cohort`, `cohort_member` | a cohort and who is in it |
| `diagnosis.yml`, `course.yml` | `subject_disease`, `subject_disease_type` | a diagnosis with its onset and diagnosis dates, which become events, and its course |

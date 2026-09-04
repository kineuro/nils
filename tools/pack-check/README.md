# Checking a pack against v0

A pack is vocabulary and grammar as data, and the only way to know it carries
v0's knowledge is to run both over the same stacks and diff the rows.

    # v1: the pack, over a CSV of stacks, with no registry
    cargo run --release -p nils-pack --example classify_csv -- \
        --pack packs/mri --csv stacks.csv --axis technique > v1.tsv

    # v0: its own detector, over the same CSV
    python3 tools/pack-check/referee.py \
        --v0 /path/to/v0/backend/src --csv stacks.csv --axis technique > v0.tsv

    python3 tools/pack-check/compare.py v1.tsv v0.tsv

`stacks.csv` is a read-only export of v0's `stack_fingerprint`
(`spikes/pack/export.sh`), which is what its classifier reads. The corpus never
leaves the private host, and the comparison publishes counts and class names,
never a description, a path or an identifier.

v0 is private and is never copied here: the referee imports it from wherever it
is on the host.

## Sorting the cohort again, with v0 as it is today

`series_classification_cache` is what v0 said when it last sorted each cohort,
and v0's code has moved since. A gate run against that table reports v0
disagreeing with itself as though v1 had caused it. `resort.py` runs v0's own
classification over the same stacks with the code as it stands, plus the two
step 4 phases that change an axis, and writes the same columns:

    python3 tools/pack-check/resort.py \
        --v0 /path/to/v0/backend/src --csv stacks.csv \
        --overrides overrides.json --cohorts stored-cache.csv \
        --reference rules --out resorted.csv

    v0-compare extract --export EXPORT --classification resorted.csv --out v0.duckdb

`--overrides` is v0's `cohort_classification_overrides`, exported read-only as
a JSON array of `{cohort, axis, bucket_path, added, removed}`. It matters: a
cohort can add keywords to one bucket of one detector, that table lives in v0's
application database rather than in its code, and without it the same stack
classifies differently. `--cohorts` says which cohort each stack came from, and
any CSV with `series_stack_id` and `dicom_origin_cohort` will do.

`--reference` chooses which stacks the physics vote may learn from, which is
the one thing v0 leaves to the history of its database rather than to its code.
`vote_reference.py` measures the candidates: `holdout` hides a share of the
stacks the rules decided and scores the vote against them, and `order` sorts
the same stacks in two different orders and counts the answers that changed.

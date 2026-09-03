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

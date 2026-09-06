<!-- SPDX-License-Identifier: AGPL-3.0-only -->

# The Wave 4a gate

`docs/specs/wave4a-engine-completes.md`, section 12: eleven bars, on the
reference corpus and on a real cohort re-digested from its archive with its
clinical files imported, because there is no migration.

Three scripts, three places:

| script | where it runs | what it holds |
|---|---|---|
| `../release-check/gate.sh` | CI, on every push, on the synthetic reference corpus | the release bars of Wave 3, the private elements (8b), the clinical join (10) and a second process through the door (9b) |
| `cohort.sh` with `check.py` | the private host that holds a cohort's archive and its clinical files | bars 1, 2, 6, 7, 8, 9, 10 and 11 on the real thing |
| `budget.sh` | the baseline host, from the archive over NFS | bar 11: the digest's rate and memory, the second pass, the release's re-run |

Nothing here names a path or a value: every location is an argument, and
what the scripts print is counts and timings.

```sh
tools/wave4a-gate/cohort.sh --nils BIN --source RAW --csv CSVS --maps MAPS \
    --packs PACKS --work WORK [--v0-counts FILE] [--workers N]
tools/wave4a-gate/budget.sh --nils BIN --source RAW --packs PACKS --work WORK [--workers N]
```

`CSVS` holds `membership.csv`, `demographics.csv`, `diseases.csv` and
`events.csv` as the cohort's own export writes them, keyed by the cohort's
identifier; `MAPS` holds the mappings for the one importer (`cohort.yml` with
its `cohort.csv`, `membership.yml`, `demographics.yml`, `diseases.yml`,
`events.yml`; `packs/clinical/imports/` has the shapes). `--v0-counts` is a
JSON file with `subjects`, `studies` and `series` as the earlier system
counted them, and an optional `causes` map naming why a count may differ;
without it the registry stands on its own.

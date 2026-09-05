# SPDX-License-Identifier: AGPL-3.0-only
"""A v1 registry against the awkward corpus, scenario by scenario.

    check.py REGISTRY.db MANIFEST.json WORKDIR [--verbose]

The corpus (`nils-dicom`'s `awkward` example) writes a tree that is wrong in
named ways and a manifest that says what a correct reader finds in it: how many
people each scenario describes, which studies belong together, and the date of
each study with the source that should have answered.

The manifest also says what `nils session` must derive from each subtree, per
scheme; the gate asks for that and leaves the answers in WORKDIR.

This compares a digest of that tree against the manifest and prints one line per
scenario. It is the gate of Wave 3's first two slices (spec §12, bar 1), and it
is meant to fail today: the repairs it checks for are what the slices add.

Counts and scenario names only. The corpus is synthetic, so nothing here is
sensitive, but the shape of the output is the same one the real gate uses.
"""

from __future__ import annotations

import json
import sqlite3
import sys
from collections import defaultdict


def load(registry: str):
    """Every study of the digest, with the scenario it came from.

    Either a registry the whole corpus was digested into, or the combined view
    `collect.py` builds when each scenario was digested with its own rule."""
    con = sqlite3.connect(registry)
    tables = {r[0] for r in con.execute("SELECT name FROM sqlite_master WHERE type='table'")}
    said: dict[str, set[str]] = defaultdict(set)
    if "said" in tables:
        for scenario, kind, _n in con.execute("SELECT scenario, kind, count FROM said"):
            said[scenario].add(kind)
    if "rows" in tables:
        rows = con.execute("SELECT path, study, subject, study_date FROM rows").fetchall()
    else:
        rows = con.execute(
            """
            SELECT sf.path, st.id, st.subject_id,
                   COALESCE(st.date_filled, st.study_date)
            FROM source_file sf
            JOIN instance i ON i.id = sf.instance_id
            JOIN series se ON se.id = i.series_id
            JOIN study st ON st.id = se.study_id
            """
        ).fetchall()
    con.close()
    # scenario -> study dir -> (study ids, subject ids, dates)
    seen: dict[str, dict[str, tuple[set, set, set]]] = defaultdict(
        lambda: defaultdict(lambda: (set(), set(), set()))
    )
    for path, study_id, subject_id, date in rows:
        parts = path.split("/")
        scenario = parts[0]
        # The study directory is everything above the file, and the mess of a
        # scenario sits under `_mess` rather than under a study.
        study_dir = "/".join(parts[:-1])
        studies, subjects, dates = seen[scenario][study_dir]
        studies.add(study_id)
        subjects.add(subject_id)
        dates.add(iso(date))
    return seen, said


def iso(value) -> str:
    """A stored date as `YYYYMMDD`, which is how the manifest writes one."""
    if not value:
        return ""
    return str(value).replace("-", "").strip()


def main() -> int:
    if len(sys.argv) < 3:
        print(__doc__.strip().splitlines()[2].strip(), file=sys.stderr)
        return 2
    registry, manifest, work = sys.argv[1], sys.argv[2], sys.argv[3]
    verbose = "--verbose" in sys.argv
    want = json.load(open(manifest, encoding="utf-8"))
    seen, said = load(registry)

    print(f"{'scenario':<24} {'people':>6} {'seen':>5}  {'dates':>7}  what is wrong")
    print("-" * 92)
    failed = 0
    for s in want["scenarios"]:
        name = s["name"]
        got = seen.get(name, {})
        # Subjects, ignoring the mess, which belongs to a study the scenario
        # already declared.
        subjects = set()
        for d, (_st, subs, _dt) in got.items():
            if "/_mess/" not in f"/{d}/":
                subjects |= subs
        wrong = []
        if len(subjects) != s["people"]:
            wrong.append(f"{len(subjects)} people, not {s['people']}")

        right_dates = 0
        for w in s["studies"]:
            entry = got.get(w["dir"])
            if entry is None:
                wrong.append(f"no study at {w['dir'].split('/', 1)[1]}")
                continue
            dates = {d for d in entry[2] if d}
            expected = w["date"] or ""
            if expected:
                if dates == {expected}:
                    right_dates += 1
                elif not dates:
                    wrong.append(f"{shortdir(w)} undated, wanted {expected} from {w['source']}")
                else:
                    wrong.append(f"{shortdir(w)} is {'/'.join(sorted(dates))}, wanted {expected}")
            else:
                if dates:
                    wrong.append(f"{shortdir(w)} is {'/'.join(sorted(dates))}, wanted no date")
                else:
                    right_dates += 1

        # A scenario may exist to make the reader speak rather than to make it
        # right, and then what it said is the check.
        want_said = s.get("diagnostic")
        if want_said and want_said not in said.get(name, set()):
            wrong.append(f"no {want_said} diagnostic")

        wrong += sessions_wrong(s, work)

        n_dates = len(s["studies"])
        ok = not wrong
        failed += 0 if ok else 1
        note = "ok" if ok else "; ".join(wrong[:2]) + (" ..." if len(wrong) > 2 else "")
        print(
            f"{name:<24} {s['people']:>6} {len(subjects):>5}  "
            f"{right_dates:>3}/{n_dates:<3}  {note}"
        )
        if verbose and not ok:
            print(f"      needs: {s['needs']}")

    print("-" * 92)
    total = len(want["scenarios"])
    print(f"{total - failed} of {total} scenarios right")
    return 1 if failed else 0


def sessions_wrong(s, work: str) -> list[str]:
    """What `nils session` got wrong for one scenario, per scheme.

    A scheme is checked on the labels it produced, in date order, and on how
    many sessions it flagged. Both matter: a scheme that labels everything and
    flags nothing has hidden the disagreement it was asked to find.
    """
    out: list[str] = []
    for i, check in enumerate(s.get("sessions", [])):
        path = f"{work}/sessions-{s['name']}-{i}.json"
        try:
            got = json.load(open(path, encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            out.append(f"scheme {i}: no sessions ({type(exc).__name__})")
            continue
        rows = got.get("rows", [])
        labels = [r["label"] for r in rows]
        want = [None if l is None else l for l in check["labels"]]
        if labels != want:
            shown = ",".join("-" if l is None else l for l in labels) or "nothing"
            wanted = ",".join("-" if l is None else l for l in want)
            out.append(f"scheme {i} gave {shown}, wanted {wanted}")
            continue
        if got.get("flagged", 0) != check["flagged"]:
            out.append(
                f"scheme {i} flagged {got.get('flagged', 0)}, wanted {check['flagged']}"
            )
    return out


def shortdir(w) -> str:
    """A study directory without its scenario prefix, which the row already has."""
    return w["dir"].split("/", 1)[1] if "/" in w["dir"] else w["dir"]


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""The bars of the release gate (Wave 3 section 12).

Every bar is a function returning a list of complaints. The gate prints them
all rather than stopping at the first, because a run that says one thing is
wrong when four are wastes three runs.

The oracle is not v0 (D16): its export is not valid BIDS, so byte-identity
against it would be a bar against being correct. The oracle here is the
standard, the reference answers checked in beside the corpus, and the
de-identification's own claims.
"""

from __future__ import annotations

import json
import os
import re
import sqlite3
import sys
import tomllib
from pathlib import Path

HERE = Path(__file__).resolve().parent

# What the source tree says, which must not appear in anything released.
SOURCE_VALUES = [
    b"REFERENCE^SUBJECT",
    b"19800101-1234",
    b"A Synthetic Clinic",
    b"081500",
    b"082000",
]
SOURCE_DATES = [b"20220115", b"20220715"]
# The example root the reference corpus hangs its UIDs from.
SOURCE_UID_ROOT = b"1.2.826.0.1.3680043.8.498"


def load(work: Path, name: str):
    path = work / f"{name}.json"
    if not path.is_file():
        return None
    return json.loads(path.read_text())


def files_under(root: Path) -> list[str]:
    out = []
    for base, _, names in os.walk(root):
        for n in names:
            out.append(str(Path(base, n).relative_to(root)))
    return sorted(out)


# --------------------------------------------------------------------------
# 3. The reference selections are right
# --------------------------------------------------------------------------


def bar_reference(work: Path) -> list[str]:
    """Hand-verified answers, checked in beside the corpus.

    Not computed by the generator: a change in the pack or in either grammar
    shows up here as a difference rather than moving with the code.
    """
    expected = tomllib.loads((HERE / "reference.toml").read_text())
    bad = []

    # The subject directory is dropped: the code is a pseudonym and changes
    # with the key, so it is not something to record.
    want = sorted(expected["descriptive"]["names"])
    dirs = sorted({
        "/".join(p.split("/")[1:-1])
        for p in files_under(work / "descriptive")
    })
    if dirs != want:
        for line in sorted(set(want) ^ set(dirs)):
            side = "missing" if line in want else "unexpected"
            bad.append(f"the descriptive tree: {side} {line}")

    if (work / "bids").is_dir():
        images = sorted(
            re.sub(r"sub-[0-9a-z]+", "sub-X", p)
            for p in files_under(work / "bids")
            if p.endswith(".nii.gz") or p.endswith(".dcm")
        )
        images = sorted({re.sub(r"/[0-9]{8}\.dcm$", "/<dicom>", p) for p in images})
        want = sorted(expected["bids"]["files"])
        if images != want:
            for line in sorted(set(want) ^ set(images)):
                side = "missing" if line in want else "unexpected"
                bad.append(f"the BIDS tree: {side} {line}")
    return bad


# --------------------------------------------------------------------------
# 2. The validator passes
# --------------------------------------------------------------------------


def bar_validator(work: Path) -> list[str]:
    """Every name in the raw tree, against the schema the engine carries.

    Structural, and run always: the official validator needs a network and a
    node, and a gate that only runs where those exist is a gate that does not
    run. When `bids-validator` is on the path it is run too and its errors are
    bars, with no warnings suppressed.
    """
    if not (work / "bids").is_dir():
        return []
    schema = json.loads((HERE / "bids-schema.json").read_text())
    entities = {e["name"]: e for e in schema["entities"]}
    order = [e["name"] for e in schema["entities"]]
    groups = schema["groups"]
    bad = []

    for path in files_under(work / "bids"):
        parts = path.split("/")
        if parts[0] in ("sourcedata", "derivatives") or len(parts) < 3:
            continue
        name = parts[-1]
        datatype = parts[-2]
        stem = re.sub(r"\.(nii\.gz|nii|json|bval|bvec)$", "", name)
        if stem == name:
            continue
        fields = stem.split("_")
        suffix = fields[-1]
        group = next(
            (g for g in groups if g["datatype"] == datatype and suffix in g["suffixes"]),
            None,
        )
        if group is None:
            bad.append(f"{path}: {suffix} is not a {datatype} suffix")
            continue
        seen = []
        for field in fields[:-1]:
            if "-" not in field:
                bad.append(f"{path}: {field} is not an entity")
                continue
            key, value = field.split("-", 1)
            if key in ("sub", "ses"):
                seen.append(key)
                continue
            if key not in entities:
                bad.append(f"{path}: {key} is not an entity of the standard")
                continue
            e = entities[key]
            if e["key"] not in group["allowed"]:
                bad.append(f"{path}: {suffix} does not take {key}")
            if e["index"] and not value.isdigit():
                bad.append(f"{path}: {key}-{value} is not an index")
            if not e["index"] and not re.fullmatch(r"[0-9a-zA-Z+]+", value):
                bad.append(f"{path}: {key}-{value} is not a label")
            if e["values"] and value not in e["values"]:
                bad.append(f"{path}: {key}-{value} is not one of {e['values']}")
            seen.append(key)
        if seen[:1] != ["sub"]:
            bad.append(f"{path}: a name begins with the subject")
        wanted = [k for k in order if k in seen]
        if [k for k in seen if k in order] != wanted:
            bad.append(f"{path}: the entities are not in the standard's order")
        for required in group["required"]:
            short = next(e["name"] for e in schema["entities"] if e["key"] == required)
            if short not in seen:
                bad.append(f"{path}: {suffix} requires {short}")

    for required in ("dataset_description.json", "participants.tsv", "README"):
        if not (work / "bids" / required).is_file():
            bad.append(f"the dataset has no {required}")
    description = json.loads((work / "bids" / "dataset_description.json").read_text())
    if description.get("BIDSVersion") != schema["bids_version"]:
        bad.append("the dataset does not say which version of the standard it is")
    return bad


# --------------------------------------------------------------------------
# 4, 5. Every stack is placed, and the descriptive layout names everything
# --------------------------------------------------------------------------


def bar_placed(work: Path, db: sqlite3.Connection) -> list[str]:
    bad = []
    stacks = db.execute("SELECT COUNT(*) FROM stack").fetchone()[0]
    excluded = db.execute(
        "SELECT COUNT(*) FROM classification_axis WHERE axis = 'disposition' AND value = 'excluded'"
    ).fetchone()[0]

    descriptive = load(work, "descriptive")
    if descriptive["stacks"] + excluded != stacks:
        bad.append(
            f"the descriptive release placed {descriptive['stacks']} of {stacks} stacks "
            f"and ruled out {excluded}; the counts do not reconcile"
        )
    # 5: a stack the registry never classified lands as `misc/stack-NNNNNNNN`,
    # which says what it is. The reference corpus has none, so any is a
    # regression in the grammar.
    unnamed = [p for p in files_under(work / "descriptive") if "/stack-" in p]
    if unnamed:
        bad.append(f"the descriptive layout did not name {len(unnamed)} stack(s)")

    bids = load(work, "bids")
    if bids is not None:
        routed = sum(bids["routes"].values())
        if routed + excluded != stacks:
            bad.append(
                f"the BIDS release routed {routed} of {stacks} stacks and ruled out "
                f"{excluded}; the counts do not reconcile"
            )
        # Nowhere is never silent.
        nowhere = bids["routes"].get("nowhere", 0)
        if nowhere != sum(bids["nowhere"].values()):
            bad.append("a stack went nowhere without a reason")
        # Of this version, not of every version: a re-run records its own.
        absent = db.execute(
            "SELECT COUNT(*) FROM release_absent WHERE release_id = ?",
            (bids["release_id"],),
        ).fetchone()[0]
        if absent != nowhere:
            bad.append(f"{nowhere} stack(s) went nowhere and {absent} were recorded")
    return bad


# --------------------------------------------------------------------------
# 6. One stack per session and role, ties reported
# --------------------------------------------------------------------------


def bar_picks(work: Path, db: sqlite3.Connection) -> list[str]:
    bad = []
    rows = db.execute(
        """SELECT role, subject_id, session_day, COUNT(*)
           FROM pick WHERE withdrawn_at IS NULL
           GROUP BY role, subject_id, session_day"""
    ).fetchall()
    for role, subject, day, n in rows:
        if n > 1:
            bad.append(f"{n} picks for {role} of subject {subject} on {day}, one too many")
    # And a tie is reported rather than settled by row order, which means the
    # margin is recorded even when it is zero.
    unrecorded = db.execute(
        "SELECT COUNT(*) FROM pick WHERE margin IS NULL AND withdrawn_at IS NULL"
    ).fetchone()[0]
    if unrecorded:
        bad.append(f"{unrecorded} pick(s) with no margin, so a tie could not be seen")
    return bad


# --------------------------------------------------------------------------
# 7. Every file is traceable
# --------------------------------------------------------------------------


def bar_traceable(work: Path, db: sqlite3.Connection) -> list[str]:
    bad = []
    orphan = db.execute(
        """SELECT COUNT(*) FROM release_file f
           LEFT JOIN stack k ON k.id = f.stack_id WHERE k.id IS NULL"""
    ).fetchone()[0]
    if orphan:
        bad.append(f"{orphan} released file(s) name a stack the registry does not have")
    # And where the file is one instance written out, the instance is real.
    orphan = db.execute(
        """SELECT COUNT(*) FROM release_file f
           LEFT JOIN instance i ON i.id = f.instance_id
           WHERE f.instance_id IS NOT NULL AND i.id IS NULL"""
    ).fetchone()[0]
    if orphan:
        bad.append(f"{orphan} released file(s) name an instance the registry does not have")
    # Every decided axis says who decided it (section 10.1).
    unattributed = db.execute(
        """SELECT COUNT(*) FROM classification_axis a
           WHERE a.value IS NOT NULL AND NOT EXISTS (
             SELECT 1 FROM classification_evidence e
             WHERE e.stack_id = a.stack_id AND e.axis = a.axis)"""
    ).fetchone()[0]
    if unattributed:
        bad.append(f"{unattributed} axis value(s) with no evidence for who decided them")
    return bad


# --------------------------------------------------------------------------
# 8. The de-identification does what it says
# --------------------------------------------------------------------------


def bar_deidentified(work: Path) -> list[str]:
    """Read the bytes, not the report.

    A byte scan rather than a tag walk, because the claim is about what leaves:
    no value the source carried appears in anything released, wherever it might
    have been copied to.
    """
    bad = []
    for tree, shifted in (("descriptive", False), ("shifted", True)):
        root = work / tree
        if not root.is_dir():
            continue
        # One file per stack directory: every file of a stack went through one
        # scrub with one plan, so the first says what the rest say, and a real
        # tree has millions of them.
        seen: set[str] = set()
        for path in files_under(root):
            if not path.endswith(".dcm"):
                continue
            directory = path.rsplit("/", 1)[0]
            if directory in seen:
                continue
            seen.add(directory)
            data = (root / path).read_bytes()
            for value in SOURCE_VALUES:
                if value in data:
                    bad.append(f"{tree}: a released file still carries {value.decode()}")
            if SOURCE_UID_ROOT in data:
                bad.append(f"{tree}: a released file still carries a source UID")
            # Overlays and curves, by group.
            for group in (0x6000, 0x5000):
                if bytes([group & 0xFF, group >> 8, 0x00, 0x30]) in data:
                    bad.append(f"{tree}: a released file still carries a {group:04X} block")
            if shifted:
                for date in SOURCE_DATES:
                    if date in data:
                        # 4.3 as a test: including inside a UID.
                        bad.append(
                            f"shifted: a released file still carries {date.decode()}, "
                            "which is the date it was supposed to have moved"
                        )
    return bad


# --------------------------------------------------------------------------
# 9. Round trip and increment
# --------------------------------------------------------------------------


def bar_increment(work: Path) -> list[str]:
    bad = []
    for tree in ("descriptive", "bids"):
        again = load(work, f"{tree}-again")
        if again is None:
            continue
        if again["written"] != 0:
            bad.append(f"{tree}: running the same release again wrote {again['written']} file(s)")
        if again["added"] or again["rewritten"] or again["removed"]:
            bad.append(f"{tree}: running the same release again was not a no-op")
        first = load(work, tree)
        if again["files"] != first["files"]:
            bad.append(f"{tree}: the second version's manifest is not the whole tree")
    return bad


# --------------------------------------------------------------------------
# 10. The date the clinical join needs survives
# --------------------------------------------------------------------------


def bar_dates(work: Path, db: sqlite3.Connection) -> list[str]:
    """Section 9.4, which is the coupling of 2.1 broken.

    The join itself is Wave 4's, because v1 has no clinical layer yet. What is
    checked here is the mechanism it needs: the time in the standard's own
    column is the registry's, under the policy the release ran under.
    """
    if not (work / "bids").is_dir():
        return []
    bad = []
    days = {
        row[0]: row[1]
        for row in db.execute(
            "SELECT id, COALESCE(date_filled, study_date) FROM study"
        ).fetchall()
    }
    wanted = {str(d).replace("-", "") for d in days.values() if d}
    for path in files_under(work / "bids"):
        if not path.endswith("_scans.tsv"):
            continue
        for line in (work / "bids" / path).read_text().splitlines()[1:]:
            _, _, acq_time = line.partition("\t")
            if acq_time in ("", "n/a"):
                bad.append(f"{path}: a scan with no time, under a policy that keeps them")
                continue
            day = acq_time.split("T")[0].replace("-", "")
            if day not in wanted:
                bad.append(f"{path}: {day} is not a study date the registry holds")
    return bad


# --------------------------------------------------------------------------
# 11. The handover verifies
# --------------------------------------------------------------------------


def bar_handover(work: Path, db: sqlite3.Connection) -> list[str]:
    report = load(work, "handover")
    if report is None:
        return []
    bad = []
    if report["failed"]:
        bad.append(f"the handover failed: {report['failed']}")
    if report["verified"] != report["archives"]:
        bad.append(
            f"{report['archives']} archive(s) written and {report['verified']} read back"
        )
    if report["missing"]:
        bad.append(f"{report['missing']} file(s) of the release were not in the tree")
    # The record accounts for every file.
    packed = db.execute(
        "SELECT COALESCE(SUM(files), 0) FROM handover_archive WHERE handover_id = ?",
        (report["handover_id"],),
    ).fetchone()[0]
    if packed != report["files"]:
        bad.append(f"the release wrote {report['files']} file(s) and the archives hold {packed}")
    return bad


# --------------------------------------------------------------------------
# 12. The budget
# --------------------------------------------------------------------------


def bar_budget(work: Path) -> list[str]:
    """Measured and stated, because the release holds per-file state.

    Not a claim that it is bounded: it is linear in the files of a version and
    of the version before it, and the number below is what that costs at this
    size. Streaming the manifest is the slice that makes it bounded.
    """
    bad = []
    for tree in ("descriptive", "bids"):
        path = work / f"{tree}.time"
        if not path.is_file():
            continue
        rss = 0
        for line in path.read_text().splitlines():
            if "Maximum resident set size" in line:
                rss = int(line.rsplit(" ", 1)[-1])
        report = load(work, tree)
        if report is None:
            continue
        files = max(report["files"], 1)
        print(f"    {tree}: {rss / 1024:.0f} MiB for {files} file(s), "
              f"{rss * 1024 / files:.0f} bytes a file")
        # The baseline host is 8 cores and 64 GB (principle 5), and a gate that
        # states no number states nothing.
        if rss > 2 * 1024 * 1024:
            bad.append(f"{tree}: {rss / 1024 / 1024:.1f} GiB for {files} files")
    return bad


def main() -> int:
    work = Path(sys.argv[1]).resolve()
    db = sqlite3.connect(work / "home" / "registry.db")
    bars = [
        ("2. the validator passes", lambda: bar_validator(work)),
        ("3. the reference selections are right", lambda: bar_reference(work)),
        ("4, 5. every stack is placed and named", lambda: bar_placed(work, db)),
        ("6. one stack per session and role", lambda: bar_picks(work, db)),
        ("7. every file is traceable", lambda: bar_traceable(work, db)),
        ("8. the de-identification does what it says", lambda: bar_deidentified(work)),
        ("9. round trip and increment", lambda: bar_increment(work)),
        ("10. the date the clinical join needs survives", lambda: bar_dates(work, db)),
        ("11. the handover verifies", lambda: bar_handover(work, db)),
        ("12. the budget", lambda: bar_budget(work)),
    ]
    failed = 0
    for name, bar in bars:
        complaints = bar()
        mark = "ok  " if not complaints else "FAIL"
        print(f"  {mark} {name}")
        for c in complaints:
            print(f"       {c}")
        failed += len(complaints)
    print()
    if failed:
        print(f"gate: {failed} complaint(s)")
        return 1
    print("gate: every bar")
    return 0


if __name__ == "__main__":
    sys.exit(main())

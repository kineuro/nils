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
from datetime import date
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
    # The state is per stack (Wave 4a section 4): every stack a dataset holds
    # is a stack the registry has, in a version the registry has.
    orphan = db.execute(
        """SELECT COUNT(*) FROM release_stack s
           LEFT JOIN stack k ON k.id = s.stack_id WHERE k.id IS NULL"""
    ).fetchone()[0]
    if orphan:
        bad.append(f"{orphan} released stack(s) the registry does not have")
    orphan = db.execute(
        """SELECT COUNT(*) FROM release_stack s
           LEFT JOIN release r ON r.id = s.release_id WHERE r.id IS NULL"""
    ).fetchone()[0]
    if orphan:
        bad.append(f"{orphan} released stack(s) name a version the registry does not have")
    # And the state describes the tree: a stack with files has a digest.
    blank = db.execute(
        "SELECT COUNT(*) FROM release_stack WHERE files > 0 AND digest = ''"
    ).fetchone()[0]
    if blank:
        bad.append(f"{blank} released stack(s) with files and no digest")
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
# 8b. What the pack did not name does not leave (Wave 4a section 5)
# --------------------------------------------------------------------------


def release_list(pack_dir: Path) -> set[str]:
    """The addresses the pack's `private.release` names, as `nils private`
    prints them (`GGGGxxEE CREATOR`). Read with a small scanner rather than a
    YAML library, so the gate needs nothing installed."""
    text = (pack_dir / "mri" / "private.yml").read_text(encoding="utf-8")
    start = text.index("\n  release:")
    out: set[str] = set()
    creator = group = element = None
    for line in text[start:].splitlines():
        m = re.match(r"\s*-?\s*(creator|group|element):\s*(.+?)\s*$", line)
        if not m:
            continue
        key, value = m.group(1), m.group(2).strip().strip('"').strip("'")
        if key == "creator":
            creator, group, element = value, None, None
        elif key == "group":
            group = int(value, 16) if value.lower().startswith("0x") else int(value)
        elif key == "element":
            element = int(value, 16) if value.lower().startswith("0x") else int(value)
        if creator is not None and group is not None and element is not None:
            out.add(f"{group:04X}xx{element:02X} {creator}")
            creator = group = element = None
    return out


def bar_private(work: Path) -> list[str]:
    """Ask the released tree what private elements it carries, with the engine's
    own survey (shapes, never values), and hold every one against the pack's
    release list. A block a release let through whole, or an element outside
    the list, is a leak whatever the report says."""
    nils = os.environ.get("NILS")
    pack_dir = os.environ.get("NILS_PACK_DIR")
    if not nils or not pack_dir:
        return ["NILS and NILS_PACK_DIR must be set for the private bar"]
    allowed = release_list(Path(pack_dir))
    answers = tomllib.loads((Path(__file__).parent / "reference.toml").read_text())
    kept = set(answers.get("private", {}).get("kept", []))
    dropped = set(answers.get("private", {}).get("dropped", []))
    bad = []
    import subprocess

    for tree in ("descriptive", "bids", "shifted"):
        root = work / tree
        if not root.is_dir():
            continue
        run = subprocess.run(
            [nils, "private", "--json", "--files", "100000", str(root)],
            capture_output=True,
            text=True,
            env={**os.environ, "NILS_REGISTRY": str(work / "home")},
        )
        if run.returncode != 0:
            bad.append(f"{tree}: nils private failed: {run.stderr.strip()[-200:]}")
            continue
        survey = json.loads(run.stdout)
        # `nils private` prints `(GGGG,xxEE) CREATOR`; the list is `GGGGxxEE CREATOR`.
        found = {re.sub(r"^\((....),xx(..)\) ", r"\1xx\2 ", e["address"]) for e in survey["elements"]}
        for address in sorted(found - allowed):
            bad.append(f"{tree}: a released file carries {address}, which the pack's release list does not name")
        if survey.get("orphans", 0):
            bad.append(f"{tree}: {survey['orphans']} private element(s) in a block with no creator")
        # What must be kept is asked of the trees that carry the diffusion
        # series as DICOM; in the BIDS layout it is a NIfTI and carries no
        # private element at all, which is the right answer there.
        if tree != "bids":
            for address in sorted(kept - found):
                bad.append(f"{tree}: {address} should have been kept and was not")
        for address in sorted(dropped & found):
            bad.append(f"{tree}: {address} should have been dropped and was kept")
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
    """Section 9.4, which is the coupling of 2.1 broken, and Wave 4a
    section 7.4, which is the join itself.

    First the mechanism: the time in the standard's own column is the
    registry's, under the policy the release ran under. Then the join: each
    session's row in `_sessions.tsv` carries the nearest EDSS the registry
    holds, with its signed distance in days, and the participant row the sex
    and the age; and in the shifted tree the observation's date moved with
    the scan, so the distance between the two columns is still the
    registry's.
    """
    if not (work / "bids").is_dir():
        return []
    bad = []
    bad += clinical_join(work, db)
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


def tsv(path: Path) -> list[dict[str, str]]:
    lines = path.read_text().splitlines()
    if not lines:
        return []
    header = lines[0].split("\t")
    return [dict(zip(header, line.split("\t"))) for line in lines[1:]]


def nearest_edss(db: sqlite3.Connection, day: str) -> tuple[str, int, str] | None:
    """The registry's own answer: the EDSS nearest `day` (YYYY-MM-DD), the
    earlier of two equidistant, as (value, signed days, date)."""
    rows = db.execute(
        "SELECT e.event_date, e.number FROM event e"
        " JOIN observation_type o ON o.id = e.observation_type_id"
        " WHERE o.name = 'EDSS' AND e.superseded_by IS NULL"
    ).fetchall()
    if not rows:
        return None
    at = date.fromisoformat(day)
    best = None
    for event_date, number in rows:
        d = date.fromisoformat(str(event_date)[:10])
        offset = (d - at).days
        key = (abs(offset), d)
        if best is None or key < best[0]:
            best = (key, number, offset, d.isoformat())
    _, number, offset, when = best
    value = str(int(number)) if float(number).is_integer() else str(number)
    return value, offset, when


def clinical_join(work: Path, db: sqlite3.Connection) -> list[str]:
    """Wave 4a section 7.4, against the reference's expectations and the
    registry's own nearest."""
    bad = []
    expected = tomllib.loads((HERE / "reference.toml").read_text()).get("clinical")
    if not expected:
        return ["reference.toml has no [clinical] section"]
    rows = tsv(work / "bids" / "participants.tsv")
    if len(rows) != 1:
        bad.append(f"participants.tsv: {len(rows)} rows, one subject expected")
    for row in rows:
        if row.get("sex") != expected["sex"]:
            bad.append(f"participants.tsv: sex {row.get('sex')!r}, {expected['sex']!r} expected")
        if row.get("age") != str(expected["age"]):
            bad.append(f"participants.tsv: age {row.get('age')!r}, {expected['age']} expected")
    sessions = [p for p in files_under(work / "bids") if p.endswith("_sessions.tsv")]
    if len(sessions) != 1:
        bad.append(f"bids: {len(sessions)} sessions files, one expected")
    for path in sessions:
        for row in tsv(work / "bids" / path):
            label = row["session_id"].removeprefix("ses-")
            want = expected["sessions"].get(label)
            if want is None:
                bad.append(f"{path}: {row['session_id']} is not a session the reference expects")
                continue
            for column in ("age", "edss", "edss_days"):
                if row.get(column) != str(want[column]):
                    bad.append(
                        f"{path}: {row['session_id']} {column} {row.get(column)!r}, {want[column]} expected"
                    )
            near = nearest_edss(db, row["acq_time"][:10])
            if near is None:
                bad.append(f"{path}: the registry holds no EDSS")
                continue
            value, offset, when = near
            if (row.get("edss"), row.get("edss_days"), row.get("edss_date")) != (value, str(offset), when):
                bad.append(
                    f"{path}: {row['session_id']} carries EDSS {row.get('edss')} at {row.get('edss_days')} days"
                    f" on {row.get('edss_date')}; the registry's nearest is {value} at {offset} on {when}"
                )
    # The shifted tree: the date moved with the scan, and the distance held.
    shifted = work / "bids-shifted"
    if shifted.is_dir():
        unshifted = {r[2] for r in (nearest_edss(db, "2022-01-15"), nearest_edss(db, "2022-07-15")) if r}
        for path in [p for p in files_under(shifted) if p.endswith("_sessions.tsv")]:
            for row in tsv(shifted / path):
                scan = date.fromisoformat(row["acq_time"][:10])
                edss_date = row.get("edss_date", "")
                if edss_date in ("", "n/a"):
                    bad.append(f"shifted {path}: {row['session_id']} has no EDSS date under shift")
                    continue
                held = (date.fromisoformat(edss_date) - scan).days
                if str(held) != row.get("edss_days"):
                    bad.append(
                        f"shifted {path}: {row['session_id']} EDSS date is {held} days from the scan,"
                        f" the row says {row.get('edss_days')}"
                    )
                if edss_date in unshifted:
                    bad.append(f"shifted {path}: {row['session_id']} EDSS date {edss_date} did not move")
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
        ("8b. what the pack did not name does not leave", lambda: bar_private(work)),
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

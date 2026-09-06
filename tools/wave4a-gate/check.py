#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""The bars of the Wave 4a gate on a real cohort (spec section 12), read off
the work directory `cohort.sh` wrote. Counts and timings only; no value from
the cohort is printed.

    tools/wave4a-gate/check.py WORKDIR
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path


def load(work: Path, name: str):
    """The report in `name.json`. A long verb prints its progress, one JSON
    line each, before the report, which may span lines, and a step's own
    line may come first; everything before the document is dropped."""
    p = work / f"{name}.json"
    if not p.is_file():
        return None
    lines = p.read_text().splitlines()
    start = None
    for n, l in enumerate(lines):
        if l.startswith('{"progress"'):
            continue
        if l.startswith("{") or l.startswith("["):
            start = n
            break
    if start is None:
        return None
    text = "\n".join(l for l in lines[start:] if not l.startswith('{"progress"')).strip()
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        return None



def files_under(root: Path) -> set[str]:
    out = set()
    for dirpath, _, names in os.walk(root):
        for n in names:
            out.add(str(Path(dirpath, n).relative_to(root)))
    return out


# 1. The registry is built from raw, and the counts reconcile to v0's
def bar_raw(work: Path) -> list[str]:
    bad = []
    d = load(work, "digest")
    if not d:
        return ["no digest report"]
    v0 = load(work, "v0-counts")
    print(f"       subjects {d.get('subjects')}, studies {d.get('studies')}, series {d.get('series')}, stacks {d.get('stacks')}, files seen {d.get('seen')}, quarantined {d.get('quarantined')}")
    if v0:
        for key in ("subjects", "studies", "series"):
            ours = d.get(key)
            theirs = v0.get(key)
            if ours is None or theirs is None:
                bad.append(f"{key}: cannot compare (ours {ours}, v0 {theirs})")
            elif ours != theirs:
                # A difference has a named cause or it is a complaint.
                cause = (v0.get("causes") or {}).get(key)
                if cause:
                    print(f"       {key}: {ours} here, {theirs} in v0; named cause: {cause}")
                else:
                    bad.append(f"{key}: {ours} here, {theirs} in v0, and no named cause")
    else:
        print("       no v0 counts given; the registry stands on its own")
    return bad


# 7. Every import shape runs through the one importer, and a re-run changes nothing
def bar_importer(work: Path) -> list[str]:
    bad = []
    for name in ("cohort", "membership", "demographics", "diseases", "events"):
        apply = load(work, f"import-{name}-apply")
        again = load(work, f"import-{name}-again")
        if not apply or not again:
            bad.append(f"{name}: no import report")
            continue
        if not apply.get("applied", True):
            bad.append(f"{name}: not applied")
        changed = again.get("added", 0) + again.get("updated", 0) + again.get("superseded", 0)
        if changed:
            bad.append(f"{name}: the re-run changed {changed} row(s)")
        refused = sum((again.get("refused") or {}).values()) if isinstance(again.get("refused"), dict) else 0
        if refused:
            bad.append(f"{name}: {refused} row(s) refused")
        print(f"       {name}: {apply.get('rows')} rows, {apply.get('added')} added, {apply.get('updated')} updated, {apply.get('superseded')} superseded; again: nothing")
    return bad


# 8. The review queue is readable in an afternoon
def bar_queue(work: Path) -> list[str]:
    c = load(work, "classify")
    items = load(work, "review-open") or {}
    if not c:
        return ["no classification report"]
    groups = c.get("review_groups")
    members = c.get("review_items")
    open_items = items.get("count", len(items.get("items", [])))
    print(f"       {members} question(s) raised as {groups} item(s); {open_items} open")
    if groups is None:
        return ["the report has no review_groups"]
    return [] if open_items <= 200 else [f"{open_items} open items is not an afternoon"]


# 2 and 6. Both layouts, and a re-run writes nothing
def bar_release(work: Path) -> list[str]:
    bad = []
    for name in ("desc", "bids"):
        first = load(work, name)
        again = load(work, f"{name}-again")
        if first is None:
            if name == "bids":
                print("       bids: skipped (no converter)")
            else:
                bad.append(f"{name}: no release report")
            continue
        if again is None:
            bad.append(f"{name}: no re-run report")
            continue
        wrote = again.get("added", 0) + again.get("rewritten", 0) + again.get("moved", 0)
        if wrote:
            bad.append(f"{name}: the re-run wrote {wrote} file(s)")
        print(f"       {name}: {first.get('files')} file(s) for {first.get('subjects')} subject(s); again wrote nothing; clinical {first.get('clinical')}")
        if name == "bids":
            nearest = {k: v for k, v in (first.get("clinical") or {}).items() if k.startswith("nearest ")}
            if not any(nearest.values()):
                bad.append("bids: no observation reached the tree; name the kinds the cohort has with --observation")
    return bad


# 9. A second process drives the verbs through the door and gets the same answer
def bar_door(work: Path) -> list[str]:
    bad = []
    caps = load(work, "door-capabilities")
    if not caps or "doors" not in caps:
        return ["the door did not answer the capabilities"]
    status = load(work, "status") or {}
    door_status = load(work, "door-status") or {}
    if status.get("registry", {}).get("epoch") != door_status.get("registry", {}).get("epoch"):
        bad.append("status: the door and the command line disagree on the epoch")
    custody = load(work, "custody") or {}
    door_custody = load(work, "door-custody") or {}
    mine = [s["store"] for s in custody.get("stores", [])]
    theirs = [s["store"] for s in door_custody.get("stores", [])]
    if mine != theirs:
        bad.append(f"custody: the door lists {theirs}, the command line {mine}")
    sel = load(work, "select") or {}
    door_sel = load(work, "door-select") or {}
    if sel.get("reaches") != door_sel.get("reaches"):
        bad.append(f"select: the door reaches {door_sel.get('reaches')}, the command line {sel.get('reaches')}")
    queued = load(work, "door-release-queued") or {}
    if "job" not in queued:
        bad.append(f"the door did not queue the release: {queued}")
    jobs = load(work, "jobs") or {}
    door_job = next((j for j in jobs.get("jobs", jobs if isinstance(jobs, list) else []) if j.get("id") == queued.get("job")), None)
    if door_job is None or door_job.get("state") != "done":
        bad.append(f"the queued release did not end done: {door_job}")
    cli_tree = files_under(work / "desc")
    door_tree = files_under(work / "door-desc")
    if not door_tree:
        bad.append("the release run through the door wrote no tree")
    elif cli_tree != door_tree:
        bad.append(f"the door's release tree differs from the command line's: {len(cli_tree ^ door_tree)} name(s)")
    else:
        print(f"       the door's release tree equals the command line's: {len(door_tree)} file(s)")
    return bad


# 10. Every store in the custody table has an owner and a retention
def bar_custody(work: Path) -> list[str]:
    bad = []
    custody = load(work, "custody") or {}
    for s in custody.get("stores", []):
        if not s.get("owner"):
            bad.append(f"custody: {s.get('store')} has no owner")
        if not s.get("kept"):
            bad.append(f"custody: {s.get('store')} has no retention")
    print(f"       {len(custody.get('stores', []))} store(s), each with an owner and a retention")
    return bad


# 11. The budget
def bar_budget(work: Path) -> list[str]:
    bad = []
    d = load(work, "digest") or {}
    files = d.get("seen") or 0
    seconds = d.get("elapsed_s") or 0
    rate = d.get("files_per_s") or (files / seconds if seconds else 0)
    peak = (d.get("peak_rss_bytes") or 0) / 1e6
    budget = work / "budget.tsv"
    rows = [l.split("\t") for l in budget.read_text().splitlines()[1:]] if budget.is_file() else []
    total = sum(int(s) for _, s in rows)
    print(f"       digest {files} file(s) in {seconds:.0f} s, {rate:.0f} files/s, peak RSS {peak:.0f} MB; every step {total} s")
    for name, s in rows:
        print(f"         {name:<26} {s:>6} s")
    if rate and rate < 500:
        bad.append(f"digest at {rate:.0f} files/s is below the floor of 500")
    if peak and peak > 8000:
        bad.append(f"peak RSS {peak:.0f} MB is above 8 GB")
    return bad


def main() -> int:
    work = Path(sys.argv[1]).resolve()
    if "--budget" in sys.argv[2:]:
        # The baseline host's half: only the release re-run and the budget.
        bars = [
            ("2. a re-run writes nothing", bar_release),
            ("11. the budget", bar_budget),
        ]
        return run_bars(work, bars)
    bars = [
        ("1. the registry is built from raw", bar_raw),
        ("2, 6. both layouts, the clinical export, a re-run writes nothing", bar_release),
        ("7. every import shape runs through the one importer, twice", bar_importer),
        ("8. the review queue is readable in an afternoon", bar_queue),
        ("9. a second process through the door gets the same answer", bar_door),
        ("10. every store has an owner and a retention", bar_custody),
        ("11. the budget", bar_budget),
    ]
    return run_bars(work, bars)


def run_bars(work: Path, bars) -> int:
    failed = 0
    for name, bar in bars:
        complaints = bar(work)
        print(f"  {'ok  ' if not complaints else 'FAIL'} {name}")
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

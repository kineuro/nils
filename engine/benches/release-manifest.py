#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Measure a release and its re-run: peak memory, wall time, registry size
(docs/specs/wave4a-engine-completes.md, section 4.4).

    release-manifest.py --corpus DIR --packs DIR --work DIR --label NAME --nils BIN [--bids]

A corpus comes from the `corpus` example:

    target/release/examples/corpus --out DIR --instances 150000 --seed 1 --pixel-bytes 2048

Runs, in a fresh registry under WORK/LABEL: key add, init, digest, with
--bids also fingerprint and classify, then release v1 and release v2 with
nothing changed, each release in its own process so its peak resident set is
its own. Prints one JSON line per step: whether it succeeded, the wall time,
the peak resident set of the child in MB, and the registry file's size after
each release. Host-agnostic: no /usr/bin/time.
"""
import argparse
import json
import os
import resource
import shutil
import subprocess
import sys
import time
from pathlib import Path


def run(label, args, stdin=None, env=None):
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    t0 = time.monotonic()
    p = subprocess.run(args, input=stdin, capture_output=True, text=True, env=env)
    wall = time.monotonic() - t0
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    # ru_maxrss of RUSAGE_CHILDREN is the max over all waited-for children,
    # so it is monotone; the step's own peak is the max after this child
    # unless an earlier child was bigger, in which case we report the larger
    # figure and say so.
    return {
        "step": label,
        "ok": p.returncode == 0,
        "wall_s": round(wall, 2),
        "maxrss_mb": round(after.ru_maxrss / 1024, 1),
        "monotone_note": "peak is the max over all steps so far" if after.ru_maxrss == before.ru_maxrss else "",
        "stdout": p.stdout[-2000:],
        "stderr": p.stderr[-2000:],
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--corpus", required=True)
    ap.add_argument("--packs", required=True)
    ap.add_argument("--work", required=True)
    ap.add_argument("--label", required=True)
    ap.add_argument("--nils", required=True)
    ap.add_argument("--bids", action="store_true")
    ap.add_argument("--workers", default="8")
    a = ap.parse_args()

    work = Path(a.work) / a.label
    if work.exists():
        shutil.rmtree(work)
    work.mkdir(parents=True)
    home = work / "home"
    out = work / "out"
    reg = ["--registry", str(home)]
    env = dict(os.environ)

    steps = []
    steps.append(run("key", [a.nils, *reg, "key", "add", "k"], stdin="bench-key\n", env=env))
    steps.append(run("init", [a.nils, *reg, "init", "--key", "k", "--display-length", "10"], env=env))
    steps.append(run("digest", [a.nils, *reg, "digest", "--name", "bench", "--workers", a.workers, a.corpus], env=env))
    if a.bids:
        # A BIDS tree needs stacks that are classified, or everything routes nowhere.
        steps.append(run("fingerprint", [a.nils, *reg, "fingerprint"], env=env))
        steps.append(run("classify", [a.nils, *reg, "classify", "--pack-dir", a.packs], env=env))
    release = [a.nils, *reg, "release", "--name", "bench", "--on-unknown", "write", "--pack-dir", a.packs]
    if a.bids:
        release += ["--layout", "bids"]
    release += ["--out", str(out)]
    # Each release in its own process, so its peak RSS is its own: the
    # children's max is monotone, and the digest is the largest step until
    # the release is.
    for name in ("release-v1", "release-v2"):
        r = subprocess.run(
            [sys.executable, __file__, "--child", *release],
            capture_output=True, text=True, env=env,
        )
        try:
            steps.append(json.loads(r.stdout.strip().splitlines()[-1]) | {"step": name})
        except Exception:
            steps.append({"step": name, "ok": False, "stderr": r.stderr[-2000:], "stdout": r.stdout[-2000:]})
        db = home / "registry.db"
        steps[-1]["registry_mb"] = round(db.stat().st_size / 1e6, 1) if db.exists() else None
    for s in steps:
        s["label"] = a.label
        print(json.dumps({k: v for k, v in s.items() if k not in ("stdout", "stderr")}))
        if not s["ok"]:
            print("  stderr:", s.get("stderr", "")[-800:], file=sys.stderr)
            print("  stdout:", s.get("stdout", "")[-800:], file=sys.stderr)
    # The tree's file count, so the two designs are shown to have written the same thing.
    n = sum(1 for _ in out.rglob("*") if _.is_file())
    print(json.dumps({"label": a.label, "tree_files": n}))


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--child":
        r = run("child", sys.argv[2:])
        print(json.dumps(r))
        sys.exit(0 if r["ok"] else 1)
    main()

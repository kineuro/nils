# SPDX-License-Identifier: AGPL-3.0-only
"""Which stacks should the physics vote be allowed to learn from?

    vote_reference.py holdout --v0 SRC --csv chain.csv --truth rules.csv --reference FILE
    vote_reference.py order   --v0 SRC --csv chain.csv --truth rules.csv --parts 8

The vote answers a stack whose base or technique the rules left empty by asking
the stacks nearest to it in physics what they are. Which stacks are in that
reference is the one thing v0 never decides in code: it reads whatever the
database holds, so the reference is whatever earlier runs happened to write,
including their own answers. Three candidates, and this measures all three:

  rules     only stacks a rule decided
  filled    rules, plus the vote's own answers, iterated until it settles
  stored    whatever the live database holds, which is `filled` for the
            cohorts sorted long ago and `rules` for the ones sorted last

`holdout` measures accuracy. It hides a random share of the stacks the rules
decided, builds the reference from a candidate without them, asks the vote what
they are, and compares its answer with what the rule said. The population is
easier than the one the vote really faces, since a stack whose text named its
own technique is not a hard case, so the absolute number is an upper bound. As
a comparison between candidates, on one population under one protocol, it is
fair.

`order` measures whether the answer is a fact about the stack. It splits the
stacks into parts, sorts them one after another the way a database receives
them, and does it again with the parts in the opposite order. A stack whose
answer depends on which part it arrived in was not answered by its physics.
Then it sorts the finished database once more, from each of the two histories,
and asks whether the two agree: a reference the vote can add to carries its
history forward for ever, and a reference built from the rules does not, so
running the sort again is either a repair or nothing at all.

v0 is private and is never copied into this repository: `--v0` is the
`backend/src` of a checkout on the host.
"""

from __future__ import annotations

import argparse
import csv
import random
import sys
from collections import Counter

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from resort import PHYSICS, fill, module, needs_fill, read_cache, reference_of, rows  # noqa: E402

csv.field_size_limit(1 << 30)

#: what `find_best_match` says when it declined to answer
DECLINED = ("no_match", "insufficient_matches", "no_compatible_match")


def load(csv_path: str, truth_path: str) -> list[dict]:
    """The rules' verdicts with the physics the vote reads joined back on."""
    truth = read_cache(truth_path)
    fp_by_id = {str(fp["series_stack_id"]): fp for fp in rows(csv_path)}
    for s in truth:
        fp = fp_by_id.get(str(s["series_stack_id"]), {})
        s["modality"] = fp.get("modality")
        for k in PHYSICS:
            s[k] = fp.get(k)
    return truth


def decided(s: dict) -> bool:
    """A stack the rules answered on both axes the vote fills, which is what
    makes it usable as a held-out question with a known answer."""
    base, tech, dt = s.get("base"), s.get("technique"), s.get("directory_type")
    if s.get("modality") != "MR":
        return False
    if not base or base == "Unknown" or not tech or tech == "Unknown":
        return False
    return bool(dt) and dt != "excluded"


def ask(gap, by_intent, global_db, s: dict):
    """v0's phase 3, for one stack: its own pool, then the global one unless
    the stack is `misc`."""
    dt = s.get("directory_type")
    db = by_intent.get(dt, global_db)
    look = lambda d: gap.find_best_match(  # noqa: E731
        ref_db=d,
        tr=s.get("mr_tr"),
        te=s.get("mr_te"),
        ti=s.get("mr_ti"),
        fa=s.get("mr_flip_angle"),
        n_instances=s.get("stack_n_instances"),
        scanning_sequence=s.get("scanning_sequence"),
    )
    r = look(db)
    if r.method in DECLINED and dt != "misc":
        r = look(global_db)
    return r


def holdout(a) -> int:
    gap = module(a.v0, "sort/gap_filling.py", "v0_gap_filling")
    truth = load(a.csv, a.truth)
    answerable = [s for s in truth if decided(s)]

    rnd = random.Random(a.seed)
    held = set(rnd.sample([str(s["series_stack_id"]) for s in answerable], k=int(len(answerable) * a.fraction)))
    print(f"{len(answerable)} stacks the rules decided, {len(held)} held out", file=sys.stderr)

    source = truth if not a.reference else load(a.csv, a.reference)
    pool = reference_of([s for s in source if str(s["series_stack_id"]) not in held])
    by_intent, global_db = gap.build_intent_scoped_databases(pool)
    print(
        f"reference: {len(pool)} stacks in {len(by_intent)} pools "
        f"(source {a.reference or a.truth})",
        file=sys.stderr,
    )

    stat = Counter()
    by_method = Counter()
    for s in truth:
        if str(s["series_stack_id"]) not in held:
            continue
        stat["asked"] += 1
        r = ask(gap, by_intent, global_db, s)
        by_method[r.method] += 1
        if r.method in DECLINED or not (r.base or r.technique):
            stat["declined"] += 1
            continue
        stat["answered"] += 1
        if r.base == s["base"]:
            stat["base_right"] += 1
        if r.technique == s["technique"]:
            stat["technique_right"] += 1
        if r.base == s["base"] and r.technique == s["technique"]:
            stat["both_right"] += 1

    name = a.name or (a.reference or a.truth)
    print(f"\n== {name} ==")
    asked, answered = stat["asked"], stat["answered"]
    print(f"held out          {asked}")
    print(f"answered          {answered}  ({pct(answered, asked)} of those asked)")
    print(f"declined          {stat['declined']}")
    print(f"base right        {stat['base_right']}  ({pct(stat['base_right'], answered)} of answers)")
    print(f"technique right   {stat['technique_right']}  ({pct(stat['technique_right'], answered)} of answers)")
    print(f"both right        {stat['both_right']}  ({pct(stat['both_right'], answered)} of answers)")
    print(f"both right        {pct(stat['both_right'], asked)} of those asked")
    print(f"reference size    {len(pool)}")
    print("by method         " + ", ".join(f"{m}={n}" for m, n in by_method.most_common()))
    return 0


def pct(n: int, of: int) -> str:
    return "n/a" if not of else f"{100.0 * n / of:.2f}%"


def order(a) -> int:
    """Sort the cohort in parts, twice, with the parts in opposite orders."""
    gap = module(a.v0, "sort/gap_filling.py", "v0_gap_filling")
    truth = load(a.csv, a.truth)

    ids = sorted({str(s["series_stack_id"]) for s in truth})
    rnd = random.Random(a.seed)
    rnd.shuffle(ids)
    size = (len(ids) + a.parts - 1) // a.parts
    parts = [set(ids[i : i + size]) for i in range(0, len(ids), size)]
    print(f"{len(ids)} stacks in {len(parts)} parts of about {size} "
          f"({a.reference_policy} reference)", file=sys.stderr)

    forward, state_f = run_in_parts(gap, truth, parts, a.reference_policy)
    backward, state_b = run_in_parts(gap, truth, list(reversed(parts)), a.reference_policy)

    both = set(forward) | set(backward)
    same = sum(1 for k in both if forward.get(k) == backward.get(k))
    only_f = sum(1 for k in both if k in forward and k not in backward)
    only_b = sum(1 for k in both if k in backward and k not in forward)
    differ = [(k, forward.get(k), backward.get(k)) for k in both if forward.get(k) != backward.get(k)]

    print(f"\n== the same cohort, the parts in two orders ==")
    print(f"filled either way {len(both)}")
    print(f"same answer       {same}  ({pct(same, len(both))})")
    print(f"different         {len(differ)}  ({pct(len(differ), len(both))})")
    print(f"  filled only forwards  {only_f}")
    print(f"  filled only backwards {only_b}")
    shapes = Counter(f"{f} / {b}" for _, f, b in differ)
    for shape, n in shapes.most_common(12):
        print(f"    {n:>7}  {shape}")

    # And now the question that matters for a system that keeps running: can a
    # sort of the finished database put the two histories back together?
    again_f = resort_once(gap, truth, state_f, a.reference_policy)
    again_b = resort_once(gap, truth, state_b, a.reference_policy)
    keys = set(again_f) | set(again_b)
    agree = sum(1 for k in keys if again_f.get(k) == again_b.get(k))
    print(f"\n== the finished database, sorted once more from each history ==")
    print(f"answered          {len(keys)}")
    print(f"the two agree     {agree}  ({pct(agree, len(keys))})")
    print(f"still different   {len(keys) - agree}")
    return 0


def resort_once(gap, truth: list[dict], state: dict[str, dict], policy: str) -> dict[str, str]:
    """One more sort of everything, over a database that already holds what the
    history left in it."""
    fresh = {}
    for s in truth:
        k = str(s["series_stack_id"])
        prior = state.get(k, {})
        row = dict(s)
        if policy == "filled":
            # The fill wrote into the cache, so the next sort reads it back and
            # has no way to tell it from what a rule decided.
            for axis in ("base", "technique", "directory_type"):
                if prior.get(axis):
                    row[axis] = prior[axis]
        fresh[k] = row
    reference = reference_of(list(fresh.values()))
    batch = [r for r in fresh.values() if needs_fill(r)]
    fill(gap, batch, reference)
    return {
        str(r["series_stack_id"]): f"{r.get('base') or ''}|{r.get('technique') or ''}"
        for r in batch
        if r.get("filled_base") or r.get("filled_technique")
    }


def run_in_parts(gap, truth: list[dict], parts: list[set], policy: str):
    """One pass of v0's own semantics: each part is sorted against everything
    already in the database, and its answers stay there for the next part.

    Under the `rules` policy the answers are still written, because they are
    what the cache holds and what a person reads, but they are kept out of the
    reference the next part is judged against."""
    state = {str(s["series_stack_id"]): dict(s) for s in truth}
    seen: set[str] = set()
    answers: dict[str, str] = {}
    for i, part in enumerate(parts, 1):
        seen |= part
        pool = [state[k] for k in seen if k in state]
        reference = reference_of(
            pool if policy == "filled" else [r for r in pool if not r.get("fill_attempted")]
        )
        batch = [state[k] for k in part if k in state]
        n = fill(gap, batch, reference)
        print(f"  part {i}: {n} filled against {len(reference)}", file=sys.stderr)
        for s in batch:
            if s.get("filled_base") or s.get("filled_technique"):
                answers[str(s["series_stack_id"])] = f"{s.get('base') or ''}|{s.get('technique') or ''}"
    return answers, state


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("mode", choices=("holdout", "order"))
    ap.add_argument("--v0", required=True)
    ap.add_argument("--csv", required=True, help="the fingerprint CSV")
    ap.add_argument("--truth", required=True, help="the rules' verdicts, with no fill applied")
    ap.add_argument("--reference", help="the cache the reference is built from, if not --truth")
    ap.add_argument("--name", help="what to call this candidate in the report")
    ap.add_argument("--fraction", type=float, default=0.05)
    ap.add_argument("--parts", type=int, default=8)
    ap.add_argument(
        "--reference-policy",
        default="filled",
        choices=("filled", "rules"),
        dest="reference_policy",
        help="for order: whether a part's own answers join the reference the next part sees",
    )
    ap.add_argument("--seed", type=int, default=20260904)
    a = ap.parse_args()

    sys.path.insert(0, a.v0)
    return holdout(a) if a.mode == "holdout" else order(a)


if __name__ == "__main__":
    raise SystemExit(main())

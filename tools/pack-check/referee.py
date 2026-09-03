# SPDX-License-Identifier: AGPL-3.0-only
"""v0's own detectors over the same stacks, writing the same rows.

    referee.py --v0 SRC --csv FILE.csv --axis technique

`SRC` is the `backend/src` of a v0 0.5.3 checkout. v0 is private and is never
copied into this repository: the checker imports it from wherever it is on the
host. The output is one row per stack, `id \t axis \t value \t tier \t matched`,
which is what `classify_csv` writes, so the two diff directly.
"""

from __future__ import annotations

import argparse
import csv
import sys

csv.field_size_limit(1 << 30)

NUMERIC = {"mr_tr", "mr_te", "mr_ti", "mr_flip_angle", "fov_x", "fov_y", "aspect_ratio"}
INTEGER = {"mr_echo_train_length", "stack_n_instances"}


def rows(path: str):
    with open(path, newline="", encoding="utf-8") as fh:
        for r in csv.DictReader(fh):
            for k, v in list(r.items()):
                if v == "":
                    r[k] = None
                elif k in NUMERIC:
                    try:
                        r[k] = float(v)
                    except ValueError:
                        r[k] = None
                elif k in INTEGER:
                    try:
                        r[k] = int(float(v))
                    except ValueError:
                        r[k] = None
            yield r


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--v0", required=True)
    ap.add_argument("--csv", required=True)
    ap.add_argument("--axis", required=True)
    ap.add_argument(
        "--verdicts",
        help="for --axis vote: v0's series_classification_cache, which is the reference it votes against",
    )
    a = ap.parse_args()

    sys.path.insert(0, a.v0)
    # Not a detector but the phase that runs after all of them, against a
    # reference built from v0's own verdicts.
    if a.axis == "vote":
        return vote(a, src=a.v0)
    from classification.core.context import ClassificationContext  # noqa: E402

    if a.axis == "technique":
        from classification.detectors.technique import TechniqueDetector  # noqa: E402

        det = TechniqueDetector()

        def decide(ctx):
            r = det.detect_technique(ctx)
            return r.technique, r.detection_method, first_evidence(r)

    elif a.axis == "provenance":
        from classification.detectors.provenance import ProvenanceDetector  # noqa: E402

        det = ProvenanceDetector()

        def decide(ctx):
            r = det.detect(ctx)
            return r.provenance, r.detection_method, first_evidence(r)

    elif a.axis == "modifier":
        from classification.detectors.modifier import ModifierDetector  # noqa: E402

        det = ModifierDetector()

        def decide(ctx):
            r = det.detect_modifiers(ctx)
            return r.modifier_csv, "", ""

    elif a.axis == "construct":
        from classification.detectors.construct import ConstructDetector  # noqa: E402

        det = ConstructDetector()

        def decide(ctx):
            r = det.detect(ctx)
            return r.construct_csv, "", ""

    elif a.axis.startswith("pipeline:"):
        # The whole of v0's classifier, not one detector: what a route
        # overrides is only visible here.
        from classification.pipeline import ClassificationPipeline  # noqa: E402

        pipe = ClassificationPipeline()
        want = a.axis.split(":", 1)[1]

        def decide(ctx):
            r = pipe.classify(ctx)
            v = {
                "base": r.base,
                "technique": r.technique,
                "construct": r.construct_csv,
                "modifier": r.modifier_csv,
                "provenance": r.provenance,
                "directory_type": r.directory_type,
                "body_part": r.body_part,
                "post_contrast": None if r.post_contrast is None else str(r.post_contrast),
            }[want]
            return v or "", "", ""

    elif a.axis == "directory_type":
        from classification.pipeline import ClassificationPipeline  # noqa: E402

        pipe = ClassificationPipeline()

        def decide(ctx):
            r = pipe.classify(ctx)
            return r.directory_type or "", "", ""

    elif a.axis == "post_contrast":
        from classification.detectors.contrast import ContrastDetector  # noqa: E402

        det = ContrastDetector()

        def decide(ctx):
            r = det.detect_contrast(ctx)
            v = "" if r.post_contrast is None else str(r.post_contrast)
            return v, r.detection_method, ""

    elif a.axis == "body_part":
        from classification.detectors.body_part import BodyPartDetector  # noqa: E402

        det = BodyPartDetector()

        def decide(ctx):
            r = det.detect_body_part(ctx)
            return r.body_part or "", r.detection_method, r.matched_keyword or ""

    elif a.axis == "base":
        from classification.detectors.base_contrast import BaseContrastDetector  # noqa: E402
        from classification.detectors.technique import TechniqueDetector  # noqa: E402

        base_det = BaseContrastDetector()
        tech_det = TechniqueDetector()

        def decide(ctx):
            # v0 hands the technique to the base detector, which is why the
            # pack decides technique before base (spec section 6.3).
            technique = tech_det.detect_technique(ctx).technique
            r = base_det.detect_base(ctx, technique=technique)
            return r.base, r.detection_method, first_evidence(r)

    elif a.axis == "search_text":
        # Not an axis: the normalized blob v0 builds at extract time and
        # stores, which the pack builds when it is loaded.
        import importlib.util

        spec = importlib.util.spec_from_file_location(
            "v0_normalizer", f"{a.v0}/sort/semantic_normalizer.py"
        )
        mod = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = mod
        spec.loader.exec_module(mod)

        def decide(ctx):
            parts = [
                ctx.series_description,
                ctx.protocol_name,
                ctx.stack_sequence_name,
                ctx.body_part_examined,
                ctx.series_comments,
                ctx.image_comments,
            ]
            text = " ".join(p for p in parts if p)
            return mod.normalize_text_blob(text) or "", "", ""

    else:
        raise SystemExit(f"the referee does not know the {a.axis} axis yet")

    out = sys.stdout
    name = a.axis.split(":", 1)[-1]
    for fp in rows(a.csv):
        ctx = ClassificationContext.from_fingerprint(fp)
        value, tier, matched = decide(ctx)
        out.write(f"{fp['series_stack_id']}\t{name}\t{value}\t{tier}\t{matched}\n")
    return 0


def vote(a, src: str) -> int:
    """v0's `sort/gap_filling.py`, over a reference built from its own
    verdicts, writing one row per MR stack in the shape `classify_csv` writes
    a pass in: the answer, the method that found it, and how many neighbours
    agreed."""
    import importlib.util
    from pathlib import Path

    if not a.verdicts:
        raise SystemExit("--axis vote needs --verdicts")
    path = Path(src) / "sort" / "gap_filling.py"
    spec = importlib.util.spec_from_file_location("v0_gap_filling", path)
    mod = importlib.util.module_from_spec(spec)
    # `dataclasses` resolves a field's type through sys.modules, so the module
    # has to be registered before it is executed.
    sys.modules[spec.name] = mod
    spec.loader.exec_module(mod)

    verdicts = {}
    with open(a.verdicts, newline="", encoding="utf-8") as fh:
        for r in csv.DictReader(fh):
            verdicts[r["series_stack_id"]] = r

    stacks = list(rows(a.csv))
    reference = []
    for fp in stacks:
        v = verdicts.get(str(fp["series_stack_id"]))
        if not v or fp.get("modality") != "MR":
            continue
        base, tech, dt = v.get("base"), v.get("technique"), v.get("directory_type")
        if not base or base == "Unknown" or not tech or tech == "Unknown":
            continue
        if not dt or dt == "excluded":
            continue
        reference.append(
            {
                "series_stack_id": fp["series_stack_id"],
                "base": base,
                "technique": tech,
                "directory_type": dt,
                "mr_tr": fp.get("mr_tr"),
                "mr_te": fp.get("mr_te"),
                "mr_ti": fp.get("mr_ti"),
                "mr_flip_angle": fp.get("mr_flip_angle"),
                "stack_n_instances": fp.get("stack_n_instances"),
            }
        )
    by_intent, global_db = mod.build_intent_scoped_databases(reference)
    print(
        f"reference: {len(reference)} stacks, {len(by_intent)} pools",
        file=sys.stderr,
    )

    out = sys.stdout
    for fp in stacks:
        if fp.get("modality") != "MR":
            continue
        v = verdicts.get(str(fp["series_stack_id"]))
        dt = (v or {}).get("directory_type") or None
        db = by_intent.get(dt, global_db)
        which = "scoped" if dt in by_intent else "global"
        ask = lambda db: mod.find_best_match(  # noqa: E731
            ref_db=db,
            tr=fp.get("mr_tr"),
            te=fp.get("mr_te"),
            ti=fp.get("mr_ti"),
            fa=fp.get("mr_flip_angle"),
            n_instances=fp.get("stack_n_instances"),
            scanning_sequence=fp.get("scanning_sequence"),
        )
        r = ask(db)
        if (
            r.method in ("no_match", "insufficient_matches", "no_compatible_match")
            and dt != "misc"
        ):
            r = ask(global_db)
            which = "global"
        answer = f"{r.base or ''}|{r.technique or ''}"
        out.write(
            f"{fp['series_stack_id']}\tvote\t{answer}\t{r.method}"
            f"\t{r.match_count} of {r.total_in_bin} in {which}\n"
        )
    return 0


def first_evidence(result) -> str:
    """What v0 cites, in the shape `classify_csv` writes it."""
    if not getattr(result, "evidence", None):
        return ""
    e = result.evidence[0]
    return str(e.value)


if __name__ == "__main__":
    raise SystemExit(main())

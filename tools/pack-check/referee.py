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
    a = ap.parse_args()

    sys.path.insert(0, a.v0)
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


def first_evidence(result) -> str:
    """What v0 cites, in the shape `classify_csv` writes it."""
    if not getattr(result, "evidence", None):
        return ""
    e = result.evidence[0]
    return str(e.value)


if __name__ == "__main__":
    raise SystemExit(main())

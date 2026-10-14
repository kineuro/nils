# SPDX-License-Identifier: AGPL-3.0-only
"""A predictor that is data: words in a field vote for a value, with a
calibrated probability per rule. It is the bundle kind the tests and a
site's first trial use; the cheap model of record 55 D5 is a ``module``
bundle the site builds from its research.

``keywords.json``::

    {"rules": [{"axis": "base", "field": "text_series_description",
                "contains": "flair", "value": "FLAIR", "p": 0.95}, ...],
     "default_p": 0.5,
     "reliability": {"missing_fields": ["text_series_description",
                                        "text_protocol_name"],
                     "base": 0.99, "per_missing": 0.3}}

The first rule of an axis whose field contains its word (case folded)
answers; an axis no rule answers is answered "no value" at ``default_p``.
The reliability is ``base`` less ``per_missing`` for each listed field the
header lacks, never below zero: a header stripped of its names is one the
rules cannot be trusted on.
"""

from __future__ import annotations


class Keywords:
    def __init__(self, doc: dict, axes: list[str]) -> None:
        self.axes = list(axes)
        self.rules = list(doc.get("rules") or [])
        self.default_p = float(doc.get("default_p", 0.5))
        rel = doc.get("reliability") or {}
        self.rel_fields = list(rel.get("missing_fields") or [])
        self.rel_base = float(rel.get("base", 1.0))
        self.rel_step = float(rel.get("per_missing", 0.0))

    def predict(self, records: list[dict], rows: dict | None = None) -> list[dict]:
        out = []
        for r in records:
            fp = r.get("fingerprint") or {}
            said = {}
            for axis in self.axes:
                hit = None
                for rule in self.rules:
                    if rule.get("axis") != axis:
                        continue
                    text = fp.get(rule.get("field"))
                    if isinstance(text, str) and str(rule.get("contains", "")).lower() in text.lower():
                        hit = rule
                        break
                if hit is None:
                    said[axis] = {"value": None, "p": self.default_p}
                else:
                    said[axis] = {"value": hit["value"], "p": float(hit.get("p", 0.9))}
            missing = sum(1 for f in self.rel_fields if not fp.get(f))
            reliability = max(0.0, self.rel_base - self.rel_step * missing)
            out.append({"axes": said, "reliability": reliability})
        return out

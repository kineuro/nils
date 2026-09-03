# SPDX-License-Identifier: AGPL-3.0-only
"""Shapes and patterns: a value never reaches a report, its shape does (the
engine's rule, `nils-dicom/src/diagnostic.rs`: digits `9`, lower case `a`,
upper case `A`, the rest kept, forty characters at most), and a divergence
is named by the pattern the two values follow, not by the values."""

from __future__ import annotations

import math

SHAPE_MAX = 40


def shape(value: object) -> str:
    """The shape of a value, the engine's way."""
    if value is None:
        return "null"
    text = value if isinstance(value, str) else _number(value)
    out: list[str] = []
    for i, c in enumerate(text):
        if i == SHAPE_MAX:
            out.append("…")
            break
        if c.isdigit():
            out.append("9")
        elif c.islower():
            out.append("a")
        elif c.isupper():
            out.append("A")
        elif c.isalpha():
            out.append("a")
        else:
            out.append(c)
    return "".join(out)


def _number(value: object) -> str:
    if isinstance(value, float):
        return repr(value)
    return str(value)


def _as_float(text: str) -> float | None:
    try:
        f = float(text)
    except ValueError:
        return None
    return f if math.isfinite(f) else None


def _parts(text: str) -> list[str]:
    return text.split("\\")


def pattern(a: object, b: object, converter: str) -> str:
    """The pattern of a divergence between v0's `a` and v1's `b`, both in
    normal form: which side is null, or how two present values differ, in
    the order the cheapest explanation comes first."""
    if a is None and b is None:
        return "equal"
    if a is None:
        return "null↔value"
    if b is None:
        return "value↔null"
    if converter in ("int", "double"):
        return _numeric_pattern(float(a), float(b))
    sa, sb = str(a), str(b)
    if sa == sb:
        return "equal"
    if sa.lower() == sb.lower():
        return "case"
    if "".join(sa.split()) == "".join(sb.split()):
        return "whitespace"
    fa, fb = _as_float(sa), _as_float(sb)
    if fa is not None and fb is not None:
        return _numeric_pattern(fa, fb)
    pa, pb = _parts(sa), _parts(sb)
    if len(pa) > 1 or len(pb) > 1:
        if sorted(pa) == sorted(pb):
            return "list-order"
        if set(pa) < set(pb) or set(pb) < set(pa):
            return "subset"
        fpa = [_as_float(p) for p in pa]
        fpb = [_as_float(p) for p in pb]
        if len(pa) == len(pb) and None not in fpa and None not in fpb:
            if all(_close(x, y) for x, y in zip(fpa, fpb)):
                return "rounded"
    if sa.startswith(sb) or sb.startswith(sa):
        return "prefix"
    return f"{shape(sa)}↔{shape(sb)}"


def _close(x: float, y: float) -> bool:
    return math.isclose(x, y, rel_tol=1e-6, abs_tol=1e-6)


def _numeric_pattern(x: float, y: float) -> str:
    if x == y:
        return "number-format"
    for decimals in range(0, 7):
        if round(y, decimals) == x or round(x, decimals) == y:
            return "rounded"
    if _close(x, y):
        return "rounded"
    if x != 0 and y != 0 and abs(math.log10(abs(x)) - math.log10(abs(y))) >= 0.99:
        return "scale"
    return f"{shape(x)}↔{shape(y)}"

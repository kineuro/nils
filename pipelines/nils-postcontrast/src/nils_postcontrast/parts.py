# SPDX-License-Identifier: AGPL-3.0-only
"""The run's model inputs (``/inputs/manifest.json``), each checked against
its digest, and the cascade file that names them."""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass
from pathlib import Path


class PartsError(ValueError):
    pass


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return "sha256:" + h.hexdigest()


@dataclass
class Part:
    input: str
    name: str | None
    version: str | None
    digest: str
    card: dict
    artifact: Path

    def ref(self) -> dict:
        return {"name": self.name, "version": self.version, "digest": self.digest}


def parts(inputs: Path) -> dict:
    mf = Path(inputs) / "manifest.json"
    try:
        doc = json.loads(mf.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        raise PartsError("the inputs have no readable manifest.json") from e
    out = {}
    for m in doc.get("models") or []:
        if not isinstance(m, dict) or not m.get("input"):
            continue
        art = m.get("artifact")
        if not art:
            raise PartsError(f"the model input {m['input']} has no artifact")
        rel = Path(art)
        if rel.is_absolute():
            ps = rel.parts
            rel = Path(*ps[2:]) if len(ps) > 2 and ps[1] == "inputs" else Path(*ps[1:])
        if ".." in rel.parts:
            raise PartsError(f"the model input {m['input']}'s artifact leaves the inputs")
        out[m["input"]] = Part(m["input"], m.get("name"), m.get("version"), str(m.get("digest") or ""), m.get("card") or {}, Path(inputs) / rel)
    return out


def checked(p: Part) -> Part:
    if not p.artifact.is_file():
        raise PartsError(f"the {p.input} model's artifact is not in the inputs")
    if not p.digest.startswith("sha256:") or sha256_file(p.artifact) != p.digest:
        raise PartsError(f"the {p.input} model's artifact is not the one its digest names")
    if p.card.get("digest") not in (None, p.digest):
        raise PartsError(f"the {p.input} model's card names another artifact")
    return p


@dataclass
class Cascade:
    part: Part
    student: Part
    fusion: Part
    m: float
    name: str
    doc: dict

    @property
    def digest(self) -> str:
        return self.part.digest


def cascade(inputs: Path) -> Cascade:
    """The cascade the run was given, with its two parts, each checked
    against the cascade's file and card."""
    ps = parts(inputs)
    for need in ("student", "fusion", "cascade"):
        if need not in ps:
            raise PartsError(f"the run was given no {need} model")
    cas, stu, fus = (checked(ps[k]) for k in ("cascade", "student", "fusion"))
    try:
        doc = json.loads(cas.artifact.read_text(encoding="utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as e:
        raise PartsError("the cascade is not JSON") from e
    if not isinstance(doc, dict) or not isinstance(doc.get("parts"), dict):
        raise PartsError("the cascade file names no parts")
    given = {"student": stu.digest, "fusion": fus.digest}
    for k, v in given.items():
        if doc["parts"].get(k) != v:
            raise PartsError(f"the cascade names another {k} than the one the run was given")
    cp = cas.card.get("parts")
    if isinstance(cp, dict):
        for k, v in cp.items():
            if k in given and v != given[k]:
                raise PartsError(f"the cascade's card names another {k} than its file")
    st = doc.get("stage1") or {}
    if st.get("rule") != "margin" or not isinstance(st.get("m"), (int, float)) or isinstance(st.get("m"), bool):
        raise PartsError("the cascade's first stage is not a margin rule with its m")
    if st.get("k") != 3:
        raise PartsError("the cascade's student is not the three-slice one this image reads")
    return Cascade(cas, stu, fus, float(st["m"]), str(doc.get("name") or ""), doc)

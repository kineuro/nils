# SPDX-License-Identifier: AGPL-3.0-only
"""The ``stacks`` input layout: ``/input/stacks.json`` (``contracts/job/v1``,
``stacks.schema.json``).

    {"contract": "job/v1",
     "sources": [{"id": 0, "mount": "/source/0"}],
     "stacks": [
       {"unit": "stack-12", "stack_id": 12,
        "files": [{"source": 0, "path": "a/1.dcm", "frames": null},
                  {"source": 0, "path": "a/mf.dcm", "frames": "1-40"}],
        "orientation": "axial", "body_part": null, "technique": "MPRAGE"}]}

A stack's files are in its slice order, and a slice index counts their
frames in that order from 0, as v0 counted them. ``frames`` names the
frames of a multi-frame file that are the stack's, from one, as ranges
(``1-4,9``); null is every frame. A stack of one file whose frames are null
is asked its NumberOfFrames; a stack of many such files is one frame per
file, which is what a classic series is, since reading every header of an
archive to count frames would cost more than the embedding.

This image also reads three keys the contract leaves to the runner, where
the runner gives them: ``orientation`` (which slices inference reads; none
means the centre three) and the pack's own ``body_part`` and ``technique``
(the seeder's pools and strata; none means every stack is in the null pool).
"""

from __future__ import annotations

import json
import re
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path


@dataclass(frozen=True)
class Slice:
    path: str  # absolute, under a source mount
    frame: int  # from 0


@dataclass
class Stack:
    stack_id: int
    slices: list[Slice]
    unit: str
    orientation: str | None = None
    body_part: str | None = None
    technique: str | None = None
    extra: dict = field(default_factory=dict)

    @property
    def num_slices(self) -> int:
        return len(self.slices)


class ManifestError(ValueError):
    pass


_RANGES = re.compile(r"^\s*\d+(\s*-\s*\d+)?(\s*,\s*\d+(\s*-\s*\d+)?)*\s*$")


def parse_frames(text: str) -> list[int]:
    """``1-4,9`` (from one) to [0, 1, 2, 3, 8] (from zero), in the order written."""
    if not _RANGES.match(text):
        raise ManifestError("a frame range that is not ranges from one, such as 1-4,9")
    out: list[int] = []
    for part in text.split(","):
        a, _, b = part.partition("-")
        lo, hi = int(a), int(b or a)
        if lo < 1 or hi < lo:
            raise ManifestError("a frame range that is empty or starts below one")
        out.extend(range(lo - 1, hi))
    return out


def number_of_frames(path: str) -> int:
    """NumberOfFrames from a file's header, 1 when it has none or cannot be read."""
    try:
        import pydicom

        ds = pydicom.dcmread(path, stop_before_pixels=True)
        return max(1, int(getattr(ds, "NumberOfFrames", 1) or 1))
    except Exception:
        return 1


def _safe_join(mount: Path, rel: str) -> str:
    p = Path(rel)
    if p.is_absolute() or ".." in p.parts:
        raise ManifestError("a file path is absolute or leaves its source mount")
    return str(mount / p)


KNOWN = {"unit", "stack_id", "files", "orientation", "body_part", "technique"}


def parse(
    doc: dict,
    *,
    source_root: Path | None = None,
    frames_of: Callable[[str], int] = number_of_frames,
) -> list[Stack]:
    """The stacks of a manifest. ``source_root`` stands in for ``/source``
    (tests, or a runner that mounts elsewhere): mount ``/source/0`` is then
    ``<source_root>/0``."""
    if not isinstance(doc, dict) or not isinstance(doc.get("stacks"), list):
        raise ManifestError("stacks.json holds no list of stacks")
    mounts: dict[int, Path] = {}
    for s in doc.get("sources") or []:
        m = Path(s["mount"])
        if source_root is not None:
            m = Path(source_root) / m.name
        mounts[int(s["id"])] = m
    out: list[Stack] = []
    seen: set[int] = set()
    for s in doc["stacks"]:
        try:
            sid = int(s["stack_id"])
        except (KeyError, TypeError, ValueError) as e:
            raise ManifestError("a stack without an integer stack_id") from e
        if sid in seen:
            raise ManifestError(f"stack {sid} is listed twice")
        seen.add(sid)
        files = s.get("files") or []
        slices: list[Slice] = []
        for f in files:
            src = int(f.get("source", 0))
            if src not in mounts:
                raise ManifestError(f"stack {sid}: a file names source {src}, which the manifest does not mount")
            path = _safe_join(mounts[src], f["path"])
            frames = f.get("frames")
            if frames is None:
                n = frames_of(path) if len(files) == 1 else 1
                slices.extend(Slice(path, i) for i in range(n))
            else:
                slices.extend(Slice(path, i) for i in parse_frames(str(frames)))
        out.append(
            Stack(
                stack_id=sid,
                slices=slices,
                unit=s.get("unit") or f"stack-{sid}",
                orientation=s.get("orientation"),
                body_part=s.get("body_part") or None,
                technique=s.get("technique"),
                extra={k: v for k, v in s.items() if k not in KNOWN},
            )
        )
    return out


def load(path: Path, *, source_root: Path | None = None) -> list[Stack]:
    try:
        doc = json.loads(Path(path).read_text(encoding="utf-8"))
    except json.JSONDecodeError as e:
        raise ManifestError(f"stacks.json is not JSON: {e}") from e
    return parse(doc, source_root=source_root)

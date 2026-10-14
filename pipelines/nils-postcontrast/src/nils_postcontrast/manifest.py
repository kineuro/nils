# SPDX-License-Identifier: AGPL-3.0-only
"""The ``stacks`` input layout (``contracts/job/v1``, ``stacks.schema.json``)
as this image reads it: ``/input/stacks.json`` with ``x-nils.input.header``,
``x-nils.input.geometry`` and ``x-nils.input.contrast``.

Per stack: its files with each file's geometry, the stack's geometry (its
orientation, slice thickness, study and whether it is sealed now), its
header (fingerprint, classification per axis, first ingest batch, the
subject's open cohorts, the datasets holding its files) and its contrast
facts (the contrast tag, the clock, the series number, the physics, whether
it is derived, which post_contrast rules fired). ``context`` lists the other
MR stacks of each session the run holds part of, with the same header and
contrast facts, never to be scored: model r1's session evidence and its
time trust are made over the whole session.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path


class ManifestError(ValueError):
    pass


@dataclass
class Entry:
    stack_id: int
    unit: str | None
    series_id: int | None
    modality: str | None
    geometry: dict
    header: dict
    contrast: dict | None
    # (absolute path, geometry dict or None, frames text or None)
    files: list = field(default_factory=list)
    scored: bool = True

    @property
    def study(self):
        return self.geometry.get("study")

    @property
    def sealed(self) -> bool:
        return self.geometry.get("sealed") is True


def _mounts(doc: dict, source_root: Path | None) -> dict:
    out = {}
    for s in doc.get("sources") or []:
        m = Path(s["mount"])
        if source_root is not None:
            m = Path(source_root) / m.name
        out[int(s["id"])] = m
    return out


def _join(mount: Path, rel: str) -> str:
    p = Path(rel)
    if p.is_absolute() or ".." in p.parts:
        raise ManifestError("a file path is absolute or leaves its source mount")
    return str(mount / p)


def _entry(s: dict, mounts: dict | None, scored: bool) -> Entry:
    try:
        sid = int(s["stack_id"])
    except (KeyError, TypeError, ValueError) as e:
        raise ManifestError("a stack without a stack_id") from e
    files = []
    if mounts is not None:
        for f in s.get("files") or []:
            try:
                files.append((_join(mounts[int(f["source"])], f["path"]), f.get("geometry"), f.get("frames")))
            except (KeyError, TypeError, ValueError) as e:
                raise ManifestError("a file names no known source") from e
    return Entry(sid, s.get("unit"), s.get("series_id"), s.get("modality"), s.get("geometry") or {}, s.get("header") or {},
                 s.get("contrast"), files, scored)


def read(path: Path, source_root: Path | None = None) -> tuple[list, list]:
    """The stacks to score and the session context, as Entries."""
    try:
        doc = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as e:
        raise ManifestError("the manifest is not readable JSON") from e
    mounts = _mounts(doc, source_root)
    stacks = [_entry(s, mounts, True) for s in doc.get("stacks") or []]
    context = [_entry(c, None, False) for c in doc.get("context") or []]
    return stacks, context


def ordered_files(e: Entry) -> list:
    """The stack's files as the study's registry query ordered them (by
    instance number, then path), with their geometry in the slices
    module's tuple: (path, ipp, ps, rows, cols, inum, nframes)."""
    out = []
    for path, g, _frames in e.files:
        g = g or {}
        out.append((path, g.get("image_position_patient"), g.get("pixel_spacing"), g.get("rows"), g.get("columns"), g.get("instance_number"),
                    g.get("number_of_frames") or 1))
    out.sort(key=lambda t: (t[5] if t[5] is not None else 0, t[0]))
    return out


def multi_frame(e: Entry) -> bool:
    """Whether the stack holds an enhanced multi-frame file: a file whose
    frames are named, or that holds more than one."""
    for _p, g, frames in e.files:
        if frames is not None:
            return True
        if isinstance(g, dict) and (g.get("number_of_frames") or 1) > 1:
            return True
    return False

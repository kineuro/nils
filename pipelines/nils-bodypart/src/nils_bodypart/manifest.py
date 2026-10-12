# SPDX-License-Identifier: AGPL-3.0-only
"""The ``stacks`` input layout: ``/input/stacks.json`` (``contracts/job/v1``,
``stacks.schema.json``).

    {"contract": "job/v1",
     "sources": [{"id": 0, "mount": "/source/0"}],
     "stacks": [
       {"unit": "stack-12", "stack_id": 12,
        "files": [{"source": 0, "path": "a/1.dcm", "frames": null},
                  {"source": 0, "path": "a/mf.dcm", "frames": "1-40"}],
        "orientation": "axial", "body_part": null, "technique": "MPRAGE",
        "slices": 41}]}

A stack's files are in its slice order, and a slice index counts their
frames in that order from 0, as v0 counted them. ``frames`` names the
frames of a multi-frame file that are the stack's, from one, as ranges
(``1-4,9``); null is every frame. A stack of one file whose frames are null
has the runner's ``slices`` count, or is asked its NumberOfFrames where the
manifest gives none; a stack of many such files is one frame per file,
which is what a classic series is, since reading every header of an
archive to count frames would cost more than the embedding.

The runner also gives, since the job contract's rulings of record 43,
``orientation`` (which slices inference reads; none means the centre three),
the stack's ``body_part`` and ``technique`` as the registry holds them now
(the seeder's pools and strata; none means every stack is in the null pool)
and ``slices``, how many slices its files hold. A descriptor that opts in
with ``x-nils.input.header`` (record 50) has each stack carry ``header``:
the stack's fingerprint, its classification per axis, its first ingest
batch and its subject's open cohorts, which stay in ``extra``. With
``x-nils.input.geometry`` the manifest also lists, under ``context``, the
other MR stacks of each session the run holds part of (record 55 E2, P1b),
never to be scored (``context``).
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
    # The files as the manifest lists them, each with the frames that are
    # the stack's (from 0), or None for every frame the file holds.
    files: list[tuple[str, list[int] | None]] = field(default_factory=list)
    # Each file's geometry as the registry holds it, where the manifest
    # gives it (``geometry`` on a file, which the engine writes when the
    # descriptor sets ``x-nils.input.geometry``), in the reader's names
    # (``ipp``, ``ps``, ``rows``, ``cols``, ``inum``, ``frames``), else
    # None; the reduced reader chooses frames from it without reading
    # headers. The stack's own is ``extra["geometry"]`` (``iop``,
    # ``thick``, ``modality``, and since P1b ``study``, ``image_type``,
    # ``sop_class``, ``n_instances`` and ``sealed`` where the engine gives
    # them).
    file_geo: list[dict | None] = field(default_factory=list)

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


def _safe_join(mount: Path, rel: str, base: str | None = None) -> str:
    """``rel`` under ``mount``, as ``str(mount / Path(rel))`` writes it.
    With ``base`` (``_base(mount)``, made once a source) a relative path
    that ``Path`` writes as it is (no empty, ``.`` or ``..`` part) is joined
    as text: an archive's manifest names hundreds of thousands of files."""
    if base is not None and type(rel) is str and rel and rel[0] != "/":
        parts = rel.split("/")
        if "" not in parts and "." not in parts and ".." not in parts:
            return base + rel
    p = Path(rel)
    if p.is_absolute() or ".." in p.parts:
        raise ManifestError("a file path is absolute or leaves its source mount")
    return str(mount / p)


def _base(mount: Path) -> str:
    """What ``str(mount / rel)`` puts before a plain relative ``rel``."""
    m = str(mount)
    return "" if m == "." else m if m.endswith("/") else m + "/"


KNOWN = {"unit", "stack_id", "files", "orientation", "body_part", "technique", "slices"}

# A file's geometry as the engine names it (stacks.schema.json, record 55
# E2), and the reader's name for each.
_FILE_GEOMETRY = {
    "image_position_patient": "ipp",
    "pixel_spacing": "ps",
    "rows": "rows",
    "columns": "cols",
    "instance_number": "inum",
    "number_of_frames": "frames",
}


# The stack's facts beside its geometry (P1b), and the reader's names.
_STACK_FACTS = {
    "study": "study",
    "image_type": "image_type",
    "sop_class_uid": "sop_class",
    "n_instances": "n_instances",
    "sealed": "sealed",
}


def file_geometry(g) -> dict | None:
    """A file's geometry in the reader's names: the engine's geometry
    object mapped, or one already in the reader's names (as the one-hour
    study's manifests wrote it) as it is; None without one."""
    if not isinstance(g, dict):
        return None
    if "image_position_patient" in g:
        return {short: g.get(name) for name, short in _FILE_GEOMETRY.items()}
    return g


def stack_geometry(s: dict) -> dict | None:
    """A stack's geometry in the reader's names (``iop``, ``thick``,
    ``modality``): the engine's (the stack's orientation and its
    fingerprint's slice thickness, beside the entry's modality, and the
    stack's facts of P1b) mapped, or one already in the reader's names as it
    is; None without one."""
    g = s.get("geometry")
    if not isinstance(g, dict):
        return None
    if "image_orientation_patient" in g:
        out = {"iop": g.get("image_orientation_patient"), "thick": g.get("slice_thickness"), "modality": s.get("modality")}
        # the stack's facts the few-slice student reads beside its geometry
        # (P1b): its session's id to group by, what its images are, how many,
        # and whether it is sealed now
        for name, short in _STACK_FACTS.items():
            if name in g:
                out[short] = g[name]
        return out
    return g


def _mounts(doc: dict, source_root: Path | None) -> dict[int, Path]:
    mounts: dict[int, Path] = {}
    for s in doc.get("sources") or []:
        m = Path(s["mount"])
        if source_root is not None:
            m = Path(source_root) / m.name
        mounts[int(s["id"])] = m
    return mounts


def _stack(s: dict, mounts: dict[int, Path], bases: dict[int, str], frames_of: Callable[[str], int]) -> Stack:
    """One entry of ``stacks`` (``parse``'s, and ``stream``'s one at a time)."""
    try:
        sid = int(s["stack_id"])
    except (KeyError, TypeError, ValueError) as e:
        raise ManifestError("a stack without an integer stack_id") from e
    files = s.get("files") or []
    counted = s.get("slices")
    counted = counted if isinstance(counted, int) and not isinstance(counted, bool) and counted > 0 else None
    slices: list[Slice] = []
    listed: list[tuple[str, list[int] | None]] = []
    geos: list[dict | None] = []
    for f in files:
        src = int(f.get("source", 0))
        if src not in mounts:
            raise ManifestError(f"stack {sid}: a file names source {src}, which the manifest does not mount")
        path = _safe_join(mounts[src], f["path"], bases[src])
        frames = f.get("frames")
        geos.append(file_geometry(f.get("geometry")))
        if frames is None:
            n = (counted or frames_of(path)) if len(files) == 1 else 1
            slices.extend(Slice(path, i) for i in range(n))
            listed.append((path, None))
        else:
            chosen = parse_frames(str(frames))
            slices.extend(Slice(path, i) for i in chosen)
            listed.append((path, chosen))
    extra = {k: v for k, v in s.items() if k not in KNOWN}
    sg = stack_geometry(s)
    if sg is not None:
        extra["geometry"] = sg
    return Stack(
        stack_id=sid,
        slices=slices,
        unit=s.get("unit") or f"stack-{sid}",
        orientation=s.get("orientation"),
        body_part=s.get("body_part") or None,
        technique=s.get("technique"),
        extra=extra,
        files=listed,
        file_geo=geos,
    )


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
    mounts = _mounts(doc, source_root)
    bases = {k: _base(m) for k, m in mounts.items()}
    out: list[Stack] = []
    seen: set[int] = set()
    for s in doc["stacks"]:
        st = _stack(s, mounts, bases, frames_of)
        if st.stack_id in seen:
            raise ManifestError(f"stack {st.stack_id} is listed twice")
        seen.add(st.stack_id)
        out.append(st)
    return out


def _context_stack(s: dict) -> Stack:
    files = [f for f in s.get("files") or [] if isinstance(f, dict)]
    extra = {"modality": s.get("modality")}
    sg = stack_geometry(s)
    if sg is not None:
        extra["geometry"] = sg
    return Stack(stack_id=int(s["stack_id"]), slices=[], unit=f"stack-{int(s['stack_id'])}", extra=extra, files=[("", None) for _ in files],
                 file_geo=[file_geometry(f.get("geometry")) for f in files])


def _context_id(s) -> int:
    try:
        return int(s["stack_id"])
    except (KeyError, TypeError, ValueError) as e:
        raise ManifestError("a context stack without an integer stack_id") from e


def context(doc: dict, *, skip=()) -> list[Stack]:
    """The manifest's context stacks (``context``, record 55 E2, P1b): the
    other stacks of each session the run holds part of, which the engine
    lists with their geometry and their files' geometry and no path, so a
    stack is read against its whole session in any run. They are never
    scored; a stack the run itself holds (``skip``) is left out."""
    out: list[Stack] = []
    seen = set(skip)
    for s in doc.get("context") or [] if isinstance(doc, dict) else []:
        sid = _context_id(s)
        if sid in seen:
            continue
        seen.add(sid)
        out.append(_context_stack(s))
    return out


class _Text:
    """A JSON text read a piece at a time: one value at a time decoded by
    the json module's own scanner from the piece in hand."""

    _WS = re.compile(r"[ \t\n\r]*")

    def __init__(self, f, piece: int) -> None:
        import codecs

        self.f, self.piece = f, piece
        self.utf8 = codecs.getincrementaldecoder("utf-8")()
        self.buf, self.pos, self.eof = "", 0, False
        self.decoder = json.JSONDecoder()

    def _more(self) -> bool:
        if self.eof:
            return False
        data = self.f.read(self.piece)
        try:
            text = self.utf8.decode(data, final=not data)
        except UnicodeDecodeError as e:
            raise ManifestError(f"stacks.json is not JSON: {e}") from e
        self.eof = not data
        self.buf, self.pos = self.buf[self.pos :] + text, 0
        return True

    def peek(self) -> str:
        """The next character that is not white space, '' at the end."""
        while True:
            self.pos = self._WS.match(self.buf, self.pos).end()
            if self.pos < len(self.buf) or not self._more():
                return self.buf[self.pos] if self.pos < len(self.buf) else ""

    def take(self, ch: str) -> None:
        if self.peek() != ch:
            raise ManifestError(f"stacks.json is not JSON: {ch!r} expected")
        self.pos += 1

    def value(self):
        self.peek()
        while True:
            try:
                v, end = self.decoder.raw_decode(self.buf, self.pos)
            except json.JSONDecodeError as e:
                if self._more():
                    continue
                raise ManifestError(f"stacks.json is not JSON: {e}") from e
            if end == len(self.buf) and self._more():
                continue  # a number may go on in the next piece
            self.pos = end
            return v


def stream(path: Path, *, source_root: Path | None = None, frames_of: Callable[[str], int] = number_of_frames, piece: int = 1 << 22):
    """The run's stacks, then its context stacks, one at a time as the
    file is read: ``("stack", Stack)`` in the order of ``parse`` and
    ``("context", Stack)`` in the order of ``context(doc, skip=<the run's
    stacks>)``, the same stacks, without holding the file or every entry at
    once (an archive's part of 20,000 stacks is most of a gigabyte). The
    engine writes ``contract``, ``sources``, ``stacks`` and ``context`` in
    that order; a manifest in another order is read whole, as ``read``,
    ``parse`` and ``context`` read it."""
    with open(path, "rb") as f:
        t = _Text(f, piece)
        if t.peek() != "{":
            yield from _whole(path, source_root, frames_of)
            return
        t.take("{")
        head: dict = {}
        mounts = bases = None
        seen: set[int] = set()
        state = "head"  # then "stacks" once they are read, "context" once that is
        first = True
        while True:
            if t.peek() == "}":
                t.pos += 1
                break
            if not first:
                t.take(",")
            first = False
            key = t.value()
            if not isinstance(key, str):
                raise ManifestError("stacks.json is not JSON: a member without a name")
            t.take(":")
            if key == "stacks" and state == "head":
                if not isinstance(head.get("sources"), list) or t.peek() != "[":
                    yield from _whole(path, source_root, frames_of)
                    return
                mounts = _mounts(head, source_root)
                bases = {k: _base(m) for k, m in mounts.items()}
                state = "stacks"
                for s in _items(t):
                    st = _stack(s, mounts, bases, frames_of)
                    if st.stack_id in seen:
                        raise ManifestError(f"stack {st.stack_id} is listed twice")
                    seen.add(st.stack_id)
                    yield "stack", st
            elif key == "context" and state == "stacks":
                state = "context"
                if t.peek() != "[":
                    v = t.value()
                    if v:
                        raise ManifestError("stacks.json's context is not a list")
                    continue
                for s in _items(t):
                    sid = _context_id(s)
                    if sid in seen:
                        continue
                    seen.add(sid)
                    yield "context", _context_stack(s)
            elif state == "head" and key != "context":
                head[key] = t.value()
            elif key in ("stacks", "context", "sources"):
                if state == "head":  # the context before the stacks
                    yield from _whole(path, source_root, frames_of)
                    return
                raise ManifestError(f"stacks.json names its {key} twice, or after the stacks")
            else:
                t.value()
        if t.peek() != "":
            raise ManifestError("stacks.json is not JSON: more after its object")
        if state == "head":
            raise ManifestError("stacks.json holds no list of stacks")


def _items(t: _Text):
    t.take("[")
    if t.peek() == "]":
        t.pos += 1
        return
    while True:
        yield t.value()
        if t.peek() == "]":
            t.pos += 1
            return
        t.take(",")


def _whole(path: Path, source_root, frames_of):
    doc = read(path)
    stacks = parse(doc, source_root=source_root, frames_of=frames_of)
    for st in stacks:
        yield "stack", st
    for c in context(doc, skip={st.stack_id for st in stacks}):
        yield "context", c


def read(path: Path) -> dict:
    try:
        return json.loads(Path(path).read_text(encoding="utf-8"))
    except json.JSONDecodeError as e:
        raise ManifestError(f"stacks.json is not JSON: {e}") from e


def load(path: Path, *, source_root: Path | None = None) -> list[Stack]:
    return parse(read(path), source_root=source_root)

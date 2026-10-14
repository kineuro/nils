# SPDX-License-Identifier: AGPL-3.0-only
"""The slice cache's writes (record 55 E5 and E2, the plan's A17b): the
small pictures each model reads of a scan, kept per session so a later
model or calibration scores them again in minutes.

- **Where:** ``<cache>/<input version>/<session>.npz``, one file per
  session and input version, under the folder a run is given read-write
  (``--cache``; the engine mounts its working place's ``slice-cache/``
  there for a run that declares the cache, which A17b adds).
- **Input versions:** ``bodypart-slice-192:v1`` (the student's slot-0
  slice), ``bodypart-planes-b24:v1`` and ``bodypart-planes-touched:v1`` (the
  certified encoder's six planes, as the 24-frame and the touched readers
  build them). A new version never matches an old entry.
- **Key:** each entry's key is the sha256 of its input version, the
  identity of the files it came from (each file's SOP Instance UID, the
  sha256 of its bytes and the frames of it that are the stack's, empty
  for every frame), sorted, and a digest of the frames the input was made
  of (each file's SOP Instance UID and frame from zero, sorted), so the
  stacks one enhanced file holds are entries of their own. Never a
  registry id, so an entry survives the production refresh. The session's file is named by a hash of its
  StudyInstanceUID, never the UID itself.
- **What an entry holds:** the input (the tile, or the six planes with the
  15 geometry numbers, the encoder's six probabilities, the 44 header
  features and the cohort) and its index record (the version, the files'
  identities, the image's version and the parts that read it).

- **Reading (A17b, the re-score):** a run given the cache looks there
  before it reads a file. The engine's manifest names each file's SOP
  Instance UID and, where a run learnt it, the sha256 of its bytes. The
  student's slice is found by its exact key. A planes entry is found by
  its stack's key (``stack_key``: the input version, the stack's
  orientation and geometry, and each of its files' UID, frames and
  geometry as the manifest lists them, which is all the reader chooses
  frames from), written only where every file the reads opened was read
  whole, and it is used only where each of its files' sha256 equals the
  manifest's. So a hit is the input the files would give, and a miss
  reads the files as before.

Only the run's main process writes, a session at a time, under a lock on
the session's file, so runs side by side merge their entries: an entry of
the same key is replaced. A file is written whole, synced to the disk and
renamed into place. The cache is written after the run's answers and on a
best-effort basis: a session's file that cannot be read (empty, cut short,
not an archive) is replaced by the run's entries, and a session that cannot
be written (a full disk) is counted and left as it was, never failing the
run. Nothing here logs a path or a UID.
"""

from __future__ import annotations

import fcntl
import hashlib
import io
import json
import os
from collections import defaultdict
from pathlib import Path

import numpy as np

SLICE = "bodypart-slice-192:v1"
PLANES = {"b24": "bodypart-planes-b24:v1", "touched": "bodypart-planes-touched:v1"}
VERSIONS = (SLICE, *PLANES.values())


def stack_key(version: str, st) -> str | None:
    """A planes entry's key by its stack (A17b): the input version, the
    stack's orientation and geometry, and each file's SOP Instance UID,
    frames and geometry in the manifest's order, which is everything the
    reader chooses the frames from; None where a file has no UID."""
    idents = getattr(st, "file_ident", None) or []
    if not st.files or len(idents) != len(st.files) or any(i is None for i in idents) or not manifest_alone(st):
        return None
    header = st.extra.get("header") if isinstance(st.extra.get("header"), dict) else {}
    fp = header.get("fingerprint") if isinstance(header.get("fingerprint"), dict) else {}
    sg = st.extra.get("geometry") if isinstance(st.extra.get("geometry"), dict) else {}
    geo = getattr(st, "file_geo", None) or [None] * len(st.files)
    doc = {
        "version": version,
        "orientation": fp.get("orientation"),
        "stack": {k: sg.get(k) for k in ("iop", "thick", "modality")},
        "files": [[i["sop_instance_uid"], frames_text(which), g] for (_, which), i, g in zip(st.files, idents, geo)],
    }
    return hashlib.sha256(json.dumps(doc, sort_keys=True, default=_plain).encode()).hexdigest()


def manifest_alone(st) -> bool:
    """Whether the reduced reader chooses the stack's frames from the
    manifest alone, reading no header first: every file one frame with
    the registry's geometry (``reduced.files_to_read``'s test). Only such a
    stack's planes are found by its key, since the frames read are then a
    matter of the manifest and the bytes of the files read."""
    from . import reduced

    sg = st.extra.get("geometry") if isinstance(st.extra.get("geometry"), dict) else {}
    geo = getattr(st, "file_geo", None) or []
    if not st.files or len(geo) != len(st.files):
        return False
    return all(w is None and reduced.registry_frame(p, g, sg) is not None for (p, w), g in zip(st.files, geo))


def _plain(x):
    """A number numpy made, as JSON writes a Python one."""
    if isinstance(x, np.generic):
        return x.item()
    if isinstance(x, np.ndarray):
        return x.tolist()
    raise TypeError(type(x).__name__)


def plain(doc):
    """A document with numpy's numbers made Python's, so JSON writes it."""
    return json.loads(json.dumps(doc, default=_plain))


def vol_of(planes: np.ndarray) -> np.ndarray:
    """The volume the encoder reads, rebuilt from its six planes (the
    speed study's ``planes_bp.vol_of``): the encoder reads only these
    planes of it (``tiny.planes``), so its answers from this volume are its
    answers from the whole one."""
    p = np.asarray(planes, np.uint8)
    v = np.zeros((64, 64, 64), np.uint8)
    v[31], v[32] = p[0], p[1]
    v[:, 31], v[:, 32] = p[2], p[3]
    v[:, :, 31], v[:, :, 32] = p[4], p[5]
    return v


def entry_key(version: str, files: list[tuple], kept: str | None = None) -> str:
    lines = "\n".join(" ".join(str(x) for x in f) for f in sorted(files))
    text = f"{version}\n{lines}"
    if kept:
        text += f"\nkept {kept}"
    return hashlib.sha256(text.encode()).hexdigest()


def frames_text(which) -> str:
    """The frames of a file that are the stack's, as a key writes them:
    empty for every frame, else each from zero, comma separated."""
    return "" if which is None else ",".join(str(int(i)) for i in which)


def kept_digest(kept, idents: dict | None) -> str | None:
    """A digest of the frames an input was made of: each file's SOP
    Instance UID and frame from zero, sorted; None where a file's identity
    is not known."""
    rows = []
    for path, frame in kept or []:
        uid = ((idents or {}).get(path) or {}).get("sop_instance_uid")
        if not uid:
            return None
        rows.append(f"{uid} {int(frame)}")
    return hashlib.sha256("\n".join(sorted(rows)).encode()).hexdigest() if rows else None


def session_name(study_uid: str) -> str:
    return hashlib.sha256(f"nils-slice-cache-session:{study_uid}".encode()).hexdigest()[:32]


def identities(paths, idents: dict | None, frames_of: dict | None = None) -> list[tuple[str, str, str]] | None:
    """The (SOP Instance UID, sha256, the stack's frames of it) of each
    path, or None when one of them was not read whole (a card decodes
    without the bytes); ``frames_of`` maps a path to the stack's frames
    from zero (None for every frame), as the manifest lists them."""
    out = []
    for p in paths:
        i = (idents or {}).get(p)
        if not i or not i.get("sop_instance_uid") or not i.get("sha256"):
            return None
        out.append((i["sop_instance_uid"], i["sha256"], frames_text((frames_of or {}).get(p))))
    return out


def study_of(paths, idents: dict | None) -> str | None:
    uids = {((idents or {}).get(p) or {}).get("study_instance_uid") for p in paths}
    uids.discard(None)
    uids.discard("")
    return uids.pop() if len(uids) == 1 else None


class Reader:
    """The cache's entries as a re-score reads them (A17b): a session's
    file loaded once, and each hit and miss counted by input version."""

    def __init__(self, root: Path):
        self.root = Path(root)
        self.sessions: dict[tuple[str, str], tuple[dict, dict]] = {}
        self.counts = {"hits": {}, "misses": {}, "no_identity": 0, "unreadable_sessions": 0}

    def _session(self, version: str, study_uid: str) -> tuple[dict, dict]:
        k = (version, study_uid)
        if k not in self.sessions:
            try:
                self.sessions[k] = read(self.root, version, study_uid)
            except Exception:  # noqa: BLE001 - a damaged file is a miss, never a failed run
                self.counts["unreadable_sessions"] += 1
                self.sessions[k] = ({}, {})
        return self.sessions[k]

    def _count(self, kind: str, version: str) -> None:
        c = self.counts[kind]
        c[version] = c.get(version, 0) + 1

    @staticmethod
    def study(st) -> str | None:
        i = st.extra.get("identity")
        uid = i.get("study_instance_uid") if isinstance(i, dict) else None
        return uid if isinstance(uid, str) and uid else None

    def _arrays(self, arrays: dict, key: str, names) -> dict | None:
        out = {}
        for n in names:
            a = arrays.get(f"{key}.{n}")
            if a is None:
                return None
            out[n] = a
        return out

    def tile(self, st, index: int):
        """The student's slice of the stack's file ``index`` (its slot-0
        file), as ``fewslice.read_tile`` gives it, ``(tile, identity)``;
        None where the cache holds none or the file's content is not
        known."""
        study = self.study(st)
        idents = getattr(st, "file_ident", None) or []
        i = idents[index] if index < len(idents) else None
        if study is None or i is None or not i.get("sha256"):
            self.counts["no_identity"] += 1
            return None
        path, which = st.files[index]
        files = [(i["sop_instance_uid"], i["sha256"], frames_text(which))]
        kept = kept_digest([(path, 0)], {path: i})
        index_doc, arrays = self._session(SLICE, study)
        key = entry_key(SLICE, files, kept)
        got = self._arrays(arrays, key, ("tile",)) if key in index_doc else None
        if got is None:
            self._count("misses", SLICE)
            return None
        self._count("hits", SLICE)
        return got["tile"], {"sop_instance_uid": i["sop_instance_uid"], "sha256": i["sha256"], "study_instance_uid": study}

    def planes(self, st, version: str) -> tuple[dict, dict] | None:
        """A planes entry of the stack, ``(arrays, record)``, where its
        stack's key matches and each of its files' sha256 is the
        manifest's; None otherwise."""
        study = self.study(st)
        key = stack_key(version, st)
        if study is None or key is None:
            self.counts["no_identity"] += 1
            return None
        index_doc, arrays = self._session(version, study)
        known = {}
        for (path, which), i in zip(st.files, st.file_ident):
            if i and i.get("sha256"):
                known[(i["sop_instance_uid"], frames_text(which))] = i["sha256"]
        for k, rec in index_doc.items():
            if rec.get("stack_key") != key:
                continue
            if all(known.get((f[0], f[2])) == f[1] for f in rec.get("files") or []) and rec.get("files"):
                got = self._arrays(arrays, k, ("planes", "image", "header44"))
                if got is not None and isinstance(rec.get("meta"), dict):
                    self._count("hits", version)
                    return got, rec
        self._count("misses", version)
        return None


def _load(path: Path) -> tuple[dict, dict[str, np.ndarray]]:
    if not path.is_file():
        return {}, {}
    with np.load(path, allow_pickle=False) as z:
        index = json.loads(str(z["index"])) if "index" in z.files else {}
        arrays = {k: z[k] for k in z.files if k != "index"}
    return index, arrays


def read(root: Path, version: str, study_uid: str) -> tuple[dict, dict[str, np.ndarray]]:
    """A session's entries of one input version: the index by key, and the arrays named ``<key>.<name>``."""
    return _load(Path(root) / version / f"{session_name(study_uid)}.npz")


class Writer:
    """Entries held per session until the run's stacks of that session are
    done (``done``), then merged into the session's file."""

    def __init__(self, root: Path, made_by: dict):
        self.root = Path(root)
        self.made_by = made_by
        self.held: dict[tuple[str, str], dict[str, tuple[dict, dict]]] = defaultdict(dict)
        self.counts = {"entries": 0, "sessions_written": 0, "skipped_no_identity": 0, "sessions_damaged_replaced": 0, "sessions_failed": 0}

    def add(self, version: str, study_uid: str | None, files: list[tuple] | None, arrays: dict, meta: dict | None = None,
            kept: str | None = None) -> bool:
        if not study_uid or not files or not kept:
            self.counts["skipped_no_identity"] += 1
            return False
        key = entry_key(version, files, kept)
        rec = {"version": version, "files": [list(f) for f in sorted(files)], "kept": kept, "made_by": self.made_by, **(meta or {})}
        self.held[(version, study_uid)][key] = (rec, {k: np.asarray(v) for k, v in arrays.items()})
        self.counts["entries"] += 1
        return True

    def flush(self, study_uids=None) -> None:
        """Write the held sessions (those of ``study_uids``, or all), each on
        its own: one that fails is counted, and the others are written."""
        for version, uid in list(self.held):
            if study_uids is not None and uid not in study_uids:
                continue
            entries = self.held.pop((version, uid))
            try:
                self._merge(version, uid, entries)
            except Exception as e:  # noqa: BLE001 - best-effort: counted by kind, never a path
                self.counts["sessions_failed"] += 1
                kinds = self.counts.setdefault("failures", {})
                kinds[type(e).__name__] = kinds.get(type(e).__name__, 0) + 1

    def _merge(self, version: str, uid: str, entries: dict) -> None:
        folder = self.root / version
        folder.mkdir(parents=True, exist_ok=True)
        path = folder / f"{session_name(uid)}.npz"
        with open(folder / f".{path.stem}.lock", "a") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            tmp = path.with_name(path.name + ".partial")
            try:
                try:
                    index, arrays = _load(path)
                except Exception:  # noqa: BLE001 - a damaged file is replaced by what this run holds
                    index, arrays = {}, {}
                    self.counts["sessions_damaged_replaced"] += 1
                for key, (rec, arrs) in entries.items():
                    for name in [n for n in arrays if n.startswith(f"{key}.")]:
                        del arrays[name]
                    index[key] = dict(rec, arrays=sorted(arrs))
                    for name, a in arrs.items():
                        arrays[f"{key}.{name}"] = a
                buf = io.BytesIO()
                np.savez_compressed(buf, index=np.array(json.dumps(index, sort_keys=True)), **arrays)
                fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
                with os.fdopen(fd, "wb") as f:
                    f.write(buf.getvalue())
                    f.flush()
                    os.fsync(f.fileno())
                os.replace(tmp, path)
                self.counts["sessions_written"] += 1
            except BaseException:
                tmp.unlink(missing_ok=True)
                raise
            finally:
                fcntl.flock(lock, fcntl.LOCK_UN)

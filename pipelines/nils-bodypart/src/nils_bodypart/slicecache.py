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
- **Key:** each entry's key is the sha256 of its input version and the
  identity of the files it came from, each file's SOP Instance UID and the
  sha256 of its bytes, sorted. Never a registry id, so an entry survives
  the production refresh. The session's file is named by a hash of its
  StudyInstanceUID, never the UID itself.
- **What an entry holds:** the input (the tile, or the six planes with the
  15 geometry numbers, the encoder's six probabilities, the 44 header
  features and the cohort) and its index record (the version, the files'
  identities, the image's version and the parts that read it).

Only the run's main process writes, a session at a time, under a lock on
the session's file, so runs side by side merge their entries: an entry of
the same key is replaced. A file is written whole and renamed into place.
Nothing here logs a path or a UID.
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


def entry_key(version: str, files: list[tuple[str, str]]) -> str:
    lines = "\n".join(f"{uid} {sha}" for uid, sha in sorted(files))
    return hashlib.sha256(f"{version}\n{lines}".encode()).hexdigest()


def session_name(study_uid: str) -> str:
    return hashlib.sha256(f"nils-slice-cache-session:{study_uid}".encode()).hexdigest()[:32]


def identities(paths, idents: dict | None) -> list[tuple[str, str]] | None:
    """The (SOP Instance UID, sha256) of each path, or None when one of them
    was not read whole (a card decodes without the bytes)."""
    out = []
    for p in paths:
        i = (idents or {}).get(p)
        if not i or not i.get("sop_instance_uid") or not i.get("sha256"):
            return None
        out.append((i["sop_instance_uid"], i["sha256"]))
    return out


def study_of(paths, idents: dict | None) -> str | None:
    uids = {((idents or {}).get(p) or {}).get("study_instance_uid") for p in paths}
    uids.discard(None)
    uids.discard("")
    return uids.pop() if len(uids) == 1 else None


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
        self.counts = {"entries": 0, "sessions_written": 0, "skipped_no_identity": 0}

    def add(self, version: str, study_uid: str | None, files: list[tuple[str, str]] | None, arrays: dict, meta: dict | None = None) -> bool:
        if not study_uid or not files:
            self.counts["skipped_no_identity"] += 1
            return False
        key = entry_key(version, files)
        rec = {"version": version, "files": [list(f) for f in sorted(files)], "made_by": self.made_by, **(meta or {})}
        self.held[(version, study_uid)][key] = (rec, {k: np.asarray(v) for k, v in arrays.items()})
        self.counts["entries"] += 1
        return True

    def flush(self, study_uids=None) -> None:
        """Write the held sessions (those of ``study_uids``, or all)."""
        for version, uid in list(self.held):
            if study_uids is not None and uid not in study_uids:
                continue
            entries = self.held.pop((version, uid))
            self._merge(version, uid, entries)

    def _merge(self, version: str, uid: str, entries: dict) -> None:
        folder = self.root / version
        folder.mkdir(parents=True, exist_ok=True)
        path = folder / f"{session_name(uid)}.npz"
        with open(folder / f".{path.stem}.lock", "a") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            try:
                index, arrays = _load(path)
                for key, (rec, arrs) in entries.items():
                    for name in [n for n in arrays if n.startswith(f"{key}.")]:
                        del arrays[name]
                    index[key] = dict(rec, arrays=sorted(arrs))
                    for name, a in arrs.items():
                        arrays[f"{key}.{name}"] = a
                buf = io.BytesIO()
                np.savez_compressed(buf, index=np.array(json.dumps(index, sort_keys=True)), **arrays)
                tmp = path.with_name(path.name + ".partial")
                fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
                with os.fdopen(fd, "wb") as f:
                    f.write(buf.getvalue())
                os.replace(tmp, path)
                self.counts["sessions_written"] += 1
            finally:
                fcntl.flock(lock, fcntl.LOCK_UN)

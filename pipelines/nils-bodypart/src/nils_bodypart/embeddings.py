# SPDX-License-Identifier: AGPL-3.0-only
"""The embedding file: one per (stack, encoder), the engine's format.

The format is the engine's (record 43 S4, ``nils-registry``'s
``embedding.rs``), so a file this image writes is one the engine registers
as it is:

- 8 bytes ``NILSEMB1``;
- the header's length, a little-endian u32;
- the header, compact JSON: ``format`` ``nils-embedding``, ``stack_id``,
  ``encoder`` (the encoder's weight digest), ``preprocess_version``,
  ``rows``, ``dim``, ``dtype`` ``<f4`` and ``slices`` (one slice index per
  row);
- zero bytes up to the next multiple of 64 from the file's start;
- the matrix, little-endian float32, row after row, and nothing after it.

The header's keys are written sorted and without spaces, as the engine's
JSON writer writes them.
"""

from __future__ import annotations

import json
import os
import re
import struct
from dataclasses import dataclass
from pathlib import Path

import numpy as np

MAGIC = b"NILSEMB1"
ALIGN = 64
EXTENSION = ".emb"
MEDIA_TYPE = "application/vnd.nils.embedding"
_DIGEST = re.compile(r"^sha256:[0-9a-f]{64}$")


@dataclass
class Embedding:
    stack_id: int
    encoder: str
    preprocess_version: str
    slices: list[int]
    matrix: np.ndarray  # (len(slices), dim) float32

    @property
    def dim(self) -> int:
        return int(self.matrix.shape[1])

    def rows_by_slice(self) -> dict[int, np.ndarray]:
        return {s: self.matrix[i] for i, s in enumerate(self.slices)}


def _check(e: Embedding) -> None:
    if not _DIGEST.match(e.encoder):
        raise ValueError(f"the encoder {e.encoder} is not a digest")
    v = e.preprocess_version
    if not v or len(v) > 128 or any(c.isspace() or not c.isprintable() for c in v):
        raise ValueError("the preprocessing version is not a word")
    if e.matrix.ndim != 2 or e.matrix.shape[0] == 0 or e.matrix.shape[1] == 0:
        raise ValueError("an embedding holds at least one row of at least one value")
    if e.matrix.shape[0] != len(e.slices):
        raise ValueError("one row per slice")
    if len(set(e.slices)) != len(e.slices):
        raise ValueError("a slice has two rows")
    if not np.all(np.isfinite(e.matrix)):
        raise ValueError("a row holds a value that is not a number")


def encode(e: Embedding) -> bytes:
    _check(e)
    header = json.dumps(
        {
            "format": "nils-embedding",
            "stack_id": int(e.stack_id),
            "encoder": e.encoder,
            "preprocess_version": e.preprocess_version,
            "rows": len(e.slices),
            "dim": e.dim,
            "dtype": "<f4",
            "slices": [int(s) for s in e.slices],
        },
        sort_keys=True,
        separators=(",", ":"),
    ).encode()
    start = -(-(12 + len(header)) // ALIGN) * ALIGN
    out = bytearray(MAGIC)
    out += struct.pack("<I", len(header))
    out += header
    out += b"\0" * (start - len(out))
    out += np.ascontiguousarray(e.matrix, dtype="<f4").tobytes()
    return bytes(out)


def decode(data: bytes) -> Embedding:
    if len(data) < 12 or data[:8] != MAGIC:
        raise ValueError("not an embedding file: it does not start with NILSEMB1")
    (n,) = struct.unpack("<I", data[8:12])
    if 12 + n > len(data):
        raise ValueError("the header is longer than the file")
    h = json.loads(data[12 : 12 + n])
    if h.get("format") != "nils-embedding" or h.get("dtype") != "<f4":
        raise ValueError("the header does not say a little-endian float32 nils-embedding")
    slices = [int(s) for s in h["slices"]]
    if h.get("rows") != len(slices):
        raise ValueError("the header's rows and slices disagree")
    start = -(-(12 + n) // ALIGN) * ALIGN
    if any(data[12 + n : start]):
        raise ValueError("the padding after the header is not zero bytes")
    dim = int(h["dim"])
    body = data[start:]
    if len(body) != len(slices) * dim * 4:
        raise ValueError("the matrix is not rows times dim float32")
    m = np.frombuffer(body, dtype="<f4").reshape(len(slices), dim).astype(np.float32)
    e = Embedding(int(h["stack_id"]), h["encoder"], h["preprocess_version"], slices, m)
    _check(e)
    return e


def write(path: Path, e: Embedding) -> bytes:
    data = encode(e)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".partial")
    tmp.write_bytes(data)
    os.replace(tmp, path)
    return data


class Store:
    """Every embedding under some directories, by (encoder, preprocessing
    version, stack). A file that is not an embedding is passed over; so is
    one that fails to decode, counted in ``unreadable``."""

    def __init__(self) -> None:
        self._by: dict[tuple[str, str, int], Embedding] = {}
        self.unreadable = 0

    @classmethod
    def scan(cls, *dirs: Path | None) -> "Store":
        s = cls()
        for d in dirs:
            if d is None or not Path(d).is_dir():
                continue
            for root, _, files in os.walk(d):
                for f in sorted(files):
                    p = Path(root) / f
                    try:
                        with open(p, "rb") as fh:
                            if fh.read(8) != MAGIC:
                                continue
                        s.add(decode(p.read_bytes()))
                    except (OSError, ValueError, KeyError, json.JSONDecodeError):
                        s.unreadable += 1
        return s

    def add(self, e: Embedding) -> None:
        key = (e.encoder, e.preprocess_version, int(e.stack_id))
        held = self._by.get(key)
        if held is not None:
            # Two files of one key: the union of their slices.
            rows = held.rows_by_slice()
            rows.update(e.rows_by_slice())
            slices = sorted(rows)
            e = Embedding(e.stack_id, e.encoder, e.preprocess_version, slices, np.stack([rows[i] for i in slices]))
        self._by[key] = e

    def get(self, encoder: str, preprocess_version: str, stack_id: int) -> Embedding | None:
        return self._by.get((encoder, preprocess_version, int(stack_id)))

    def __len__(self) -> int:
        return len(self._by)

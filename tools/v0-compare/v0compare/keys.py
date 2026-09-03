# SPDX-License-Identifier: AGPL-3.0-only
"""Subject codes: v0's `subject_code_gen` and v1's `blake2b-8` are the same
function, a keyed BLAKE2b of the identifier's UTF-8 bytes with an 8-byte
digest, written as hex. The tool recomputes codes only to classify how a v0
subject got its code (§12.4); a key is read from a file and never printed,
an identifier never leaves the process."""

from __future__ import annotations

import hashlib
import stat
from pathlib import Path


def blake2b8(identifier: str, key: str) -> str:
    return hashlib.blake2b(identifier.encode("utf-8"), key=key.encode("utf-8"), digest_size=8).hexdigest()


def cohort_seed(cohort: str) -> str:
    """The seed v0 fell back to without a configured one: the cohort name,
    upper-cased (`extract/config.py`, `resolved_subject_code_seed`)."""
    base = cohort.strip()
    return base.upper() if base else "DEFAULT-SEED"


def read_key(path: Path) -> str:
    """The key in `path`, stripped of a trailing newline. The file should be
    readable by its owner only; anything wider is reported, not refused."""
    mode = stat.S_IMODE(path.stat().st_mode)
    if mode & 0o077:
        import sys

        print(f"warning: {path} is readable by others (mode {mode:o})", file=sys.stderr)
    key = path.read_text(encoding="utf-8").rstrip("\r\n")
    if not key:
        raise ValueError(f"{path}: empty key")
    return key


def classify_code(code: str, identifiers: list[str], key: str | None, cohort: str | None) -> str:
    """How `code` relates to the subject's identifiers: `key-consistent` when
    it is the keyed hash of one of them, `cohort-hashed` when it is the hash
    under v0's fallback seed, `no identifier` when there is nothing to hash,
    else `other` (a CSV-mapped code, or an identifier v0 overwrote)."""
    if not identifiers:
        return "no identifier"
    if key is not None and any(blake2b8(i, key) == code for i in identifiers):
        return "key-consistent"
    if cohort is not None:
        seed = cohort_seed(cohort)
        if any(blake2b8(i, seed) == code for i in identifiers):
            return "cohort-hashed"
    return "other"

# SPDX-License-Identifier: AGPL-3.0-only
"""Fixtures: a synthetic corpus written by `nils-dicom`'s `corpus` example,
digested by the `nils` binary into a SQLite registry under a throwaway
key. Both binaries come from `NILS_BIN` and `NILS_CORPUS_BIN`, else the
engine's release target directory; the end-to-end tests skip when they are
not built."""

from __future__ import annotations

import json
import os
import re
import secrets
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

_SHOW_LINE = re.compile(r"^\s+patient-id\s+(\S+)\s+\(identity")


@dataclass
class Synth:
    root: Path
    registry: Path
    #: the subject-code key, as `nils key add` read it
    key: str
    manifest: dict
    #: v1 subject code -> PatientID (None for a subject identified by its study)
    patient_ids: dict[str, str | None] = field(default_factory=dict)
    nils: Path = Path("nils")

    def run(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [str(self.nils), *args, "--registry", str(self.registry)],
            check=True,
            capture_output=True,
            text=True,
        )


def _binary(env: str, default: Path) -> Path:
    path = Path(os.environ.get(env) or default)
    if not path.is_file():
        pytest.skip(f"{path} is not built (set {env} or build the engine in release mode)")
    return path


@pytest.fixture(scope="session")
def nils_bin() -> Path:
    return _binary("NILS_BIN", REPO / "engine" / "target" / "release" / "nils")


@pytest.fixture(scope="session")
def corpus_bin() -> Path:
    return _binary("NILS_CORPUS_BIN", REPO / "engine" / "target" / "release" / "examples" / "corpus")


def make_synth(base: Path, nils: Path, corpus: Path, *, backend: str = "sqlite", dsn: str | None = None,
               schema: str | None = None, instances: int = 6000, seed: int = 7) -> Synth:
    root = base / "corpus"
    manifest = json.loads(
        subprocess.run(
            [
                str(corpus),
                "--out",
                str(root),
                "--instances",
                str(instances),
                "--seed",
                str(seed),
                "--same-day-percent",
                "60",
                "--refused-every",
                "100",
            ],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    )
    registry = base / "reg"
    registry.mkdir()
    key = secrets.token_hex(24)
    key_file = base / "reg.key"
    key_file.write_text(key + "\n", encoding="utf-8")
    key_file.chmod(0o600)
    synth = Synth(root=root, registry=registry, key=key, manifest=manifest, nils=nils)
    synth.run("key", "add", "test", "--from-file", str(key_file))
    key_file.unlink()
    init = ["init", "--backend", backend, "--scheme", "blake2b-8", "--key", "test"]
    if dsn:
        init += ["--dsn", dsn]
    if schema:
        init += ["--schema", schema]
    synth.run(*init)
    synth.run("digest", str(root), "--files", "dcm,no-ext")
    return synth


def harvest_patient_ids(synth: Synth, codes: list[str]) -> None:
    """`nils linkage show` per code: the PatientID the digest filed, if any."""
    for code in codes:
        out = synth.run("linkage", "show", code, "--why", "v0-compare test fixture").stdout
        found = None
        for line in out.splitlines():
            m = _SHOW_LINE.match(line)
            if m:
                found = m.group(1)
                break
        synth.patient_ids[code] = found


@pytest.fixture(scope="session")
def synth(tmp_path_factory: pytest.TempPathFactory, nils_bin: Path, corpus_bin: Path) -> Synth:
    return make_synth(tmp_path_factory.mktemp("synth"), nils_bin, corpus_bin)

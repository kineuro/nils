# SPDX-License-Identifier: AGPL-3.0-only
"""A System 1 bundle: the one file a site's model is kept as.

A bundle is a tar archive holding ``system1.json`` and the files it names::

    {
      "format": "nils-system1/1",
      "name": "s1c", "version": "2026-10-09",
      "axes": ["provenance", "technique", "modifier", "construct", "base"],
      "predictor": {"kind": "keywords", "file": "keywords.json"}
                 | {"kind": "module", "file": "predict.py", "entry": "load"},
      "files": {"keywords.json": "sha256:...", ...}
    }

The archive's own sha256 is the model's digest on its card, so the engine
gives the run the file it registered. Every member is checked against the
digest ``system1.json`` names before anything is read, and a member the
manifest does not name, a link or a path that leaves the folder is refused.

A ``keywords`` predictor is data this image reads (``keywords.py``); a
``module`` predictor is code the site's model brings, a Python file whose
``entry`` function takes the bundle's folder and answers an object with
``axes`` and ``predict(records, rows)``. Code runs only when the run says
it trusts it (``--trust-code true``), and only after its bytes match.
"""

from __future__ import annotations

import hashlib
import importlib.util
import json
import tarfile
from pathlib import Path

FORMAT = "nils-system1/1"
MANIFEST = "system1.json"


class BundleError(ValueError):
    pass


def sha256_file(p: Path) -> str:
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return "sha256:" + h.hexdigest()


def _safe(name: str) -> bool:
    p = Path(name)
    return not p.is_absolute() and ".." not in p.parts and name not in ("", ".")


def open_bundle(archive: Path, into: Path) -> dict:
    """Check and unpack a bundle into an empty folder; answer its manifest."""
    archive, into = Path(archive), Path(into)
    into.mkdir(parents=True, exist_ok=True)
    try:
        tf = tarfile.open(archive, "r:*")
    except (tarfile.TarError, OSError) as e:
        raise BundleError(f"the model input is not a System 1 bundle: {e}") from e
    with tf:
        members = tf.getmembers()
        names = {m.name.lstrip("./") for m in members if m.isfile()}
        if MANIFEST not in names:
            raise BundleError(f"the bundle holds no {MANIFEST}")
        for m in members:
            if m.issym() or m.islnk() or m.isdev():
                raise BundleError("the bundle holds a link or a device, which a bundle never does")
            if not _safe(m.name.lstrip("./") or "."):
                if m.isdir():
                    continue
                raise BundleError("the bundle holds a path that leaves its folder")
        manifest = json.loads(tf.extractfile(next(m for m in members if m.isfile() and m.name.lstrip("./") == MANIFEST)).read())
        if manifest.get("format") != FORMAT:
            raise BundleError(f"system1.json says format {manifest.get('format')!r}, not {FORMAT}")
        files = manifest.get("files") or {}
        unknown = sorted(names - set(files) - {MANIFEST})
        if unknown:
            raise BundleError(f"the bundle holds {len(unknown)} file(s) system1.json does not name")
        missing = sorted(set(files) - names)
        if missing:
            raise BundleError(f"system1.json names {len(missing)} file(s) the bundle does not hold")
        for m in members:
            name = m.name.lstrip("./")
            if not m.isfile():
                continue
            target = into / name
            target.parent.mkdir(parents=True, exist_ok=True)
            with tf.extractfile(m) as src, open(target, "wb") as dst:
                while True:
                    b = src.read(1 << 20)
                    if not b:
                        break
                    dst.write(b)
    for name, digest in files.items():
        got = sha256_file(into / name)
        if got != digest:
            raise BundleError(f"{name} is {got}, and system1.json names {digest}")
    axes = manifest.get("axes")
    if not isinstance(axes, list) or not axes or not all(isinstance(a, str) for a in axes):
        raise BundleError("system1.json names the axes it answers, a list")
    return manifest


def load_predictor(folder: Path, manifest: dict, trust_code: bool):
    """The predictor a bundle names."""
    p = manifest.get("predictor") or {}
    kind = p.get("kind")
    if kind == "keywords":
        from .keywords import Keywords

        return Keywords(json.loads((folder / p["file"]).read_text()), manifest["axes"])
    if kind == "module":
        if not trust_code:
            raise BundleError(
                "the bundle's predictor is code the site's model brings; the run must say it trusts it (--trust-code true), "
                "and it runs only after its bytes match the digest system1.json names"
            )
        path = folder / p["file"]
        spec = importlib.util.spec_from_file_location("nils_system1_site_predictor", path)
        if spec is None or spec.loader is None:
            raise BundleError(f"{p['file']} does not load as a Python module")
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        entry = getattr(mod, p.get("entry", "load"), None)
        if entry is None:
            raise BundleError(f"{p['file']} has no {p.get('entry', 'load')} function")
        return entry(folder, manifest)
    raise BundleError(f"system1.json names a predictor of kind {kind!r}; keywords and module are known")


def make_bundle(out: Path, folder: Path, manifest: dict) -> str:
    """Write a bundle from a folder of files and a manifest (its ``files``
    filled with each file's sha256); answer the archive's digest. For the
    tests and for a site that builds its own."""
    out, folder = Path(out), Path(folder)
    files = {}
    for p in sorted(folder.rglob("*")):
        if p.is_file() and p.name != MANIFEST:
            files[str(p.relative_to(folder))] = sha256_file(p)
    manifest = {**manifest, "format": FORMAT, "files": files}
    (folder / MANIFEST).write_text(json.dumps(manifest, indent=1, sort_keys=True) + "\n")
    with tarfile.open(out, "w", format=tarfile.PAX_FORMAT) as tf:
        for name in [MANIFEST, *sorted(files)]:
            info = tf.gettarinfo(str(folder / name), arcname=name)
            info.mtime = 0
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            info.mode = 0o644
            with open(folder / name, "rb") as f:
                tf.addfile(info, f)
    return sha256_file(out)

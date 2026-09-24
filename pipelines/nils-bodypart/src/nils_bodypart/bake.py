# SPDX-License-Identifier: AGPL-3.0-only
"""Bake the pinned weights into the image, and prove they load offline.

    python -m nils_bodypart.bake download   # at build time, with the network
    python -m nils_bodypart.bake verify     # after it, with HF_HUB_OFFLINE=1

``download`` fetches each repository of ``encoders.PINS`` at its pinned
commit into the Hugging Face cache (``HF_HOME``) and points the cache's
``main`` at that commit, so the loaders v0 used (``hf-hub:`` for open_clip,
``from_pretrained`` for transformers, and the tokenizer open_clip builds from
a companion repository) resolve to the pin without the network. It then
writes ``encoders.json``: per encoder its name, a version naming the commit,
the sha256 of its weights file (its identity as a registered encoder), and
its width.

``verify`` loads both encoders with the network off, embeds one blank image
and one prompt, and checks each width against ``encoders.json``.
"""

from __future__ import annotations

import hashlib
import json
import os
import sys
from pathlib import Path

from .encoders import ENCODERS_JSON, PINS, Encoders


def _sha256_file(p: Path) -> str:
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return "sha256:" + h.hexdigest()


def _pin_main(repo: str, revision: str) -> None:
    from huggingface_hub.constants import HF_HUB_CACHE

    refs = Path(HF_HUB_CACHE) / ("models--" + repo.replace("/", "--")) / "refs"
    refs.mkdir(parents=True, exist_ok=True)
    (refs / "main").write_text(revision)


def download() -> None:
    from huggingface_hub import snapshot_download

    out: dict[str, dict] = {}
    for name, pin in PINS.items():
        path = Path(snapshot_download(pin["repo"], revision=pin["revision"]))
        _pin_main(pin["repo"], pin["revision"])
        for c in pin["companions"]:
            snapshot_download(c["repo"], revision=c["revision"], allow_patterns=c["allow"])
            _pin_main(c["repo"], c["revision"])
        out[name] = {
            "name": name,
            "version": f"{pin['repo'].split('/')[-1]}@{pin['revision'][:12]}",
            "digest": _sha256_file(path / pin["weights"]),
            "dim": pin["dim"],
            "repo": pin["repo"],
            "revision": pin["revision"],
        }
    ENCODERS_JSON.parent.mkdir(parents=True, exist_ok=True)
    ENCODERS_JSON.write_text(json.dumps(out, indent=2, sort_keys=True) + "\n")
    print(json.dumps(out, indent=2, sort_keys=True))


def verify() -> None:
    import numpy as np

    if os.environ.get("HF_HUB_OFFLINE") != "1":
        sys.exit("verify runs with HF_HUB_OFFLINE=1, or it proves nothing")
    enc = Encoders(device="cpu")
    blank = np.zeros((224, 224, 3), dtype=np.uint8)
    for name, info in enc.infos().items():
        e = enc.get(name)
        v = e.encode_images([blank])
        if v.shape != (1, info.dim):
            sys.exit(f"{name}: an image embeds to {v.shape}, and encoders.json says {info.dim}")
        if e.has_text:
            t = e.encode_texts(["axial MRI scan of the brain"])
            if t.shape != (1, info.dim):
                sys.exit(f"{name}: a prompt embeds to {t.shape}")
        print(f"{name}: {info.version} {info.digest} dim {info.dim}, loaded offline")


if __name__ == "__main__":
    {"download": download, "verify": verify}[sys.argv[1]]()

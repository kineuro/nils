# SPDX-License-Identifier: AGPL-3.0-only
"""The two frozen encoders, pinned, and a stand-in for tests.

- **BiomedCLIP** (``microsoft/BiomedCLIP-PubMedBERT_256-vit_base_patch16_224``),
  image and text, 512-d, through open_clip, as v0 loaded it;
- **SigLIP2** (``google/siglip2-base-patch16-224``), image only, 768-d,
  through transformers, as v0 loaded it.

Every Hugging Face repository is pinned to a commit in ``PINS``. The image
downloads exactly those commits at build time (``bake.py``), points each
repository's ``main`` at its pin in the baked cache, and runs with
``HF_HUB_OFFLINE=1``, so a load at run time reads the pinned files and never
the network. An encoder's identity is the sha256 of its weights file, which
``bake.py`` writes into ``encoders.json`` beside the weights.

The stand-in (``--standin``, or ``NILS_BODYPART_STANDIN=1``) replaces both
with a fixed random projection of the prepared image, so the entry points
run in tests without torch or weights. Its digests say it is a stand-in and
never collide with a real encoder's.
"""

from __future__ import annotations

import hashlib
import json
import logging
import os
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

import numpy as np

logger = logging.getLogger(__name__)

# The pinned commits. A change here is a new encoder: every embedding of the
# old one stays under its own key, and every head that read it goes stale.
PINS: dict[str, dict] = {
    "biomedclip": {
        "repo": "microsoft/BiomedCLIP-PubMedBERT_256-vit_base_patch16_224",
        "revision": "9f341de24bfb00180f1b847274256e9b65a3a32e",
        "weights": "open_clip_pytorch_model.bin",
        # open_clip builds the text tower and its tokenizer from this
        # repository's config and vocabulary; its weights come from the
        # BiomedCLIP checkpoint.
        "companions": [
            {
                "repo": "microsoft/BiomedNLP-BiomedBERT-base-uncased-abstract",
                "revision": "d673b8835373c6fa116d6d8006b33d48734e305d",
                "allow": ["config.json", "tokenizer_config.json", "vocab.txt", "special_tokens_map.json"],
            }
        ],
        "dim": 512,
    },
    "siglip2": {
        "repo": "google/siglip2-base-patch16-224",
        "revision": "75de2d55ec2d0b4efc50b3e9ad70dba96a7b2fa2",
        "weights": "model.safetensors",
        "companions": [],
        "dim": 768,
    },
}

DEFAULT_CHAIN = ("biomedclip", "siglip2")
ENCODERS_JSON = Path(os.environ.get("NILS_BODYPART_HOME", "/opt/nils-bodypart")) / "encoders.json"


@dataclass(frozen=True)
class EncoderInfo:
    name: str
    version: str
    digest: str
    dim: int
    repo: str | None = None
    revision: str | None = None

    def as_json(self) -> dict:
        return {
            "name": self.name,
            "version": self.version,
            "digest": self.digest,
            "dim": self.dim,
            "repo": self.repo,
            "revision": self.revision,
        }


def select_device(forced: str | None = None) -> str:
    """cuda, mps or cpu: what is asked, else the best there is."""
    forced = (forced or os.environ.get("NILS_BODYPART_DEVICE") or "auto").strip().lower()
    if forced in ("cuda", "cpu", "mps"):
        return forced
    try:
        import torch
    except Exception:
        return "cpu"
    if torch.cuda.is_available():
        return "cuda"
    if hasattr(torch.backends, "mps") and torch.backends.mps.is_available():
        return "mps"
    return "cpu"


def device_name(device: str) -> str:
    if device != "cuda":
        return device
    try:
        import torch

        return f"cuda:{torch.cuda.get_device_name(0)}"
    except Exception:
        return "cuda"


class BaseEncoder:
    info: EncoderInfo
    has_text = False

    def encode_images(self, images: Sequence[np.ndarray]) -> np.ndarray:
        raise NotImplementedError

    def encode_texts(self, prompts: Sequence[str]) -> np.ndarray:
        raise NotImplementedError(f"{self.info.name} has no text tower in use")


def baked_infos() -> dict[str, EncoderInfo]:
    """What ``bake.py`` recorded of the weights in the image."""
    doc = json.loads(ENCODERS_JSON.read_text(encoding="utf-8"))
    return {name: EncoderInfo(**{k: v for k, v in e.items() if k in EncoderInfo.__dataclass_fields__}) for name, e in doc.items()}


class BiomedCLIPEncoder(BaseEncoder):
    has_text = True

    def __init__(self, info: EncoderInfo, device: str) -> None:
        import open_clip
        import torch

        self.device = device
        hub = f"hf-hub:{PINS['biomedclip']['repo']}"
        model, _, preprocess = open_clip.create_model_and_transforms(hub)
        self.tokenizer = open_clip.get_tokenizer(hub)
        self.model = model.to(device).eval()
        self.preprocess = preprocess
        self.torch = torch
        self.info = info

    def encode_images(self, images: Sequence[np.ndarray]) -> np.ndarray:
        from PIL import Image

        torch = self.torch
        if not images:
            return np.zeros((0, self.info.dim), dtype=np.float32)
        batch = torch.stack([self.preprocess(Image.fromarray(a)) for a in images]).to(self.device)
        with torch.no_grad():
            f = self.model.encode_image(batch)
            f = f / f.norm(dim=-1, keepdim=True).clamp(min=1e-12)
        return f.detach().cpu().to(torch.float32).numpy()

    def encode_texts(self, prompts: Sequence[str]) -> np.ndarray:
        torch = self.torch
        toks = self.tokenizer(list(prompts)).to(self.device)
        with torch.no_grad():
            f = self.model.encode_text(toks)
            f = f / f.norm(dim=-1, keepdim=True).clamp(min=1e-12)
        return f.detach().cpu().to(torch.float32).numpy()


class SigLIP2Encoder(BaseEncoder):
    def __init__(self, info: EncoderInfo, device: str) -> None:
        import torch
        import transformers

        pin = PINS["siglip2"]
        self.device = device
        self.model = transformers.AutoModel.from_pretrained(pin["repo"], revision=pin["revision"]).to(device).eval()
        # Only the image processor, and the slow one, which is what v0's
        # AutoProcessor gave it; the text tower is not used.
        self.processor = transformers.AutoImageProcessor.from_pretrained(pin["repo"], revision=pin["revision"], use_fast=False)
        self.torch = torch
        self.info = info

    @staticmethod
    def _pooled(out):
        if hasattr(out, "shape"):
            return out
        pooled = getattr(out, "pooler_output", None)
        if pooled is not None:
            return pooled
        return out.last_hidden_state[:, 0, :]

    def encode_images(self, images: Sequence[np.ndarray]) -> np.ndarray:
        from PIL import Image

        torch = self.torch
        if not images:
            return np.zeros((0, self.info.dim), dtype=np.float32)
        inputs = self.processor(images=[Image.fromarray(a) for a in images], return_tensors="pt").to(self.device)
        with torch.no_grad():
            f = self._pooled(self.model.get_image_features(**inputs))
            f = f / f.norm(dim=-1, keepdim=True).clamp(min=1e-12)
        return f.detach().cpu().to(torch.float32).numpy()


class StandinEncoder(BaseEncoder):
    """A fixed random projection of the image, 16 x 16 block means, for tests."""

    has_text = True

    def __init__(self, name: str, dim: int) -> None:
        digest = "sha256:" + hashlib.sha256(f"nils-bodypart standin {name} {dim}".encode()).hexdigest()
        self.info = EncoderInfo(name=name, version="standin", digest=digest, dim=dim)
        rng = np.random.default_rng(int.from_bytes(hashlib.sha256(name.encode()).digest()[:4], "little"))
        self.proj = rng.standard_normal((256, dim)).astype(np.float32)
        self.has_text = name == "biomedclip"

    def encode_images(self, images: Sequence[np.ndarray]) -> np.ndarray:
        rows = []
        for a in images:
            g = a[..., 0].astype(np.float32) / 255.0
            h, w = g.shape
            g = g[: h - h % 16, : w - w % 16].reshape(16, h // 16, 16, w // 16).mean(axis=(1, 3))
            rows.append((g.reshape(-1) - 0.5) @ self.proj)
        if not rows:
            return np.zeros((0, self.info.dim), dtype=np.float32)
        x = np.stack(rows).astype(np.float32)
        return x / np.linalg.norm(x, axis=1, keepdims=True).clip(min=1e-12)

    def encode_texts(self, prompts: Sequence[str]) -> np.ndarray:
        out = []
        for p in prompts:
            r = np.random.default_rng(int.from_bytes(hashlib.sha256(p.encode()).digest()[:4], "little"))
            v = r.standard_normal(self.info.dim).astype(np.float32)
            out.append(v / np.linalg.norm(v))
        return np.stack(out) if out else np.zeros((0, self.info.dim), dtype=np.float32)


class Encoders:
    """The encoders of a run, loaded when first asked for."""

    def __init__(self, *, standin: bool = False, device: str | None = None) -> None:
        self.standin = standin or os.environ.get("NILS_BODYPART_STANDIN") == "1"
        self.device = "cpu" if self.standin else select_device(device)
        self._loaded: dict[str, BaseEncoder] = {}
        self._infos: dict[str, EncoderInfo] | None = None

    def infos(self) -> dict[str, EncoderInfo]:
        if self._infos is None:
            if self.standin:
                self._infos = {n: StandinEncoder(n, PINS[n]["dim"]).info for n in PINS}
            else:
                self._infos = baked_infos()
        return self._infos

    def info(self, name: str) -> EncoderInfo:
        try:
            return self.infos()[name]
        except KeyError:
            raise ValueError(f"unknown encoder {name!r}") from None

    def get(self, name: str) -> BaseEncoder:
        if name not in self._loaded:
            if self.standin:
                self._loaded[name] = StandinEncoder(name, PINS[name]["dim"])
            elif name == "biomedclip":
                self._loaded[name] = BiomedCLIPEncoder(self.info(name), self.device)
            elif name == "siglip2":
                self._loaded[name] = SigLIP2Encoder(self.info(name), self.device)
            else:
                raise ValueError(f"unknown encoder {name!r}")
        return self._loaded[name]

    def device_label(self) -> str:
        return "cpu" if self.standin else device_name(self.device)

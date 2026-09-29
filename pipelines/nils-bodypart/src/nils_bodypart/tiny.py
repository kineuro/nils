# SPDX-License-Identifier: AGPL-3.0-only
"""The certified body-part encoder (record 50), in numpy: no torch.

The artifact is one safetensors file holding three seeds of a small 2D
network, each seed's tensors under ``s0.``, ``s1.``, ``s2.`` with the names
a torch state dict gives them, and in its metadata ``arch`` (``tiny2d``),
``classes`` (six, in order), ``seeds`` (per seed the geometry's mean and
standard deviation and a temperature) and ``preprocessing``.

A seed reads the volume's three mid-planes (axial, coronal, sagittal, each
the mean of the two centre planes) and the fifteen geometry numbers
(``log1p`` of the ten extents and counts, the normal, the obliquity over 90
and CT), normalised by the seed's mean and deviation. The network is four
blocks of a 3x3 convolution without bias, batch normalisation (eval), ReLU
and 2x2 max pooling (3, 16, 32, 48 and 64 channels), a global mean, and a
head of Linear(64 + 15, 64), ReLU and Linear(64, 6). Each seed's logits are
softmaxed at its temperature, and the three are averaged. Everything is
float32, as torch computes it on the CPU.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

import numpy as np

ARCH = "tiny2d"
CHANNELS = (3, 16, 32, 48, 64)
NGEO = 15
EPS = 1e-5


class EncoderError(ValueError):
    pass


def planes(v: np.ndarray) -> np.ndarray:
    """v (64, 64, 64) in [0, 1], vol[z, y, x] -> (3, 64, 64): the axial,
    coronal and sagittal mid-planes."""
    return np.stack([0.5 * (v[31] + v[32]), 0.5 * (v[:, 31] + v[:, 32]), 0.5 * (v[:, :, 31] + v[:, :, 32])]).astype(np.float32)


def geo_feats(g: np.ndarray) -> np.ndarray:
    """The fifteen raw geometry numbers -> the fifteen the network reads."""
    g = np.asarray(g, np.float32)
    return np.concatenate([np.log1p(np.maximum(g[:10], np.float32(0))), g[10:13], g[13:14] / np.float32(90.0), g[14:15]]).astype(np.float32)


def _conv3x3(x: np.ndarray, w: np.ndarray) -> np.ndarray:
    """x (C, H, W), w (O, C, 3, 3), padding 1, no bias."""
    c, h, wd = x.shape
    p = np.zeros((c, h + 2, wd + 2), np.float32)
    p[:, 1:-1, 1:-1] = x
    cols = np.empty((c, 3, 3, h, wd), np.float32)
    for dy in range(3):
        for dx in range(3):
            cols[:, dy, dx] = p[:, dy : dy + h, dx : dx + wd]
    out = w.reshape(w.shape[0], -1).astype(np.float32) @ cols.reshape(c * 9, h * wd)
    return out.reshape(w.shape[0], h, wd)


def _maxpool2(x: np.ndarray) -> np.ndarray:
    c, h, w = x.shape
    h2, w2 = h // 2, w // 2
    return x[:, : h2 * 2, : w2 * 2].reshape(c, h2, 2, w2, 2).max(axis=(2, 4))


@dataclass
class Seed:
    tensors: dict[str, np.ndarray]
    geo_mean: np.ndarray
    geo_sd: np.ndarray
    temperature: float

    def logits(self, x: np.ndarray, g: np.ndarray) -> np.ndarray:
        t = self.tensors
        z = x.astype(np.float32)
        for b in range(4):
            z = _conv3x3(z, t[f"f.{b}.0.weight"])
            scale = t[f"f.{b}.1.weight"] / np.sqrt(t[f"f.{b}.1.running_var"] + np.float32(EPS))
            z = (z - t[f"f.{b}.1.running_mean"][:, None, None]) * scale[:, None, None] + t[f"f.{b}.1.bias"][:, None, None]
            z = _maxpool2(np.maximum(z, np.float32(0)))
        feat = z.mean(axis=(1, 2), dtype=np.float32)
        G = (geo_feats(g) - self.geo_mean) / self.geo_sd
        h = np.concatenate([feat, G.astype(np.float32)])
        h = np.maximum(t["h.head.0.weight"] @ h + t["h.head.0.bias"], np.float32(0))
        return (t["h.head.3.weight"] @ h + t["h.head.3.bias"]).astype(np.float32)


def softmax_T(logits: np.ndarray, T: float) -> np.ndarray:
    z = logits / T
    z = z - z.max(-1, keepdims=True)
    e = np.exp(z)
    return e / e.sum(-1, keepdims=True)


class Encoder:
    """The three seeds of a ``tiny2d`` artifact."""

    def __init__(self, path: Path | str) -> None:
        from safetensors import safe_open
        from safetensors.numpy import load_file

        with safe_open(str(path), framework="np") as f:
            meta = f.metadata() or {}
        if meta.get("arch") != ARCH:
            raise EncoderError(f"the encoder's arch is {meta.get('arch')!r}, not {ARCH}")
        try:
            self.classes = list(json.loads(meta["classes"]))
            seeds = json.loads(meta["seeds"])
        except (KeyError, json.JSONDecodeError) as e:
            raise EncoderError("the encoder's metadata names no classes or seeds") from e
        self.preprocessing = meta.get("preprocessing")
        raw = load_file(str(path))
        self.seeds: list[Seed] = []
        for name in sorted(seeds):
            card = seeds[name]
            prefix = name + "."
            tensors = {k[len(prefix) :]: np.asarray(v, np.float32) for k, v in raw.items() if k.startswith(prefix) and not k.endswith("num_batches_tracked")}
            self._check(name, tensors)
            self.seeds.append(
                Seed(
                    tensors,
                    np.asarray(card["geo_mean"], np.float32),
                    np.asarray(card["geo_sd"], np.float32),
                    float(card["temperature"]),
                )
            )
        if not self.seeds:
            raise EncoderError("the encoder holds no seed")

    def _check(self, name: str, t: dict[str, np.ndarray]) -> None:
        want = {}
        for b in range(4):
            i, o = CHANNELS[b], CHANNELS[b + 1]
            want[f"f.{b}.0.weight"] = (o, i, 3, 3)
            for k in ("weight", "bias", "running_mean", "running_var"):
                want[f"f.{b}.1.{k}"] = (o,)
        want.update({"h.head.0.weight": (64, 64 + NGEO), "h.head.0.bias": (64,), "h.head.3.weight": (len(self.classes), 64), "h.head.3.bias": (len(self.classes),)})
        for k, shape in want.items():
            if k not in t or tuple(t[k].shape) != shape:
                raise EncoderError(f"seed {name} of the encoder lacks {k} of shape {shape}")

    def probabilities(self, vol: np.ndarray, geo: np.ndarray) -> np.ndarray:
        """The seeds' calibrated probabilities averaged, float32, for a
        uint8 volume (64, 64, 64) and the fifteen raw geometry numbers."""
        x = planes(vol.astype(np.float32) / np.float32(255.0))
        runs = [softmax_T(s.logits(x, geo)[None, :], s.temperature)[0] for s in self.seeds]
        return np.mean(runs, 0).astype(np.float32)

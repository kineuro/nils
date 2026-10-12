# SPDX-License-Identifier: AGPL-3.0-only
"""The few-slice student S1 (record 55 E2, P1b): nils-design study
``2026-10-05-image-model-speed``, ``tools/mimic_bp.py`` (``build(ntok=42,
detail=False)``, k = 1), frozen by amendment 5 as one safetensors file whose
tensors carry each seed's prefix (``s20261005.``, ...).

The network: an EfficientNet-B0 (timm, one input channel, no classifier)
over each native slice, the slice's four geometry tokens added to its
features, attention pooling over the slices (one here), the stack's 27
tokens and the encoder's 15 geometry numbers (``fewslice.geo_feats``) beside
it, and six outputs: the certified encoder's six probabilities, predicted.
Each seed answers; the cascade reads the seeds and their mean.

It runs in float32 with TF32 off, on the CPU or on a card; the research ran
it on a card in bf16, so the production numerics are a new candidate that
amendment 5's A5.2 grades. torch and timm are imported here only, so the
rest of the image is tested without them. The weights are a registered
model input, never baked into the image.
"""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np

NTOK = 42  # 27 stack tokens and 15 geometry numbers
OUTPUTS = 6


class StudentError(ValueError):
    pass


def build(ntok: int = NTOK):
    import timm
    import torch
    import torch.nn as nn

    class Mimic(nn.Module):
        def __init__(self):
            super().__init__()
            # pretrained=False: every weight comes from the frozen file
            self.enc = timm.create_model("efficientnet_b0.ra_in1k", pretrained=False, in_chans=1, num_classes=0)
            d = self.enc.num_features
            self.sg = nn.Sequential(nn.Linear(4, 64), nn.GELU(), nn.Linear(64, d))
            self.tk = nn.Sequential(nn.Linear(ntok, 128), nn.GELU())
            self.att = nn.Sequential(nn.Linear(d, 128), nn.Tanh(), nn.Linear(128, 1))
            self.head = nn.Sequential(nn.Linear(d + 128, 256), nn.GELU(), nn.Dropout(0.1), nn.Linear(256, OUTPUTS))

        def forward(self, x, sg, m, tok):
            B, k = x.shape[:2]
            f = self.enc(x.flatten(0, 1)).view(B, k, -1)
            f = f + self.sg(sg)
            a = self.att(f).squeeze(-1).float().masked_fill(~m, -1e4)
            w = torch.softmax(a, 1).to(f.dtype)
            return self.head(torch.cat([(w.unsqueeze(-1) * f).sum(1), self.tk(tok)], 1))

    return Mimic()


class Student:
    """The frozen seeds, each a network in eval mode."""

    def __init__(self, path: Path, device: str = "cpu", threads: int | None = None):
        import torch
        from safetensors import safe_open
        from safetensors.torch import load_file

        torch.backends.cuda.matmul.allow_tf32 = False
        torch.backends.cudnn.allow_tf32 = False
        if threads:
            torch.set_num_threads(max(1, int(threads)))
        try:
            with safe_open(str(path), "pt") as f:
                meta = f.metadata() or {}
            tensors = load_file(str(path))
        except Exception as e:  # noqa: BLE001
            raise StudentError("the student is not a safetensors file") from e
        seeds = [s for s in str(meta.get("seeds") or "").split(",") if s]
        if not seeds:
            seeds = sorted({k.split(".", 1)[0][1:] for k in tensors if k.startswith("s") and "." in k})
        if not seeds:
            raise StudentError("the student names no seeds")
        self.seeds = seeds
        self.device = device
        self.meta = {k: v for k, v in meta.items() if k in ("seeds", "network", "input")}
        self.nets = []
        for s in seeds:
            sd = {k[len(f"s{s}.") :]: v for k, v in tensors.items() if k.startswith(f"s{s}.")}
            net = build()
            try:
                net.load_state_dict(sd, strict=True)
            except RuntimeError as e:
                raise StudentError(f"seed {s} does not fit the student's network") from e
            self.nets.append(net.to(device).eval())

    def predict(self, tiles: np.ndarray, sgeo: np.ndarray, tokens: np.ndarray, batch: int = 64) -> np.ndarray:
        """(N, seeds, 6) float32 probabilities from N stacks' slot-0 tiles
        (uint8, 192 x 192), slice geometry (N, 4) and tokens (N, 42), each
        stack scored as ``mimic_bp.DS`` and ``run_predict`` scored it."""
        import torch

        n = len(tiles)
        out = np.full((n, len(self.nets), OUTPUTS), np.nan, np.float32)
        with torch.no_grad():
            for i in range(0, n, batch):
                x = torch.from_numpy(np.ascontiguousarray(tiles[i : i + batch], dtype=np.uint8)).float() / 255.0
                x = ((x - 0.5) / 0.25)[:, None, None]  # (B, k=1, 1, 192, 192)
                sg = torch.from_numpy(np.ascontiguousarray(sgeo[i : i + batch], dtype=np.float32))[:, None]
                m = torch.ones((x.shape[0], 1), dtype=torch.bool)
                tk = torch.from_numpy(np.ascontiguousarray(tokens[i : i + batch], dtype=np.float32))
                x, sg, m, tk = (t.to(self.device) for t in (x, sg, m, tk))
                for j, net in enumerate(self.nets):
                    lo = net(x, sg, m, tk)
                    out[i : i + batch, j] = torch.softmax(lo.float(), 1).cpu().numpy()
        return out


def describe(student: Student) -> str:
    return json.dumps({"seeds": student.seeds, **student.meta}, sort_keys=True)

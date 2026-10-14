# SPDX-License-Identifier: AGPL-3.0-only
"""The few-slice student (round 4's `fs3`): an EfficientNet-B0 over each of
three native slices (one input channel), the slice tokens added to each
slice's features, attention pooling over the valid slices, the stack tokens
through a 128-unit layer, and a head of 256 units with six outputs: model
r1's three image arms (T, U, V) as logits, then each arm's availability as a
logit. Three seeds, one file (``student.safetensors``, keys ``s0.``,
``s1.``, ``s2.``).

On a card the study ran it under bfloat16 autocast; on a CPU in float32.
The same is done here, so a card run gives the study's numbers."""

from __future__ import annotations

import numpy as np

from .slices import NSG, NTOK

SEEDS = 3


def net():
    import timm
    import torch
    import torch.nn as nn

    class FsNet(nn.Module):
        def __init__(self):
            super().__init__()
            self.enc = timm.create_model("efficientnet_b0.ra_in1k", pretrained=False, in_chans=1, num_classes=0)
            d = self.enc.num_features
            self.sg = nn.Sequential(nn.Linear(NSG, 64), nn.GELU(), nn.Linear(64, d))
            self.tk = nn.Sequential(nn.Linear(NTOK, 128), nn.GELU())
            self.att = nn.Sequential(nn.Linear(d, 128), nn.Tanh(), nn.Linear(128, 1))
            self.head = nn.Sequential(nn.Linear(d + 128, 256), nn.GELU(), nn.Dropout(0.1), nn.Linear(256, 6))

        def forward(self, x, sg, m, tok):
            B, k = x.shape[:2]
            f = self.enc(x.flatten(0, 1)).view(B, k, -1) + self.sg(sg)
            a = self.att(f).squeeze(-1).float().masked_fill(~m, -1e4)
            w = torch.softmax(a, 1).to(f.dtype)
            return self.head(torch.cat([(w.unsqueeze(-1) * f).sum(1), self.tk(tok)], 1))

    return FsNet()


class StudentError(ValueError):
    pass


class Student:
    """The three seeds, loaded on one device."""

    def __init__(self, path, device: str = "cpu", threads: int | None = None):
        import torch
        from safetensors.torch import load_file

        if threads:
            torch.set_num_threads(int(threads))
        sd = load_file(str(path))
        self.device = torch.device(device)
        self.nets = []
        for s in range(SEEDS):
            part = {k[len(f"s{s}."):]: v for k, v in sd.items() if k.startswith(f"s{s}.")}
            if not part:
                raise StudentError(f"the student file holds no seed {s}")
            m = net()
            m.load_state_dict(part, strict=True)
            m.to(self.device).eval()
            self.nets.append(m)

    def predict(self, tiles: np.ndarray, valid: np.ndarray, sg: np.ndarray, tok: np.ndarray) -> np.ndarray:
        """(B, seeds, 6) float32 outputs for a batch."""
        import torch

        dev = self.device
        with torch.no_grad(), torch.autocast(dev.type, dtype=torch.bfloat16, enabled=dev.type == "cuda"):
            x = torch.from_numpy(tiles.astype(np.float32)).unsqueeze(2).to(dev)
            m = torch.from_numpy(valid).to(dev)
            g = torch.from_numpy(sg.astype(np.float32)).to(dev)
            t = torch.from_numpy(tok.astype(np.float32)).to(dev)
            return np.stack([n(x, g, m, t).float().cpu().numpy() for n in self.nets], 1)

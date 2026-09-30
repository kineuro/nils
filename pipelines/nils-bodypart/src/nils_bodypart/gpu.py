# SPDX-License-Identifier: AGPL-3.0-only
"""The GPU path of the certified body-part model (record 50): the same
answers, sooner.

Two parts, each used only where a CUDA card is there and its libraries load,
and each falling back to the CPU path otherwise:

- :class:`J2KDecoder` decodes a stack's lossless JPEG 2000 frames in one
  batch on the card with nvImageCodec (nvJPEG2000). Lossless JPEG 2000 is
  reversible integer arithmetic, so any correct decoder gives the same
  samples. It is plugged into pydicom as a decoding plugin for that transfer
  syntax alone: pydicom still reads the file, finds the frames, and does
  everything after the codestream (sign, precision, photometric
  interpretation, rescale), so the only step that changes is the one that
  turns a codestream into samples. A frame the card cannot decode, or whose
  samples are not the shape and sign the codestream says, is decoded by
  pydicom's own plugin as before.
- :class:`TorchEncoder` runs the image encoder (``tiny.Encoder``) over a
  batch of stacks on the card, in float32 with TF32 off. It is not the
  numpy encoder bit for bit: the certified path keeps the numpy encoder
  unless a run asks for this one (``--encoder-device cuda``).
"""

from __future__ import annotations

import logging
import os
import threading

import numpy as np

logger = logging.getLogger("nils_bodypart")

PLUGIN = "nils-nvimgcodec"
J2K_LOSSLESS = "1.2.840.10008.1.2.4.90"

# The samples the card decoded for the stack now being read, by codestream.
# pydicom hands the plugin the codestream it found, and the plugin answers
# from here or decodes on the CPU. One stack at a time in a process.
_CACHE: dict[bytes, bytes] = {}
_STATS = {"gpu_frames": 0, "cpu_frames": 0}
_LOCK = threading.Lock()

# pydicom's plugin contract: which syntaxes, and what they need.
DECODER_DEPENDENCIES = {J2K_LOSSLESS: ("nvidia-nvimgcodec-cu12", "nvidia-nvjpeg2k-cu12")}


def is_available(uid: str) -> bool:
    return uid == J2K_LOSSLESS


def _decode_frame(src: bytes, runner) -> bytes | bytearray:
    """pydicom's decoding plugin: the card's samples of ``src``, with the
    options pydicom's own OpenJPEG plugin sets; or that plugin's answer."""
    from pydicom.pixels.common import PhotometricInterpretation as PI
    from pydicom.pixels.decoders import pylibjpeg

    got = _CACHE.get(bytes(src))
    if got is None:
        _STATS["cpu_frames"] += 1
        return pylibjpeg._decode_frame(src, runner)
    _STATS["gpu_frames"] += 1
    # as pydicom.pixels.decoders.pylibjpeg does for an OpenJPEG syntax
    if runner.photometric_interpretation in (PI.YBR_ICT, PI.YBR_RCT):
        runner.set_option("photometric_interpretation", PI.RGB)
    precision = runner.get_option("j2k_precision", runner.bits_stored)
    if 0 < precision <= 8:
        runner.set_option("bits_allocated", 8)
    elif 8 < precision <= 16:
        runner.set_option("bits_allocated", 16)
    elif 16 < precision <= 32:
        runner.set_option("bits_allocated", 32)
    return got


_REGISTERED = False


def _register() -> None:
    global _REGISTERED
    with _LOCK:
        if _REGISTERED:
            return
        from pydicom.pixels.decoders import JPEG2000LosslessDecoder

        if PLUGIN not in JPEG2000LosslessDecoder.available_plugins:
            JPEG2000LosslessDecoder.add_plugin(PLUGIN, (__name__, "_decode_frame"))
        _REGISTERED = True


def cuda_available() -> bool:
    """A CUDA card this process may use, without importing torch."""
    if os.environ.get("CUDA_VISIBLE_DEVICES", None) in ("", "-1", "none"):
        return False
    try:
        import ctypes

        lib = ctypes.CDLL("libcuda.so.1")
        n = ctypes.c_int(0)
        return lib.cuInit(0) == 0 and lib.cuDeviceGetCount(ctypes.byref(n)) == 0 and n.value > 0
    except OSError:
        return False


class J2KDecoder:
    """A stack's lossless JPEG 2000 frames decoded in one batch on the card.

    Called as ``volume.build``'s ``decode``: the frames each file must give
    in, per file its frames (or the exception that stopped it) out, as
    ``volume.decode_each`` gives them."""

    def __init__(self, device_id: int = 0) -> None:
        from nvidia import nvimgcodec

        self._nv = nvimgcodec
        # few CPU threads of its own: the run's parallelism is its worker
        # processes; a decoder that cannot be made so is made as the library
        # makes it by default
        try:
            self._dec = nvimgcodec.Decoder(device_id=device_id, max_num_cpu_threads=2)
        except Exception:  # noqa: BLE001
            self._dec = nvimgcodec.Decoder(device_id=device_id)
        self._params = nvimgcodec.DecodeParams(
            allow_any_depth=True, color_spec=nvimgcodec.ColorSpec.UNCHANGED, apply_exif_orientation=False
        )
        _register()

    @staticmethod
    def stats() -> dict[str, int]:
        return dict(_STATS)

    def _codestreams(self, ds, indices: list[int]) -> list[tuple[bytes, dict]]:
        """The codestreams of the frames asked for, with what their samples
        must be, when the file is lossless JPEG 2000 of one sample a pixel."""
        from pydicom.encaps import generate_frames
        from pydicom.pixels.utils import get_j2k_parameters

        if str(getattr(ds.file_meta, "TransferSyntaxUID", "")) != J2K_LOSSLESS:
            return []
        if int(getattr(ds, "SamplesPerPixel", 1) or 1) != 1:
            return []
        nf = int(getattr(ds, "NumberOfFrames", 1) or 1)
        want = set(indices) if nf > 1 else {0}
        rows, cols = int(ds.Rows), int(ds.Columns)
        out = []
        for i, frame in enumerate(generate_frames(ds.PixelData, number_of_frames=nf)):
            if i not in want:
                continue
            p = get_j2k_parameters(frame)
            prec = p.get("precision")
            if not prec or prec > 16:
                continue
            out.append((frame, {"shape": (rows, cols), "signed": bool(p.get("is_signed")), "bytes": 1 if prec <= 8 else 2}))
        return out

    def __call__(self, need: dict[str, list[int]]):
        import pydicom

        from . import volume

        read: dict = {}
        out: dict = {}
        streams: list[tuple[bytes, dict]] = []
        for path, idx in need.items():
            try:
                ds = pydicom.dcmread(path, force=True)
            except Exception as e:  # noqa: BLE001 - as decode_each
                out[path] = e
                continue
            read[path] = ds
            try:
                streams += self._codestreams(ds, idx)
            except Exception:  # noqa: BLE001 - pydicom decodes it on the CPU
                pass
        _CACHE.clear()
        if streams:
            try:
                imgs = self._dec.decode([self._nv.CodeStream(s) for s, _ in streams], params=self._params)
            except Exception:  # noqa: BLE001 - the whole batch goes to the CPU
                imgs = [None] * len(streams)
            for (src, want), img in zip(streams, imgs):
                if img is None:
                    continue
                a = np.asarray(img.cpu())
                if a.ndim == 3 and a.shape[-1] == 1:
                    a = a[..., 0]
                if a.shape != want["shape"] or a.dtype.itemsize != want["bytes"] or (a.dtype.kind == "i") != want["signed"]:
                    continue
                _CACHE[src] = np.ascontiguousarray(a).tobytes()
        for path, ds in read.items():
            try:
                out[path] = volume.decode_frames(path, need[path], ds=ds, plugin=PLUGIN)
            except Exception as e:  # noqa: BLE001 - as decode_each
                out[path] = e
        _CACHE.clear()
        return out


class TorchEncoder:
    """``tiny.Encoder``'s three seeds in torch on the card, over a batch."""

    def __init__(self, encoder, device: str = "cuda") -> None:
        import torch

        torch.backends.cuda.matmul.allow_tf32 = False
        torch.backends.cudnn.allow_tf32 = False
        torch.backends.cudnn.benchmark = False
        torch.backends.cudnn.deterministic = True
        self.torch, self.device = torch, torch.device(device)
        self.seeds = []
        for s in encoder.seeds:
            t = {k: torch.from_numpy(np.ascontiguousarray(v)).to(self.device) for k, v in s.tensors.items()}
            self.seeds.append((t, torch.from_numpy(s.geo_mean).to(self.device), torch.from_numpy(s.geo_sd).to(self.device), float(s.temperature)))

    def probabilities(self, vols: list[np.ndarray], geos: list[np.ndarray]) -> np.ndarray:
        """(B, 6) float32: the seeds' calibrated probabilities averaged."""
        from .tiny import EPS, geo_feats, planes

        torch, F = self.torch, self.torch.nn.functional
        x = torch.from_numpy(np.stack([planes(v.astype(np.float32) / np.float32(255.0)) for v in vols])).to(self.device)
        g = torch.from_numpy(np.stack([geo_feats(q) for q in geos])).to(self.device)
        runs = []
        with torch.no_grad():
            for t, gm, gs, T in self.seeds:
                z = x
                for b in range(4):
                    z = F.conv2d(z, t[f"f.{b}.0.weight"], padding=1)
                    scale = t[f"f.{b}.1.weight"] / torch.sqrt(t[f"f.{b}.1.running_var"] + EPS)
                    z = (z - t[f"f.{b}.1.running_mean"][None, :, None, None]) * scale[None, :, None, None] + t[f"f.{b}.1.bias"][None, :, None, None]
                    z = F.max_pool2d(torch.clamp_min(z, 0), 2)
                feat = z.mean(dim=(2, 3))
                h = torch.cat([feat, (g - gm) / gs], dim=1)
                h = torch.clamp_min(h @ t["h.head.0.weight"].T + t["h.head.0.bias"], 0)
                logits = h @ t["h.head.3.weight"].T + t["h.head.3.bias"]
                runs.append(torch.softmax(logits / T, dim=1))
            p = torch.stack(runs).mean(0)
        return p.cpu().numpy().astype(np.float32)

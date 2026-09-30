# SPDX-License-Identifier: AGPL-3.0-only
"""The certified model's faster paths give the serial path's answers: worker
processes, the GPU decoder's pydicom plugin, and the batched encoder in
torch. The card itself is exercised only where there is one."""

from __future__ import annotations

import json
import shutil
from pathlib import Path

import numpy as np
import pytest

pytest.importorskip("lightgbm")
pytest.importorskip("safetensors")

from nils_bodypart import cli, fusion, gpu, scoring, volume  # noqa: E402

from . import fusion_data as fd  # noqa: E402


@pytest.fixture(scope="module")
def world(tmp_path_factory) -> dict:
    root = tmp_path_factory.mktemp("speed")
    return {"root": root, **fd.write_stacks(root), **fd.write_inputs(root)}


def run(world, out: Path, *more: str) -> dict:
    code = cli.main(
        ["infer-fusion", "--stacks", str(world["stacks"]), "--source-root", str(world["source_root"]),
         "--inputs", str(world["inputs"]), "--output", str(out), *more]
    )
    assert code == 0
    return json.loads((out / "results.json").read_text())


def answers(out: Path, res: dict) -> tuple:
    units = [(u["unit_id"], u["status"], u.get("error"), u.get("metrics"), [o["sha256"] for o in u.get("outputs", [])]) for u in res["units"]]
    tables = {p.name: p.read_bytes() for p in sorted((out / "bodypart-fusion").glob("*.json"))}
    return units, res["proposals"], res["metrics"], tables


def test_worker_processes_give_the_serial_answers_byte_for_byte(world, tmp_path):
    serial = run(world, tmp_path / "serial", "--threads", "1", "--device", "cpu")
    pooled = run(world, tmp_path / "pooled", "--threads", "3", "--device", "cpu")
    assert serial["device"] == pooled["device"] == "cpu"
    a, b = answers(tmp_path / "serial", serial), answers(tmp_path / "pooled", pooled)
    assert a == b
    # every kind of unit is there: answered, abstained and failed, in the manifest's order
    assert {u[1] for u in a[0]} >= {"succeeded", "skipped"}
    assert [u[0] for u in a[0]] == [f"stack-{s['stack_id']}" for s in json.loads(Path(world["stacks"]).read_text())["stacks"]]


def test_a_run_asked_for_a_card_where_there_is_none_runs_on_the_cpu(world, tmp_path, monkeypatch):
    monkeypatch.setattr(gpu, "cuda_available", lambda: False)
    res = run(world, tmp_path / "out", "--threads", "1", "--device", "cuda")
    assert res["device"] == "cpu" and "decode_device" not in res["params"]


def test_the_device_is_auto_cpu_or_cuda(world, tmp_path):
    code = cli.main(["infer-fusion", "--stacks", str(world["stacks"]), "--inputs", str(world["inputs"]), "--output", str(tmp_path / "o"), "--device", "tpu"])
    assert code == 1 and "auto, cpu or cuda" in json.loads((tmp_path / "o" / "results.json").read_text())["error"]


def _j2k_file(tmp_path: Path) -> Path:
    """A lossless JPEG 2000 copy of one synthetic slice."""
    pytest.importorskip("openjpeg")
    import pydicom
    from pydicom.uid import JPEG2000Lossless

    src = tmp_path / "plain.dcm"
    fd.write_slice(src, ipp=[-100.0, -120.0, 30.0], iop=[1, 0, 0, 0, 1, 0], ps=[0.9, 0.9], rows=64, cols=48, thick=3.0, inum=1)
    ds = pydicom.dcmread(src)
    ds.BitsStored = 12
    ds.HighBit = 11
    arr = (ds.pixel_array & 0x0FFF).astype(np.uint16)
    ds.PixelData = arr.tobytes()
    ds.compress(JPEG2000Lossless, encoding_plugin="pylibjpeg")
    out = tmp_path / "j2k.dcm"
    ds.save_as(out, enforce_file_format=True)
    return out


def test_the_gpu_plugin_hands_pydicom_the_samples_and_changes_nothing_after(tmp_path):
    """The plugin answers from the samples the card decoded; here they are
    OpenJPEG's own, put where the card would put them, so no card is needed."""
    import openjpeg
    import pydicom
    from pydicom.encaps import generate_frames

    path = _j2k_file(tmp_path)
    ds = pydicom.dcmread(path)
    cs = next(iter(generate_frames(ds.PixelData, number_of_frames=1)))
    gpu._register()
    want = volume.decode_frames(str(path), [0])
    gpu._CACHE[bytes(cs)] = bytes(openjpeg.decode_pixel_data(cs, version=2))
    before = dict(gpu._STATS)
    got = volume.decode_frames(str(path), [0], ds=pydicom.dcmread(path), plugin=gpu.PLUGIN)
    gpu._CACHE.clear()
    assert gpu._STATS["gpu_frames"] == before["gpu_frames"] + 1
    assert got[0].dtype == want[0].dtype and np.array_equal(got[0], want[0])
    # a codestream it was not given is decoded on the CPU, as before
    got = volume.decode_frames(str(path), [0], ds=pydicom.dcmread(path), plugin=gpu.PLUGIN)
    assert gpu._STATS["cpu_frames"] == before["cpu_frames"] + 1 and np.array_equal(got[0], want[0])


@pytest.mark.skipif(not gpu.cuda_available(), reason="no CUDA card")
def test_the_card_decodes_lossless_jpeg_2000_to_the_same_samples(tmp_path):
    pytest.importorskip("nvidia.nvimgcodec")
    path = _j2k_file(tmp_path)
    want = volume.decode_each({str(path): [0]})
    got = gpu.J2KDecoder()({str(path): [0]})
    assert np.array_equal(got[str(path)][0], want[str(path)][0])


def test_the_torch_encoder_is_the_numpy_encoder_within_float32(world):
    pytest.importorskip("torch")
    model = fusion.load(world["inputs"])
    rng = np.random.default_rng(5)
    vols = [rng.integers(0, 256, (64, 64, 64), dtype=np.uint8) for _ in range(3)]
    geos = [np.abs(rng.normal(100, 50, 15)).astype(np.float32) for _ in range(3)]
    want = np.stack([model.encoder.probabilities(v, g) for v, g in zip(vols, geos)])
    got = gpu.TorchEncoder(model.encoder, device="cuda" if gpu.cuda_available() else "cpu").probabilities(vols, geos)
    assert got.shape == (3, 6) and np.abs(got - want).max() < 1e-5


def test_readahead_reads_nothing_it_cannot_open(tmp_path):
    from nils_bodypart import manifest

    st = manifest.Stack(1, [], "stack-1", files=[(str(tmp_path / "missing.dcm"), None)])
    scoring.readahead(st)  # no error

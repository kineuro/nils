# SPDX-License-Identifier: AGPL-3.0-only
"""The few-slice student S1 (record 55 E2, P1b) with random weights of the
frozen shape: needs torch and timm, which the image has and the test
environment may not."""

from __future__ import annotations

import numpy as np
import pytest

torch = pytest.importorskip("torch")
pytest.importorskip("timm")
pytest.importorskip("safetensors")

from nils_bodypart import student  # noqa: E402

from . import cascade_data as cd  # noqa: E402


@pytest.fixture(scope="module")
def frozen_shape(tmp_path_factory):
    return cd.write_student(tmp_path_factory.mktemp("student") / "student.safetensors")


def inputs(n: int, seed: int = 0):
    rng = np.random.default_rng(seed)
    return (rng.integers(0, 256, (n, 192, 192), dtype=np.uint8), rng.normal(0, 1, (n, 4)).astype(np.float32),
            rng.normal(0, 1, (n, student.NTOK)).astype(np.float32))


def test_each_seed_gives_six_probabilities_whatever_the_batch(frozen_shape):
    s = student.Student(frozen_shape, threads=2)
    assert s.seeds == ["1", "2", "3"]
    tiles, sgeo, tok = inputs(7)
    p = s.predict(tiles, sgeo, tok, batch=4)
    assert p.shape == (7, 3, 6) and p.dtype == np.float32
    assert np.allclose(p.sum(-1), 1, atol=1e-5)
    one = s.predict(tiles[2:3], sgeo[2:3], tok[2:3], batch=1)
    assert np.allclose(one[0], p[2], atol=1e-5)
    assert not np.allclose(p[:, 0], p[:, 1])  # the seeds are three networks


def test_the_input_is_the_studys(frozen_shape):
    """A tile is scaled to [0, 1] and normalised at 0.5 and 0.25, one slice
    a stack, its four geometry numbers beside it, and the 42 stack numbers
    at the head, as mimic_bp's dataset fed the network."""
    s = student.Student(frozen_shape, threads=2)
    tiles, sgeo, tok = inputs(3, seed=4)
    net = s.nets[0]
    x = (torch.from_numpy(tiles).float() / 255.0 - 0.5) / 0.25
    with torch.no_grad():
        want = torch.softmax(net(x[:, None, None], torch.from_numpy(sgeo)[:, None], torch.ones(3, 1, dtype=torch.bool), torch.from_numpy(tok)), 1).numpy()
    assert np.allclose(s.predict(tiles, sgeo, tok)[:, 0], want, atol=1e-6)


def test_a_file_of_another_network_is_refused(tmp_path):
    from safetensors.torch import save_file

    bad = tmp_path / "bad.safetensors"
    save_file({"s1.enc.conv_stem.weight": torch.zeros(3, 3)}, str(bad), metadata={"seeds": "1"})
    with pytest.raises(student.StudentError):
        student.Student(bad)
    junk = tmp_path / "junk.safetensors"
    junk.write_bytes(b"not a safetensors file")
    with pytest.raises(student.StudentError):
        student.Student(junk)

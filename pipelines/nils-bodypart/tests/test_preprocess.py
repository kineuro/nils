# SPDX-License-Identifier: AGPL-3.0-only
"""v0's slice choice and slice preparation."""

from __future__ import annotations

import numpy as np

from nils_bodypart.preprocess import (
    central_slice_indices,
    load_frames,
    orientation_slice_indices,
    preprocess_slice,
    seed_slice_index,
    slices_to_embed,
)

from .conftest import write_dicom


def test_centre_slices():
    assert central_slice_indices(10) == [4, 5, 6]
    assert central_slice_indices(1) == [0]
    assert central_slice_indices(2) == [0, 1]
    assert central_slice_indices(0) == []
    assert central_slice_indices(9, n=1) == [4]


def test_axial_reads_five_spread_slices_and_the_rest_the_centre():
    assert orientation_slice_indices(10, "Axial") == [2, 3, 5, 6, 8]
    assert orientation_slice_indices(100, "axial") == [20, 35, 50, 65, 80]
    assert orientation_slice_indices(3, "axial") == [0, 1, 2]
    assert orientation_slice_indices(10, "sagittal") == [4, 5, 6]
    assert orientation_slice_indices(10, None) == [4, 5, 6]


def test_one_embedding_covers_every_later_step():
    for n, ori in [(10, "axial"), (10, "coronal"), (1, None), (37, "axial")]:
        want = set(slices_to_embed(n, ori))
        assert seed_slice_index(n) in want
        assert set(central_slice_indices(n, 3)) <= want
        assert set(orientation_slice_indices(n, ori)) <= want


def test_a_slice_becomes_a_letterboxed_rgb_224():
    arr = np.zeros((100, 50), dtype=np.int16)
    arr[:, 20:30] = 1000
    out = preprocess_slice(arr, rescale_slope=2.0, rescale_intercept=-5.0)
    assert out.shape == (224, 224, 3) and out.dtype == np.uint8
    assert (out[..., 0] == out[..., 1]).all() and (out[..., 1] == out[..., 2]).all()
    # Letterboxed: a tall image leaves black columns at both sides.
    assert out[:, :40].max() == 0 and out[:, -40:].max() == 0
    assert out.max() == 255


def test_the_window_is_ignored_and_percentiles_clip():
    rng = np.random.default_rng(0)
    base = rng.normal(100, 10, (64, 64))
    spiked = base.copy()
    spiked[0, 0] = 1e6  # one hot pixel is clipped at the 99th percentile
    a = preprocess_slice(base)
    b = preprocess_slice(spiked)
    assert np.abs(a.astype(int) - b.astype(int)).mean() < 3
    flat = preprocess_slice(np.full((8, 8), 7.0))
    assert flat.max() == 0


def test_frames_read_from_single_and_multi_frame_files(tmp_path):
    single = tmp_path / "one.dcm"
    write_dicom(single, np.full((4, 5), 9, dtype=np.uint16), slope=2.0, intercept=1.0)
    got = load_frames(str(single), [0, 3])
    assert set(got) == {0, 3}
    arr, slope, intercept = got[0]
    assert arr.shape == (4, 5) and slope == 2.0 and intercept == 1.0
    multi = tmp_path / "multi.dcm"
    write_dicom(multi, np.stack([np.full((4, 4), k, dtype=np.uint16) for k in range(6)]))
    got = load_frames(str(multi), [0, 5, 9])
    assert got[5][0][0, 0] == 5 and got[9][0][0, 0] == 5 and got[0][0][0, 0] == 0
    assert load_frames(str(tmp_path / "missing.dcm"), [0]) == {}

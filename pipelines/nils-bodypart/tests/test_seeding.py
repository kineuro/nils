# SPDX-License-Identifier: AGPL-3.0-only
"""v0's seeding rules against small fixtures."""

from __future__ import annotations

import numpy as np
import pytest

from nils_bodypart.seeding import (
    Candidate,
    SeedConfig,
    farthest_point_indices,
    partition_by_category,
    score_zero_shot,
    seed_category,
    select_seed_candidates,
    split_budget,
    stratified_subsample,
)


def unit(v):
    v = np.asarray(v, dtype=np.float32)
    return v / np.linalg.norm(v)


def test_farthest_point_starts_where_the_seed_says_and_spreads():
    rng = np.random.default_rng(0)
    centres = np.eye(3, dtype=np.float32)
    x = np.vstack([c + 0.01 * rng.standard_normal((5, 3)) for c in centres])
    picked = farthest_point_indices(x, 3, seed=1234)
    assert picked[0] == int(np.random.default_rng(1234).integers(0, len(x)))
    # One pick per cluster.
    assert sorted(i // 5 for i in picked) == [0, 1, 2]


def test_farthest_point_stops_at_duplicates_and_caps_at_n():
    x = np.tile(np.array([[1.0, 0.0]], dtype=np.float32), (4, 1))
    assert len(farthest_point_indices(x, 3, seed=0)) == 1
    assert farthest_point_indices(np.zeros((0, 2)), 3) == []
    y = np.eye(2, dtype=np.float32)
    assert sorted(farthest_point_indices(y, 10, seed=0)) == [0, 1]


def test_margin_is_the_positive_share_less_the_strongest_negative():
    pos = unit([1, 0, 0])
    neg = np.stack([unit([0, 1, 0]), unit([0, 0, 1])])
    img = np.stack([unit([1, 0.1, 0]), unit([0.2, 1, 0])])
    zs, margin = score_zero_shot(img, pos, neg, temperature=100.0)
    logits = (img / np.linalg.norm(img, axis=1, keepdims=True)) @ np.vstack([pos, neg]).T * 100.0
    p = np.exp(logits - logits.max(axis=1, keepdims=True))
    p /= p.sum(axis=1, keepdims=True)
    np.testing.assert_allclose(zs, p[:, 0], rtol=1e-5)
    np.testing.assert_allclose(margin, p[:, 0] - p[:, 1:].max(axis=1), rtol=1e-5, atol=1e-6)
    assert zs[0] > 0.99 and margin[1] < 0


def test_the_gate_falls_back_when_too_few_pass():
    pos = unit([1, 0])
    neg = np.stack([unit([0, 1])])
    # Every image scores p about 0.5: nothing passes 0.55, all pass 0.40.
    img = np.stack([unit([1, 0.999 + 0.0001 * i]) for i in range(10)])
    zs, _ = score_zero_shot(img, pos, neg)
    assert (zs < 0.55).all() and (zs >= 0.40).all()
    got = select_seed_candidates(img, pos, neg, config=SeedConfig(n_target=5, fallback_m_min=-1.0))
    assert 0 < len(got) <= 5
    # With v0's fallback margin of 0 only those with p(pos) >= p(neg) stay.
    got = select_seed_candidates(img, pos, neg, config=SeedConfig(n_target=5))
    assert all(c.margin >= 0 for c in got)


def test_the_primary_gate_holds_when_enough_pass():
    pos = unit([1, 0, 0])
    neg = np.stack([unit([0, 1, 0])])
    rng = np.random.default_rng(3)
    strong = [unit([1, 0.01 * rng.random(), 0.3 * rng.random()]) for _ in range(30)]
    weak = [unit([0.5, 1, 0]) for _ in range(5)]
    img = np.stack(strong + weak)
    got = select_seed_candidates(img, pos, neg, config=SeedConfig(n_target=10))
    assert len(got) == 10
    assert all(c.index < 30 and c.zs_prob >= 0.55 and c.margin >= 0.05 for c in got)


def test_pools_split_by_what_the_rules_said():
    cands = [
        Candidate("1", "brain"),
        Candidate("2", None),
        Candidate("3", "spine"),
        Candidate("4", "neck"),
        Candidate("5", "brain-neck"),
    ]
    prior, null = partition_by_category(cands, "brain")
    assert [c.stack for c in prior] == ["1"] and [c.stack for c in null] == ["2"]
    prior, _ = partition_by_category(cands, "brain-neck")
    assert [c.stack for c in prior] == ["4", "5"]
    prior, null = partition_by_category(cands, "chest")
    assert prior == [] and [c.stack for c in null] == ["2"]


def test_budget_is_half_at_most_fifty_and_rolls_over():
    assert split_budget(100, 500) == (50, 50)
    assert split_budget(300, 500) == (50, 250)
    assert split_budget(100, 10) == (10, 90)
    assert split_budget(7, 500) == (3, 4)
    assert split_budget(10, 500, n_keyword_prior=8, n_zero_shot=5) == (8, 5)
    with pytest.raises(ValueError):
        split_budget(0, 1)


def test_subsample_is_stratified_capped_and_deterministic():
    cands = [Candidate(str(i), None, "SE" if i < 90 else "GRE") for i in range(100)]
    a = stratified_subsample(list(cands), 10, "cohort", "brain")
    b = stratified_subsample(list(cands), 10, "cohort", "brain")
    assert [c.stack for c in a] == [c.stack for c in b]
    assert len(a) == 10
    # The rare cell keeps its share of at least one.
    assert any(c.technique == "GRE" for c in stratified_subsample(list(cands), 5, "k", "brain"))
    assert stratified_subsample(cands[:3], 10, "k", "brain") == cands[:3]
    other = stratified_subsample(list(cands), 10, "cohort", "spine")
    assert [c.stack for c in other] != [c.stack for c in a]


def test_seed_category_runs_both_stages():
    rng = np.random.default_rng(7)
    cands, emb = [], {}
    for i in range(40):
        bp = "brain" if i < 12 else (None if i < 34 else "spine")
        cands.append(Candidate(str(i), bp, "SE"))
        v = rng.standard_normal(8)
        if bp is None:
            v[0] += 6.0  # the null pool looks like the positive prompt
        emb[str(i)] = unit(v)
    pos = np.stack([unit([1, 0, 0, 0, 0, 0, 0, 0])])
    neg = np.stack([unit([0, 1, 0, 0, 0, 0, 0, 0])])
    seeds = seed_category(cands, "brain", emb, pos, neg, n_target=10)
    prior = [s for s in seeds if s.source == "keyword_prior"]
    zs = [s for s in seeds if s.source == "zero_shot"]
    assert len(prior) == 5 and all(int(s.stack) < 12 for s in prior)
    assert zs and all(12 <= int(s.stack) < 34 for s in zs)
    assert not any(int(s.stack) >= 34 for s in seeds)

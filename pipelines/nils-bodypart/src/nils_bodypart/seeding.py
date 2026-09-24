# SPDX-License-Identifier: AGPL-3.0-only
"""Seeding: which stacks a person is first asked to label, per value.

Ported from v0: the scoring and the diversity pick from ``seed.py``, the two
pools from ``candidates.py`` and the budget and the stratified subsample
from ``seeding.py`` (``SeedMixin.seed``). The rules, unchanged:

1. The candidates split into a *keyword-prior pool* (the pack's rules said
   this value, or one the map folds into it) and a *null pool* (the rules
   said nothing). A stack the rules gave another value is in neither.
2. Up to half the budget, at most 50, comes from the prior pool by
   farthest-point sampling on BiomedCLIP image embeddings; whatever the prior
   pool cannot fill rolls over to the zero-shot stage.
3. The null pool is subsampled, stratified by (technique, body part), then
   scored with the text-image margin: a softmax at temperature 100 over the
   averaged positive prompt and every negative prompt gives
   ``margin = p(pos) - max p(neg)``. Kept: p >= 0.55 and margin >= 0.05, or,
   when fewer than min(20, budget) pass, p >= 0.40 and margin >= 0. The kept
   ones are picked by farthest-point sampling on cosine distance, seed 1234.
"""

from __future__ import annotations

import random
from collections import defaultdict
from collections.abc import Sequence
from dataclasses import dataclass

import numpy as np


@dataclass(frozen=True)
class SeedConfig:
    n_target: int = 100
    p_min: float = 0.55
    m_min: float = 0.05
    fallback_p_min: float = 0.40
    fallback_m_min: float = 0.0
    softmax_temperature: float = 100.0
    diversity_seed: int = 1234


DEFAULT_SEED_CONFIG = SeedConfig()

# v0's map from a category to the keyword answers that count as its prior,
# in the pack's values. v0 had no neck category and folded a keyword neck
# into Brain-Neck; chest had no keyword rule.
DEFAULT_CATEGORY_KEYWORD_MAP: dict[str, list[str]] = {
    "brain": ["brain"],
    "spine": ["spine"],
    "brain-neck": ["brain-neck", "neck"],
}

# v0's default categories, in the pack's values.
DEFAULT_CATEGORIES: tuple[str, ...] = ("brain", "brain-neck", "spine", "chest")


def _l2_normalize_rows(x: np.ndarray) -> np.ndarray:
    n = np.linalg.norm(x, axis=1, keepdims=True).clip(min=1e-12)
    return x / n


def score_zero_shot(
    image_embeddings: np.ndarray,
    pos_text_embedding: np.ndarray,
    neg_text_embeddings: np.ndarray,
    *,
    temperature: float = DEFAULT_SEED_CONFIG.softmax_temperature,
) -> tuple[np.ndarray, np.ndarray]:
    """``(zs_prob, margin)``, each of shape (N,): the softmax share of the
    positive prompt among (positive, negatives), and that share less the
    strongest negative's."""
    if image_embeddings.size == 0:
        return np.zeros((0,), dtype=np.float32), np.zeros((0,), dtype=np.float32)
    img = _l2_normalize_rows(image_embeddings.astype(np.float32))
    pos = pos_text_embedding.astype(np.float32)
    pos = pos / max(float(np.linalg.norm(pos)), 1e-12)
    neg = _l2_normalize_rows(np.atleast_2d(neg_text_embeddings).astype(np.float32))
    text = np.vstack([pos[None, :], neg])
    logits = (img @ text.T) * float(temperature)
    logits -= logits.max(axis=1, keepdims=True)
    exp = np.exp(logits)
    probs = exp / exp.sum(axis=1, keepdims=True)
    zs_prob = probs[:, 0].astype(np.float32)
    if probs.shape[1] == 1:
        margin = zs_prob.copy()
    else:
        margin = zs_prob - probs[:, 1:].max(axis=1).astype(np.float32)
    return zs_prob, margin


def farthest_point_indices(embeddings: np.ndarray, n_target: int, *, seed: int = 0) -> list[int]:
    """Up to ``n_target`` rows by farthest-point sampling on cosine distance.

    The first pick is drawn with ``numpy.random.default_rng(seed)``; each next
    pick maximises the smallest cosine distance to those picked. It stops
    early when every row left duplicates one picked. Indices in pick order.
    """
    n = embeddings.shape[0]
    if n == 0 or n_target <= 0:
        return []
    n_target = min(n_target, n)
    x = _l2_normalize_rows(embeddings.astype(np.float32))
    rng = np.random.default_rng(seed)
    first = int(rng.integers(0, n))
    picked = [first]
    min_dist = 1.0 - x @ x[first]
    while len(picked) < n_target:
        nxt = int(np.argmax(min_dist))
        if min_dist[nxt] <= 0.0:
            break
        picked.append(nxt)
        min_dist = np.minimum(min_dist, 1.0 - x @ x[nxt])
    return picked


@dataclass
class SeedCandidate:
    index: int
    zs_prob: float
    margin: float


def select_seed_candidates(
    image_embeddings: np.ndarray,
    pos_text_embedding: np.ndarray,
    neg_text_embeddings: np.ndarray,
    *,
    config: SeedConfig = DEFAULT_SEED_CONFIG,
) -> list[SeedCandidate]:
    """Score, gate (with v0's fallback) and pick by diversity."""
    n = image_embeddings.shape[0]
    if n == 0 or config.n_target <= 0:
        return []
    zs, margin = score_zero_shot(
        image_embeddings, pos_text_embedding, neg_text_embeddings, temperature=config.softmax_temperature
    )
    keep = (zs >= config.p_min) & (margin >= config.m_min)
    if int(keep.sum()) < min(20, config.n_target):
        keep = (zs >= config.fallback_p_min) & (margin >= config.fallback_m_min)
    if int(keep.sum()) == 0:
        return []
    kept_idx = np.where(keep)[0]
    pick_local = farthest_point_indices(image_embeddings[kept_idx], config.n_target, seed=config.diversity_seed)
    return [
        SeedCandidate(index=int(kept_idx[li]), zs_prob=float(zs[kept_idx[li]]), margin=float(margin[kept_idx[li]]))
        for li in pick_local
    ]


def select_prior_candidates(
    image_embeddings: np.ndarray, n_target: int, *, seed: int = DEFAULT_SEED_CONFIG.diversity_seed
) -> list[int]:
    """The prior pool needs no scoring: a diverse pick of its rows."""
    if image_embeddings.shape[0] == 0 or n_target <= 0:
        return []
    return farthest_point_indices(image_embeddings, n_target, seed=seed)


# ---------------------------------------------------------------- the pools


@dataclass(frozen=True)
class Candidate:
    """One stack offered to the seeder, with what the pack's rules said."""

    stack: str
    body_part: str | None
    technique: str | None = None
    orientation: str | None = None


def partition_by_category(
    candidates: Sequence[Candidate],
    category: str,
    category_keyword_map: dict[str, list[str]] | None = None,
) -> tuple[list[Candidate], list[Candidate]]:
    """(prior pool, null pool): the stacks the rules gave this value, and the
    stacks the rules gave nothing. A stack given another value is excluded."""
    mapping = category_keyword_map or DEFAULT_CATEGORY_KEYWORD_MAP
    match_values = {v.lower() for v in mapping.get(category, [])}
    prior: list[Candidate] = []
    null: list[Candidate] = []
    for c in candidates:
        if c.body_part is None or c.body_part == "":
            null.append(c)
        elif c.body_part.lower() in match_values:
            prior.append(c)
    return prior, null


def split_budget(
    n_target: int, prior_size: int, n_keyword_prior: int | None = None, n_zero_shot: int | None = None
) -> tuple[int, int]:
    """(prior budget, zero-shot budget): half of the target, at most 50, from
    the prior pool by default; what the prior pool cannot fill rolls over."""
    if n_target <= 0:
        raise ValueError("n_target must be > 0")
    if n_keyword_prior is None:
        n_keyword_prior = min(50, n_target // 2)
    if n_zero_shot is None:
        n_zero_shot = n_target - n_keyword_prior
    actual = min(n_keyword_prior, prior_size)
    n_zero_shot += n_keyword_prior - actual
    return actual, n_zero_shot


def stratified_subsample(
    candidates: list[Candidate], pool_cap: int, seed_key: str, category: str, env_cap: int | None = None
) -> list[Candidate]:
    """v0's technique-weighted subsample of the null pool: cells of
    (technique, body part), each given its proportional share of
    ``pool_cap`` and at least one, shuffled by ``random.Random(f"{seed_key}:
    {category}")``. v0 seeded with the cohort's name; here the caller names
    the key. ``env_cap`` is v0's ``BODY_PART_SEED_POOL_CAP``, read only when
    the pool is larger than the cap."""
    if pool_cap <= 0 or not candidates:
        return []
    if len(candidates) <= pool_cap:
        return list(candidates)
    if env_cap is not None:
        pool_cap = int(env_cap) or pool_cap
    rng = random.Random(f"{seed_key}:{category}")
    cells: dict[tuple[str, str], list[Candidate]] = defaultdict(list)
    for c in candidates:
        cells[(c.technique or "Unknown", c.body_part or "unlabeled")].append(c)
    total = len(candidates)
    out: list[Candidate] = []
    for items in cells.values():
        share = max(1, round(pool_cap * len(items) / total))
        rng.shuffle(items)
        out.extend(items[:share])
    rng.shuffle(out)
    return out[:pool_cap]


@dataclass
class Seed:
    """One proposal of the seeder."""

    stack: str
    category: str
    source: str  # "keyword_prior" | "zero_shot"
    zs_prob: float
    margin: float


def seed_category(
    candidates: Sequence[Candidate],
    category: str,
    embedding_of: dict[str, np.ndarray],
    text_positive: np.ndarray,
    text_negative: np.ndarray,
    *,
    n_target: int = 100,
    n_keyword_prior: int | None = None,
    n_zero_shot: int | None = None,
    p_min: float | None = None,
    m_min: float | None = None,
    category_keyword_map: dict[str, list[str]] | None = None,
    seed_key: str = "nils",
    pool_cap_override: int | None = None,
) -> list[Seed]:
    """Both stages for one value. ``embedding_of`` maps a stack to its
    BiomedCLIP embedding of the centre slice; a stack without one is left
    out, as v0's worker left out a slice it could not resolve.
    ``text_positive`` is the positive prompts' embeddings (P, D), averaged
    here, and ``text_negative`` the negatives' (M, D)."""
    prior_pool, null_pool = partition_by_category(candidates, category, category_keyword_map)
    n_prior, n_zs = split_budget(n_target, len(prior_pool), n_keyword_prior, n_zero_shot)
    out: list[Seed] = []

    if n_prior > 0 and prior_pool:
        usable = [c for c in prior_pool if c.stack in embedding_of]
        if usable:
            emb = np.stack([embedding_of[c.stack] for c in usable])
            for i in select_prior_candidates(emb, n_prior):
                out.append(Seed(stack=usable[i].stack, category=category, source="keyword_prior", zs_prob=1.0, margin=1.0))

    if n_zs > 0 and null_pool:
        scored = stratified_subsample(list(null_pool), n_zs, seed_key, category, pool_cap_override)
        usable = [c for c in scored if c.stack in embedding_of]
        if usable:
            emb = np.stack([embedding_of[c.stack] for c in usable])
            pos = np.atleast_2d(text_positive).mean(axis=0)
            pos = pos / max(float(np.linalg.norm(pos)), 1e-12)
            cfg = SeedConfig(
                n_target=int(n_zs),
                p_min=DEFAULT_SEED_CONFIG.p_min if p_min is None else float(p_min),
                m_min=DEFAULT_SEED_CONFIG.m_min if m_min is None else float(m_min),
            )
            for c in select_seed_candidates(emb, pos, text_negative, config=cfg):
                out.append(
                    Seed(stack=usable[c.index].stack, category=category, source="zero_shot", zs_prob=c.zs_prob, margin=c.margin)
                )
    return out

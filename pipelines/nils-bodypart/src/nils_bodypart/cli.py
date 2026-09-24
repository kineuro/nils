# SPDX-License-Identifier: AGPL-3.0-only
"""The four entry points: ``nils-bodypart embed | seed | train | infer``.

What a container meets (``contracts/job/v1``): ``/input/stacks.json``, the
source places read-only under ``/source/<n>``, the typed inputs read-only
under ``/inputs/<id>``, and ``/output``, the only place it writes. Each entry
point ends with ``results.json`` in its output folder. The exit code is 0
when the run completed, whatever its units' statuses (the runner reads
those), and 1 when the run itself could not be done; ``results.json`` then
says why.
"""

from __future__ import annotations

import argparse
import json
import logging
import os
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import numpy as np

from . import PACK_VALUES, PACK_VERSION, PREPROCESS_VERSION, __version__
from . import embeddings as emb
from . import manifest
from .encoders import DEFAULT_CHAIN, Encoders
from .preprocess import central_slice_indices, load_frames, preprocess_slice, seed_slice_index, slices_to_embed
from .results import FAILED, SKIPPED, Run, Unit

logger = logging.getLogger("nils_bodypart")
AXIS = "body_part"


class RunError(Exception):
    """The run cannot be done at all."""


def _chain(arg: str | None) -> list[str]:
    names = [n.strip() for n in (arg or ",".join(DEFAULT_CHAIN)).split(",") if n.strip()]
    for n in names:
        if n not in DEFAULT_CHAIN:
            raise RunError(f"unknown encoder {n!r}; the image has {', '.join(DEFAULT_CHAIN)}")
    return names


def _image_digest() -> str | None:
    d = os.environ.get("NILS_IMAGE_DIGEST")
    return d if d and d.startswith("sha256:") else None


def _encoder_card(info) -> dict:
    """An encoder's card, as the engine registers an encoder (record 43 S4)."""
    card = {
        "name": info.name,
        "version": info.version,
        "kind": "encoder",
        "digest": info.digest,
        "task": "encoder",
        "artifact": {"format": "other"},
        "preprocessing": {"version": PREPROCESS_VERSION},
        "intended_use": "turns the slices of a stack into features; its weights stay in the pipeline image",
    }
    if info.repo:
        card["params"] = {"repo": info.repo, "revision": info.revision, "dim": info.dim}
    if _image_digest():
        card["image_digest"] = _image_digest()
    return card


def _features(store: emb.Store, digests: list[str], stack: manifest.Stack, indices: list[int]) -> dict[int, np.ndarray]:
    """{slice: the chain's rows concatenated} for the slices every encoder has."""
    held = [store.get(d, PREPROCESS_VERSION, stack.stack_id) for d in digests]
    if any(h is None for h in held):
        return {}
    maps = [h.rows_by_slice() for h in held]  # type: ignore[union-attr]
    return {s: np.concatenate([m[s] for m in maps]).astype(np.float32) for s in indices if all(s in m for m in maps)}


# ------------------------------------------------------------------- embed


def _load_stack_images(stack: manifest.Stack, indices: list[int]) -> dict[int, np.ndarray]:
    by_file: dict[str, list[tuple[int, int]]] = {}
    for i in indices:
        sl = stack.slices[i]
        by_file.setdefault(sl.path, []).append((i, sl.frame))
    out: dict[int, np.ndarray] = {}
    for path, pairs in by_file.items():
        frames = load_frames(path, [f for _, f in pairs])
        for i, f in pairs:
            got = frames.get(f)
            if got is None:
                continue
            arr, slope, intercept = got
            try:
                out[i] = preprocess_slice(arr, rescale_slope=slope, rescale_intercept=intercept)
            except ValueError:
                continue
    return out


def cmd_embed(a: argparse.Namespace) -> Run:
    encoders = Encoders(standin=a.standin, device=a.device)
    chain = _chain(a.encoders)
    params = {"encoders": chain, "batch": a.batch, "threads": a.threads, "preprocess_version": PREPROCESS_VERSION}
    run = Run("bodypart-embed", a.output, params, encoders.device_label())
    stacks = manifest.load(a.stacks, source_root=a.source_root)
    store = emb.Store.scan(a.embeddings)
    infos = {n: encoders.info(n) for n in chain}
    run.models = [_encoder_card(infos[n]) for n in chain]

    # What each stack still needs, per encoder: only what the cache lacks.
    plan: list[tuple[manifest.Stack, dict[str, list[int]], list[int]]] = []
    for st in stacks:
        if st.num_slices == 0:
            run.units.append(Unit(st.unit, status=FAILED, error="the stack lists no files"))
            continue
        want = slices_to_embed(st.num_slices, st.orientation)
        missing: dict[str, list[int]] = {}
        for n in chain:
            held = store.get(infos[n].digest, PREPROCESS_VERSION, st.stack_id)
            have = set(held.slices) if held else set()
            m = [i for i in want if i not in have]
            if m:
                missing[n] = m
        if not missing:
            run.units.append(Unit(st.unit, status=SKIPPED, metrics={"cached": True, "slices": len(want)}))
            continue
        plan.append((st, missing, want))

    computed = 0
    per_chunk = max(1, a.batch // 4)
    pool = ThreadPoolExecutor(max_workers=max(1, a.threads))
    try:
        for off in range(0, len(plan), per_chunk):
            chunk = plan[off : off + per_chunk]
            union = [sorted({i for m in missing.values() for i in m}) for _, missing, _ in chunk]
            images = list(pool.map(lambda p: _load_stack_images(*p), [(c[0], u) for c, u in zip(chunk, union)]))
            fresh_rows: dict[tuple[int, str], dict[int, np.ndarray]] = {}
            for n in chain:
                keys: list[tuple[int, int]] = []
                imgs: list[np.ndarray] = []
                for ci, (_, missing, _) in enumerate(chunk):
                    for i in missing.get(n, []):
                        if i in images[ci]:
                            keys.append((ci, i))
                            imgs.append(images[ci][i])
                for k in range(0, len(imgs), a.batch):
                    mat = encoders.get(n).encode_images(imgs[k : k + a.batch])
                    for (ci, i), row in zip(keys[k : k + a.batch], mat):
                        fresh_rows.setdefault((ci, n), {})[i] = row
            for ci, (st, _, want) in enumerate(chunk):
                u = Unit(st.unit, metrics={"slices": len(want), "read": len(images[ci])})
                if len(images[ci]) < len(union[ci]):
                    u.metrics["unreadable"] = len(union[ci]) - len(images[ci])
                for n in chain:
                    fresh = fresh_rows.get((ci, n))
                    if not fresh:
                        continue
                    rows = dict(fresh)
                    held = store.get(infos[n].digest, PREPROCESS_VERSION, st.stack_id)
                    if held is not None:
                        for s, r in held.rows_by_slice().items():
                            rows.setdefault(s, r)
                    slices = sorted(rows)
                    e = emb.Embedding(st.stack_id, infos[n].digest, PREPROCESS_VERSION, slices, np.stack([rows[s] for s in slices]))
                    u.outputs.append(
                        run.output_file(
                            f"embeddings/{n}/{st.stack_id}{emb.EXTENSION}",
                            emb.encode(e),
                            "embedding",
                            emb.MEDIA_TYPE,
                            stack_id=st.stack_id,
                            model=infos[n].digest,
                            preprocess_version=PREPROCESS_VERSION,
                        )
                    )
                    computed += len(fresh)
                if not images[ci]:
                    u.status, u.error = FAILED, "no slice of the stack could be read"
                elif not u.outputs:
                    u.status, u.error = FAILED, "no embedding was written"
                run.units.append(u)
    finally:
        pool.shutdown()
    run.metrics = {"stacks": len(stacks), "cached": sum(1 for u in run.units if u.status == SKIPPED), "rows_written": computed}
    return run


# -------------------------------------------------------------------- seed


def cmd_seed(a: argparse.Namespace) -> Run:
    from .prompts import prompts_for_category
    from .seeding import DEFAULT_CATEGORIES, DEFAULT_CATEGORY_KEYWORD_MAP, Candidate, seed_category

    encoders = Encoders(standin=a.standin, device=a.device)
    categories = [c.strip() for c in (a.categories or ",".join(DEFAULT_CATEGORIES)).split(",") if c.strip()]
    for c in categories:
        if c not in PACK_VALUES:
            raise RunError(f"{c!r} is not a value of the pack's body_part ({PACK_VERSION})")
    keyword_map = json.loads(a.keyword_map) if a.keyword_map else DEFAULT_CATEGORY_KEYWORD_MAP
    params = {
        "categories": categories,
        "n_target": a.n_target,
        "n_keyword_prior": a.n_keyword_prior,
        "n_zero_shot": a.n_zero_shot,
        "p_min": a.p_min,
        "m_min": a.m_min,
        "keyword_map": keyword_map,
        "seed_key": a.seed_key,
        "pool_cap": a.pool_cap,
    }
    run = Run("bodypart-seed", a.output, params, encoders.device_label())
    stacks = manifest.load(a.stacks, source_root=a.source_root)
    store = emb.Store.scan(a.embeddings)
    bc = encoders.info("biomedclip")

    candidates: list[Candidate] = []
    emb_of: dict[str, np.ndarray] = {}
    unit_of: dict[int, Unit] = {}
    for st in stacks:
        idx = seed_slice_index(st.num_slices)
        e = store.get(bc.digest, PREPROCESS_VERSION, st.stack_id)
        row = e.rows_by_slice().get(idx) if (e is not None and idx is not None) else None
        if row is None:
            run.units.append(Unit(st.unit, status=SKIPPED, error="no BiomedCLIP embedding of the centre slice"))
            continue
        emb_of[str(st.stack_id)] = row
        candidates.append(Candidate(str(st.stack_id), st.body_part, st.technique, st.orientation))
        unit_of[st.stack_id] = Unit(st.unit)
        run.units.append(unit_of[st.stack_id])

    seeds: list[dict] = []
    per_value: dict[str, dict] = {}
    enc = encoders.get("biomedclip") if candidates else None
    for cat in categories if enc is not None else []:
        pack = prompts_for_category(cat)
        got = seed_category(
            candidates,
            cat,
            emb_of,
            enc.encode_texts(list(pack.positive)),
            enc.encode_texts(list(pack.negative)),
            n_target=a.n_target,
            n_keyword_prior=a.n_keyword_prior,
            n_zero_shot=a.n_zero_shot,
            p_min=a.p_min,
            m_min=a.m_min,
            category_keyword_map=keyword_map,
            seed_key=a.seed_key,
            pool_cap_override=a.pool_cap,
        )
        per_value[cat] = {src: sum(1 for s in got if s.source == src) for src in ("keyword_prior", "zero_shot")}
        seeds += [
            {"axis": AXIS, "stack_id": int(s.stack), "value": s.category, "source": s.source, "zs_prob": round(s.zs_prob, 4), "margin": round(s.margin, 4)}
            for s in got
        ]
    # Seeds are suggestions for a person to curate, not a model's answer, so
    # they are not `proposals` (contracts/job/v1 proposals.schema.json): they
    # are `seeds`, with one small file per seeded stack (a stack-level output
    # names its stack), and `selection` is the list a curation campaign
    # starts from.
    by_stack: dict[int, list[dict]] = {}
    for sd in seeds:
        by_stack.setdefault(sd["stack_id"], []).append(sd)
    for sid, rows in sorted(by_stack.items()):
        body = json.dumps({"stack_id": sid, "seeds": rows}, indent=2, sort_keys=True) + "\n"
        unit_of[sid].outputs.append(run.output_file(f"seeds/{sid}.json", body.encode(), "output", "application/json"))
    run.extra["seeds"] = seeds
    run.extra["selection"] = {"stacks": sorted(by_stack)}
    run.metrics = {"candidates": len(candidates), "per_value": per_value, "stacks_seeded": len(by_stack), "encoder": bc.digest}
    return run


# ------------------------------------------------------------------- train


def cmd_train(a: argparse.Namespace) -> Run:
    from . import labels as labelsets
    from .head import TrainConfig, canonical_json, fit_head, head_document, sha256_bytes

    encoders = Encoders(standin=a.standin, device="cpu")
    chain = _chain(a.encoders)
    remap = json.loads(a.label_remap) if a.label_remap else {}
    classes = [c.strip() for c in a.classes.split(",")] if a.classes else None
    cfg = TrainConfig(
        pca_components=a.pca_components if a.use_pca else None,
        logreg_C=a.C,
        random_state=a.random_state,
        n_train_slices=a.n_train_slices,
        auto_tune=a.auto_tune,
        estimator_kind=a.estimator,
    )
    params = {
        "estimator": a.estimator,
        "encoders": chain,
        "classes": classes,
        "label_remap": remap,
        "auto_tune": a.auto_tune,
        "use_pca": a.use_pca,
        "pca_components": a.pca_components,
        "C": a.C,
        "random_state": a.random_state,
        "n_train_slices": a.n_train_slices,
        "min_per_class": a.min_per_class,
        "threshold": a.threshold,
        "calibration": "temperature" if a.estimator == "logreg" else "CalibratedClassifierCV(sigmoid, cv=3)",
    }
    if not 0.0 < a.threshold <= 1.0:
        raise RunError(f"the threshold is a probability above 0 and at most 1, not {a.threshold}")
    run = Run("bodypart-train", a.output, params, "cpu")
    ls = labelsets.load(a.labels)
    by_stack, conflicted = ls.stack_labels(AXIS)
    stacks = manifest.load(a.stacks, source_root=a.source_root)
    store = emb.Store.scan(a.embeddings)
    infos = [encoders.info(n) for n in chain]
    digests = [i.digest for i in infos]

    X_rows, y_rows, outside = [], [], 0
    for st in stacks:
        u = Unit(st.unit)
        value = by_stack.get(st.stack_id)
        value = remap.get(value, value) if value is not None else None
        if value is None:
            u.status, u.error = SKIPPED, "the label set gives the stack no body_part, or two"
        elif value not in PACK_VALUES:
            outside += 1
            u.status, u.error = SKIPPED, "the label is not a value of the pack's body_part"
        elif classes is not None and value not in classes:
            u.status, u.error = SKIPPED, "the label is not one of the classes asked for"
        else:
            feats = _features(store, digests, st, central_slice_indices(st.num_slices, n=cfg.n_train_slices))
            if not feats:
                u.status, u.error = SKIPPED, "no embedding of the centre slices under every encoder"
            else:
                X_rows.append(np.stack(list(feats.values())).mean(axis=0))
                y_rows.append(value)
                u.metrics = {"slices": len(feats), "label": value}
        run.units.append(u)

    counts: dict[str, int] = {}
    for v in y_rows:
        counts[v] = counts.get(v, 0) + 1
    in_manifest = {st.stack_id for st in stacks}
    run.metrics = {
        "label_set": ls.digest,
        "labelled_stacks": len(by_stack),
        "not_in_manifest": sum(1 for s in by_stack if s not in in_manifest),
        "conflicted": conflicted,
        "outside_pack": outside,
        "per_class_counts": counts,
    }
    if len(counts) < 2:
        raise RunError(f"training needs at least two classes with features, got {sorted(counts)}")
    few = sorted(c for c, n in counts.items() if n < a.min_per_class)
    if few:
        raise RunError(f"classes with fewer than {a.min_per_class} samples: {few}")

    X = np.stack(X_rows).astype(np.float32)
    head = fit_head(X, np.asarray(y_rows, dtype=object), cfg)
    chain_doc = [{"name": i.name, "digest": i.digest, "dim": i.dim} for i in infos]
    if head.kind == "logreg":
        doc = head_document(head, encoder_chain=chain_doc, preprocess_version=PREPROCESS_VERSION, n_train_slices=cfg.n_train_slices)
        data, name, fmt, media = canonical_json(doc), "head.json", "json", "application/json"
    else:
        import io

        import joblib

        buf = io.BytesIO()
        joblib.dump({"estimator": head.estimator, "classes": head.classes, "encoder_chain": chain_doc}, buf)
        data, name, fmt, media = buf.getvalue(), "head.joblib", "joblib", "application/octet-stream"
    digest = sha256_bytes(data)
    metrics = {
        **head.metrics,
        "n_samples": int(X.shape[0]),
        "per_class_counts": head.counts,
        "dim_in": int(X.shape[1]),
        "pca_components": head.n_components,
        "hyperparams": head.hyperparams,
    }
    if head.temperature is not None:
        metrics["temperature"] = round(head.temperature, 4)
    if head.tune_report is not None:
        metrics["tune"] = {k: v for k, v in head.tune_report.items() if k != "grid"} | {"grid_size": len(head.tune_report["grid"])}
    card = {
        "name": a.name,
        "version": a.model_version or f"h{digest[7:19]}",
        "kind": "head",
        "digest": digest,
        "task": f"axis:{AXIS}",
        "slot": a.slot,
        # Every encoder the head reads, in the order it concatenates their
        # features (contracts/model/v1, record 43).
        "encoders": [{"digest": i.digest, "name": i.name, "version": i.version} for i in infos],
        # The probability at or above which the engine stages its proposals;
        # a run may raise it, never lower it.
        "threshold": a.threshold,
        "trained_on": {"label_set": ls.digest, "name": ls.name or "", "rows": len(ls.rows)},
        "pack_version": ls.pack_version or PACK_VERSION,
        "artifact": {"format": fmt, "bytes": len(data)},
        "metrics": metrics,
        "preprocessing": {
            "version": PREPROCESS_VERSION,
            "encoders": chain_doc,
            "n_train_slices": cfg.n_train_slices,
            "features": "the chain's rows concatenated, averaged over the centre slices",
        },
        "params": params,
        "intended_use": "proposes the pack's body_part of an MRI stack from its slices; a proposal is evidence until a person commits it",
        "limits": [
            "trained on the label set it names; values outside its classes are never proposed",
            "the axial brain-neck rule needs brain, spine and brain-neck among its classes",
        ]
        + (["the artifact is a pickle, which executes code when it is loaded"] if fmt == "joblib" else []),
        "runtime": {"name": "nils-bodypart", "version": __version__},
    }
    if _image_digest():
        card["image_digest"] = _image_digest()
    artifact = run.output_file(f"head/{name}", data, "output", media, model=digest)
    card_file = run.output_file("head/card.json", (json.dumps(card, indent=2, sort_keys=True) + "\n").encode(), "output", "application/json", model=digest)
    run.models = [card]
    run.extra["head"] = {"artifact": artifact["path"], "card": card_file["path"], "digest": digest}
    run.metrics.update({k: metrics[k] for k in ("calibrated", "uncalibrated", "folds") if k in metrics})
    return run


# ------------------------------------------------------------------- infer


def _load_head(directory: Path, allow_pickle: bool):
    """The head in a folder: a ``nils-bodypart-head/1`` JSON document, or a
    joblib pickle when trusted, and the card beside it if there is one."""
    from .head import HEAD_FORMAT, JsonHead, _align, sha256_bytes

    d = Path(directory)
    files = sorted(p for p in d.rglob("*") if p.is_file()) if d.is_dir() else [d]
    doc_file, pickle_file, card = None, None, {}
    for p in files:
        if p.suffix == ".json":
            try:
                j = json.loads(p.read_text(encoding="utf-8"))
            except (UnicodeDecodeError, json.JSONDecodeError):
                continue
            if isinstance(j, dict) and j.get("format") == HEAD_FORMAT:
                doc_file = p
            elif isinstance(j, dict) and j.get("kind") == "head" and "digest" in j:
                card = j
        elif p.suffix == ".joblib":
            pickle_file = p
    if doc_file is not None:
        data = doc_file.read_bytes()
        head = JsonHead(json.loads(data))
        chain = head.encoder_chain
    elif pickle_file is not None:
        if not allow_pickle:
            raise RunError("the head is a pickle; run with --allow-pickle true to trust it")
        import joblib

        data = pickle_file.read_bytes()
        obj = joblib.load(pickle_file)
        est, classes, chain = obj["estimator"], list(obj["classes"]), obj["encoder_chain"]

        class _Wrapped:
            classes_ = classes

            def predict_proba(self, X):
                return _align(est.predict_proba(X), list(est.classes_), classes)

        head = _Wrapped()
    else:
        raise RunError("the head's folder holds no nils-bodypart head")
    digest = sha256_bytes(data)
    if card.get("digest") and card["digest"] != digest:
        raise RunError("the head is not the artifact its card names")
    return head, chain, {"digest": digest, "name": card.get("name"), "version": card.get("version")}


def cmd_infer(a: argparse.Namespace) -> Run:
    from .infer import InferConfig, predict_stack

    head, chain, model = _load_head(a.head, a.allow_pickle)
    cfg = InferConfig(n_slices=3, manual_review_below=a.threshold)
    run = Run("bodypart-infer", a.output, {"threshold": a.threshold, "model": model}, "cpu")
    store = emb.Store.scan(a.embeddings)
    digests = [c["digest"] for c in chain]
    labels: dict[str, int] = {}
    needs = 0
    for st in manifest.load(a.stacks, source_root=a.source_root):
        feats = _features(store, digests, st, list(range(st.num_slices)))
        pred = predict_stack(head=head, stack=str(st.stack_id), num_slices=st.num_slices, orientation=st.orientation, slice_features=feats, config=cfg)
        if pred.label is None:
            run.units.append(Unit(st.unit, status=SKIPPED, error="no embedding of the slices inference reads under every encoder of the head"))
            continue
        detail = {
            "value": pred.label,
            "confidence": round(pred.confidence, 4),
            "needs_check": pred.needs_check,
            "slices": pred.n_slices_used,
            "aggregation": pred.reasoning.get("aggregation"),
        }
        u = Unit(st.unit, metrics=detail)
        # The stack's answer as a file of its own: a stack-level output names
        # its stack (contracts/job/v1), and the reasoning is kept whole here.
        body = {"stack_id": st.stack_id, **detail, "probabilities": pred.probs, "reasoning": pred.reasoning, "model": model}
        u.outputs.append(run.output_file(f"bodypart/{st.stack_id}.json", (json.dumps(body, indent=2, sort_keys=True) + "\n").encode(), "output", "application/json"))
        run.units.append(u)
        labels[pred.label] = labels.get(pred.label, 0) + 1
        needs += int(pred.needs_check)
        p = {
            "stack_id": st.stack_id,
            "axis": AXIS,
            "value": pred.label,
            "probabilities": {k: round(v, 4) for k, v in pred.probs.items()},
            "model_digest": model["digest"],
        }
        if pred.reasoning.get("aggregation") == "axial_compose":
            p["note"] = "axial brain-neck rule: brain in the upper slices, spine in the lower"
        run.proposals.append(p)
    run.metrics = {"proposed": len(run.proposals), "per_value": labels, "needs_check": needs}
    return run


# --------------------------------------------------------------------- main


def _bool(v: str) -> bool:
    return str(v).strip().lower() in ("1", "true", "yes", "on")


def parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="nils-bodypart", description="v0's body-part detector as a NILS pipeline")
    p.add_argument("--version", action="version", version=f"nils-bodypart {__version__}")
    sub = p.add_subparsers(dest="entry", required=True)

    def common(sp):
        sp.add_argument("--stacks", type=Path, default=Path("/input/stacks.json"), help="the stacks layout's manifest")
        sp.add_argument("--source-root", type=Path, default=None, help="where /source/<n> is, when not at /source")
        sp.add_argument("--embeddings", type=Path, default=None, help="existing embeddings, read only")
        sp.add_argument("--output", type=Path, default=Path("/output"))
        sp.add_argument("--standin", action="store_true", help="tiny stand-in encoders, for tests")
        sp.add_argument("--device", default=os.environ.get("NILS_BODYPART_DEVICE", "auto"))

    e = sub.add_parser("embed", help="prepare slices and embed them")
    common(e)
    e.add_argument("--encoders", default=",".join(DEFAULT_CHAIN))
    e.add_argument("--batch", type=int, default=32)
    e.add_argument("--threads", type=int, default=16)

    s = sub.add_parser("seed", help="propose the stacks to label first, per value")
    common(s)
    s.add_argument("--categories", default=None)
    s.add_argument("--n-target", type=int, default=100)
    s.add_argument("--n-keyword-prior", type=int, default=None)
    s.add_argument("--n-zero-shot", type=int, default=None)
    s.add_argument("--p-min", type=float, default=None)
    s.add_argument("--m-min", type=float, default=None)
    s.add_argument("--keyword-map", default=None, help="JSON: value to the keyword answers that are its prior")
    s.add_argument("--seed-key", default="nils")
    s.add_argument("--pool-cap", type=int, default=None)

    t = sub.add_parser("train", help="fit and calibrate a head on a label set")
    common(t)
    t.add_argument("--labels", type=Path, required=True, help="the label set's folder")
    t.add_argument("--encoders", default=",".join(DEFAULT_CHAIN))
    t.add_argument("--estimator", choices=("logreg", "rf", "svm"), default="logreg")
    t.add_argument("--classes", default=None)
    t.add_argument("--label-remap", default=None, help="JSON: fold labels before training")
    t.add_argument("--auto-tune", type=_bool, default=True)
    t.add_argument("--use-pca", type=_bool, default=True)
    t.add_argument("--pca-components", type=int, default=128)
    t.add_argument("--C", type=float, default=1.0)
    t.add_argument("--random-state", type=int, default=0)
    t.add_argument("--n-train-slices", type=int, default=3)
    t.add_argument("--min-per-class", type=int, default=5)
    t.add_argument("--name", default="bodypart-head")
    t.add_argument("--model-version", default=None)
    t.add_argument("--slot", default="site")
    t.add_argument("--threshold", type=float, default=0.70, help="the card's threshold: proposals at or above it are staged")

    i = sub.add_parser("infer", help="propose a body part per stack")
    common(i)
    i.add_argument("--head", type=Path, required=True, help="the head's folder")
    i.add_argument("--threshold", type=float, default=0.70)
    i.add_argument("--allow-pickle", type=_bool, default=False)
    return p


ENTRIES = {"embed": cmd_embed, "seed": cmd_seed, "train": cmd_train, "infer": cmd_infer}


def main(argv: list[str] | None = None) -> int:
    logging.basicConfig(level=os.environ.get("LOG_LEVEL", "INFO"), format="%(levelname)s %(name)s: %(message)s")
    a = parser().parse_args(argv)
    try:
        run = ENTRIES[a.entry](a)
    except (RunError, manifest.ManifestError, ValueError) as e:
        failed = Run(f"bodypart-{a.entry}", a.output, {}, "cpu")
        failed.extra["error"] = str(e)
        failed.write()
        print(f"nils-bodypart {a.entry}: {e}", file=sys.stderr)
        return 1
    path = run.write()
    c = run.counts()
    print(f"nils-bodypart {a.entry}: {c['succeeded']} succeeded, {c['skipped']} skipped, {c['failed']} failed; {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

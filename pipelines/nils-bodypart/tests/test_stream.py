# SPDX-License-Identifier: AGPL-3.0-only
"""The overlapped cascade's start (the image-model speed study, A5.2.3e):
the manifest read as a stream, its file paths joined as text, the stack's
geometry planned without the plane points, and the pool that holds every
read until the student has loaded. Each gives what the whole-file and
full-plan code gives, value for value."""

from __future__ import annotations

import json
import random
import time
from pathlib import Path

import numpy as np
import pytest

pytest.importorskip("lightgbm")
pytest.importorskip("safetensors")

from nils_bodypart import cascade, fewslice, fusion, manifest, reduced, scoring  # noqa: E402

from . import cascade_data as cd  # noqa: E402
from . import fusion_data as fd  # noqa: E402


@pytest.fixture(scope="module")
def world(tmp_path_factory) -> dict:
    root = tmp_path_factory.mktemp("stream")
    return {"root": root, **cd.write_stacks(root)}


def _with_context(world, tmp_path: Path) -> Path:
    """The test manifest without stack 22, which the context lists, beside a
    context stack the run holds itself and one listed twice."""
    doc = json.loads(world["stacks"].read_text())
    gone = next(s for s in doc["stacks"] if s["stack_id"] == 22)
    doc["stacks"] = [s for s in doc["stacks"] if s["stack_id"] != 22]
    ctx = {"stack_id": 22, "modality": gone["modality"], "geometry": gone["geometry"], "files": [{"geometry": f["geometry"]} for f in gone["files"]]}
    held = dict(ctx, stack_id=21)
    doc["context"] = [ctx, held, dict(ctx)]
    path = tmp_path / "with-context.json"
    path.write_text(json.dumps(doc, separators=(",", ":")))
    return path


def _whole(path: Path, root: Path) -> list:
    doc = manifest.read(path)
    stacks = manifest.parse(doc, source_root=root)
    return [("stack", s) for s in stacks] + [("context", c) for c in manifest.context(doc, skip={s.stack_id for s in stacks})]


def _same(a: list, b: list) -> None:
    assert [(k, s.stack_id) for k, s in a] == [(k, s.stack_id) for k, s in b]
    for (_, x), (_, y) in zip(a, b):
        assert x == y, x.stack_id


@pytest.mark.parametrize("piece", [1, 7, 4096, 1 << 22])
def test_the_streamed_manifest_is_the_whole_files(world, tmp_path, piece):
    """``stream`` gives the stacks ``parse`` gives and the context stacks
    ``context`` gives, one at a time, whatever the size of the pieces it
    reads, compact or pretty-printed."""
    root = world["source_root"]
    for path in (world["stacks"], _with_context(world, tmp_path)):
        for indent in (None, 2):
            doc = json.loads(path.read_text())
            p = tmp_path / f"m-{indent}.json"
            p.write_text(json.dumps(doc, indent=indent))
            _same(list(manifest.stream(p, source_root=root, piece=piece)), _whole(p, root))


def test_a_manifest_in_another_order_is_read_whole(world, tmp_path):
    root = world["source_root"]
    doc = json.loads(_with_context(world, tmp_path).read_text())
    for order in (["context", "stacks", "sources", "contract"], ["stacks", "sources"], ["contract", "stacks", "sources", "context"]):
        p = tmp_path / f"order-{'-'.join(order)}.json"
        p.write_text(json.dumps({k: doc[k] for k in order if k in doc}))
        _same(list(manifest.stream(p, source_root=root, piece=64)), _whole(p, root))


@pytest.mark.parametrize(
    "text",
    [
        '{"sources": [{"id": 0, "mount": "/s/0"}], "stacks": [{"stack_id": 1}, {"stack_id": 1}]}',
        '{"sources": [{"id": 0, "mount": "/s/0"}], "stacks": [{"stack_id": 1}]} trailing',
        '{"sources": [{"id": 0, "mount": "/s/0"}], "stacks": [{"stack_id": 1},',
        '{"sources": [{"id": 0, "mount": "/s/0"}], "stacks": [{"stack_id": 1, "files": [{"source": 0, "path": "../x"}]}]}',
        '{"sources": [{"id": 0, "mount": "/s/0"}]}',
        "not json",
        "",
    ],
)
def test_a_damaged_manifest_is_refused_as_the_whole_file_is(tmp_path, text):
    p = tmp_path / "bad.json"
    p.write_text(text)
    with pytest.raises(manifest.ManifestError):
        list(manifest.stream(p, piece=8))
    with pytest.raises(manifest.ManifestError):
        manifest.load(p)


def test_a_path_is_joined_as_pathlib_joins_it():
    rng = random.Random(55)
    rels = ["a/b.dcm", "a//b", "./a", "a/./b", "a/.", "a/", "a/..", "../a", "/abs", "a b/c.d", "..a/b..", ".hidden/x", "", ".", "a/.../b"]
    rels += ["".join(rng.choice("ab./") for _ in range(rng.randint(0, 8))) for _ in range(3000)]
    for mount in (Path("/source/0"), Path("/"), Path("rel/m"), Path("."), Path("/a/b/")):
        for rel in rels:
            p = Path(rel)
            want = None if p.is_absolute() or ".." in p.parts else str(mount / p)
            try:
                got = manifest._safe_join(mount, rel, manifest._base(mount))
            except manifest.ManifestError:
                got = None
            assert got == want, (str(mount), rel)


def test_the_geometry_plan_is_the_full_plans_geometry(world, monkeypatch):
    """Stage 1's registry inputs read the touched plan's geometry only: the
    plan without its plane points gives every geometry number and header
    feature of the full one, and refuses what it refuses."""
    inp = fd.write_inputs(world["root"] / "inputs-geo")
    model = fusion.load(inp["inputs"])
    stacks = manifest.load(world["stacks"], source_root=world["source_root"])
    lite = {s.stack_id: fewslice.registry_inputs(s, model) for s in stacks}
    monkeypatch.setattr(reduced, "geometry", lambda frames, orientation: reduced.plan(frames, orientation, "touched"))
    full = {s.stack_id: fewslice.registry_inputs(s, model) for s in stacks}
    assert {k for k, v in full.items() if v is None} == {k for k, v in lite.items() if v is None}
    assert sum(v is not None for v in full.values()) >= 7
    for sid, f in full.items():
        if f is None:
            continue
        g = lite[sid]
        assert np.array_equal(f[0], g[0]) and np.array_equal(f[1], g[1]) and f[2:] == g[2:], sid
    # a degenerate geometry is refused by both
    st = next(s for s in stacks if s.stack_id == 21)
    frames = reduced.stack_frames(st.files, st.file_geo, st.extra.get("geometry"), {})
    for f in frames:
        f.ps = [0.0, 0.0]
    for fn in (lambda: reduced.plan(frames, "axial", "touched"), lambda: reduced.geometry(frames, "axial")):
        with pytest.raises(np.linalg.LinAlgError):
            fn()


def test_slot_zero_is_its_choice_placed_by_the_session(world):
    stacks = manifest.load(world["stacks"], source_root=world["source_root"])
    rows = [fewslice.row_of(s) for s in stacks]
    fewslice.session_tops(rows)
    seen = 0
    for g in rows:
        s0 = fewslice.slot0(g)
        c = fewslice.slot0_choice(g)
        assert (s0 is None) == (c is None)
        if s0 is not None:
            i, geo = fewslice.slot0_geo(g, c)
            assert i == s0[0] and np.array_equal(geo, s0[1])
            seen += 1
    assert seen >= 6


def test_the_pool_holds_every_read_until_it_is_opened(world):
    """The overlap's pool plans before anything is read: work that reads
    waits until ``open_reads`` (the student has loaded), and the planning
    goes first."""
    inp = fd.write_inputs(world["root"] / "inputs-lane")
    ladder = {"stages": [("b24", 0.5), ("touched", None)], "thresholds": {"fine": 0.5, "coarse": 0.5}}
    model = fusion.load(inp["inputs"])
    lane = scoring.Lane(model, inp["inputs"], workers=2, device="cpu", chunk=4, depth=1, ladder=ladder, gpu_workers=0, hold_reads=True)
    try:
        held = lane.read(abs, -3)
        planned = lane.first(abs, -2)
        assert planned.result(timeout=120) == 2
        time.sleep(0.5)
        assert not held.done()
        lane.open_reads()
        assert held.result(timeout=120) == 3
    finally:
        lane.close()


def test_a_student_that_cannot_load_fails_the_overlapped_run(world, tmp_path, monkeypatch):
    from nils_bodypart import cli, student

    class Broken:
        def __init__(self, *a, **k):
            raise student.StudentError("the student is not a safetensors file")

    monkeypatch.setattr(student, "Student", Broken)
    inp = cd.add_cascade(fd.write_inputs(tmp_path), deferral="all")
    code = cli.main(["infer-fusion", "--stacks", str(world["stacks"]), "--source-root", str(world["source_root"]), "--inputs", str(inp["inputs"]),
                     "--output", str(tmp_path / "out"), "--device", "cpu", "--threads", "2", "--overlap", "on"])
    r = json.loads((tmp_path / "out" / "results.json").read_text())
    assert code == 1 and "safetensors" in r["error"] and not r["units"]


def test_the_overlap_reads_a_session_from_its_context(world, tmp_path, monkeypatch):
    """A run that holds part of a session, streamed: every stack's table,
    unit and proposal is the sequential run's, in the same order, each
    table written as its stack's outcome was final, the context's stack 22
    among the session's top."""
    from nils_bodypart import cli, student

    class StandIn:
        def __init__(self, path, device="cpu", threads=None):
            pass

        def predict(self, tiles, sgeo, tokens, batch=64):
            return cd.stand_in_student(tiles, sgeo, tokens)

    monkeypatch.setattr(student, "Student", StandIn)
    inp = cd.add_cascade(fd.write_inputs(tmp_path / "in"), deferral="mixed")
    page = _with_context(world, tmp_path)
    got = {}
    for mode in ("off", "on"):
        out = tmp_path / mode
        code = cli.main(["infer-fusion", "--stacks", str(page), "--source-root", str(world["source_root"]), "--inputs", str(inp["inputs"]),
                         "--output", str(out), "--device", "cpu", "--threads", "2", "--student-batch", "2", "--overlap", mode])
        assert code == 0
        r = json.loads((out / "results.json").read_text())
        got[mode] = ({int(p.stem): json.loads(p.read_text()) for p in (out / "bodypart-fusion").glob("*.json")}, r["metrics"], r["proposals"], r["units"],
                     r["counts"])
    assert got["on"] == got["off"]
    assert got["on"][1]["stage1"]["context_stacks"] == 1 and got["on"][1]["stages"]["s1"] >= 1


def test_the_cascades_plan_is_the_sequential_loops(world):
    """``plan_stack`` decides each stack as the cascade did before it was a
    function: every reason and stage-1 ending of the test stacks."""
    inp = cd.add_cascade(fd.write_inputs(world["root"] / "inputs-plan"))
    model = fusion.load(inp["inputs"])
    stacks = manifest.load(world["stacks"], source_root=world["source_root"])
    got = {s.stack_id: cascade.plan_stack(s, fewslice.row_of(s), model)[0] for s in stacks}
    assert got == {21: None, 22: None, 23: None, 24: None, 25: None, 26: "sealed", 27: "not-mr", 28: "no-geometry", 29: "multi-frame", 30: None,
                   31: None}


def _leaning(tiles, sgeo, tokens):
    """A stand-in student that leans on the heights relative to the
    session's top (token 23, the slice's height), so a batch scored with
    another top gives other numbers."""
    base = cd.stand_in_student(tiles, sgeo, tokens).astype(np.float64)
    shift = (tokens[:, 23] * 40.0 + sgeo[:, 1] * 25.0)[:, None, None]
    z = np.log(np.clip(base, 1e-9, 1)) + shift * np.array([1.0, -1.0, 0.5, 0.0, 0.0, 0.0])[None, None, :]
    e = np.exp(z - z.max(-1, keepdims=True))
    return (e / e.sum(-1, keepdims=True)).astype(np.float32)


def _key(o):
    r = o.result
    return (o.reason, o.stage1, o.d3, o.why, None if r is None else (r.stage, r.answers, r.image.tolist(), r.cohort, r.reader))


@pytest.mark.parametrize("late", ["stack", "context"])
def test_a_batch_scored_before_its_sessions_top_is_scored_again(world, tmp_path, monkeypatch, late):
    """The student scores a batch with each session's top so far; the
    session's top (stack 22) comes after it, last among the stacks or in
    the context. The batches it moves are scored again, the same batches,
    and every outcome is the sequential path's."""
    import threading

    inp = cd.add_cascade(fd.write_inputs(tmp_path), deferral="mixed")
    model = fusion.load(inp["inputs"])
    cas = cascade.load(inp["inputs"], model)
    doc = json.loads(world["stacks"].read_text())
    stacks = manifest.parse(doc, source_root=world["source_root"])
    top = next(s for s in stacks if s.stack_id == 22)
    run = [s for s in stacks if s.stack_id != 22] + ([top] if late == "stack" else [])
    context = [] if late == "stack" else manifest.context({"context": [{"stack_id": 22, "modality": "MR", "geometry": next(
        x for x in doc["stacks"] if x["stack_id"] == 22)["geometry"], "files": [{"geometry": f["geometry"]} for f in next(
            x for x in doc["stacks"] if x["stack_id"] == 22)["files"]]}]})
    scored = threading.Event()

    def student(tiles, sgeo, tokens):
        scored.set()
        return _leaning(tiles, sgeo, tokens)

    want = cascade.score(model, cas, inp["inputs"], run, workers=2, student_factory=student, student_batch=2, context=context)

    def items():
        for s in run[:-1] if late == "stack" else run:
            yield "stack", s
        scored.wait(120)  # the top comes once the student has scored a batch
        yield (late, top if late == "stack" else context[0])

    monkeypatch.setattr(cascade, "STREAM_LAG", 0)
    monkeypatch.setattr(cascade, "STREAM_BLOCK", 1)
    scored.clear()
    stats: dict = {}
    got = cascade.score_stream(model, cas, inp["inputs"], items(), workers=2, student_factory=student, student_batch=2, stats=stats)
    assert stats["overlap_batches_again"] >= 1
    assert [o.stack.stack_id for o in got] == [o.stack.stack_id for o in want]
    for a, b in zip(got, want):
        assert _key(a) == _key(b), a.stack.stack_id
    assert sum(o.result is not None and o.result.stage == "s1" for o in got) >= 1


@pytest.mark.parametrize("flip", ["to-accepted", "to-deferred"])
def test_a_decision_the_final_top_changes_is_the_sequential_one(world, tmp_path, monkeypatch, flip):
    """D3 by the seeds' spread (one of its features, read off the student's
    numbers): stack 21's spread with the session's top so far lies on the
    other side of the threshold than with the whole session's. Scored
    again, stack 21 ends as the sequential path ends it: answered by stage 1
    with the planes it was first sent to dropped, or sent to the planes
    after stage 1 first answered it."""
    import threading

    inp = cd.add_cascade(fd.write_inputs(tmp_path), deferral="mixed")
    model = fusion.load(inp["inputs"])
    cas = cascade.load(inp["inputs"], model)
    stacks = manifest.load(world["stacks"], source_root=world["source_root"])
    top = next(s for s in stacks if s.stack_id == 22)
    run = [s for s in stacks if s.stack_id != 22] + [top]
    seen: list = []
    real = cascade.defer_scores

    def recorded(c, X):
        seen.extend(np.asarray(X)[:, 13].tolist())
        return real(c, X)

    monkeypatch.setattr(cascade, "defer_scores", recorded)
    cascade.score(model, cas, inp["inputs"], run, workers=1, student_factory=_leaning, student_batch=2)
    final = seen[0]  # stack 21 is the first candidate
    seen.clear()
    cascade.score(model, cas, inp["inputs"], run[:-1], workers=1, student_factory=_leaning, student_batch=2)
    early = seen[0]
    assert early != final
    t = (early + final) / 2
    accept_below = (final < t) == (flip == "to-accepted")

    def by_spread(c, X):
        s = np.asarray(X)[:, 13]
        return np.where((s < t) == accept_below, 1.0, 0.0)

    monkeypatch.setattr(cascade, "defer_scores", by_spread)
    want = cascade.score(model, cas, inp["inputs"], run, workers=2, student_factory=_leaning, student_batch=2)
    w21 = next(o for o in want if o.stack.stack_id == 21)
    assert (w21.stage1 == "answered") == (flip == "to-accepted")
    scored = threading.Event()

    def student(tiles, sgeo, tokens):
        scored.set()
        return _leaning(tiles, sgeo, tokens)

    def items():
        for s in run[:-1]:
            yield "stack", s
        scored.wait(120)
        yield "stack", top

    monkeypatch.setattr(cascade, "STREAM_LAG", 0)
    monkeypatch.setattr(cascade, "STREAM_BLOCK", 1)
    stats: dict = {}
    got = cascade.score_stream(model, cas, inp["inputs"], items(), workers=2, student_factory=student, student_batch=2, stats=stats)
    assert stats["overlap_batches_again"] >= 1
    for a, b in zip(got, want):
        assert _key(a) == _key(b), a.stack.stack_id
    g21 = next(o for o in got if o.stack.stack_id == 21)
    assert g21.result is not None and g21.result.stage == ("s1" if flip == "to-accepted" else w21.result.stage)

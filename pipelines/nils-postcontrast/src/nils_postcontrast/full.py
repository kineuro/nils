# SPDX-License-Identifier: AGPL-3.0-only
"""``nils-postcontrast full``: the full post-contrast pipeline behind the
cascade (``postcontrast-full``, record 55 E3, spec 12.2), model r1 itself
on the stacks the three-slice student defers.

Per stack the run is model r1's: the stack converted from its files
(dcm2niix, retried once), placed on the MNI template by SynthMorph's affine
with the template regions and the venous regions carried onto it,
segmented by SynthSeg and skull-stripped by SynthStrip; then the regions'
and the structures' intensities, a Frangi vessel filter and the stripped
stack's intensity tail (without N4, round 2's class II candidate); the
three frozen arms; and model r1's fusion with the stack's header evidence,
calibrated per site, as the cascade's first stage uses it.

Who is a candidate is the cascade's rule (T1-type brain at a calibrated
site); every other stack is "not sure" with its reason. A stack whose
tools fail keeps the arms it has, as model r1 does; but a stack left with
no image arm at all, for any reason, is "not sure": no usable volume (no
conversion, fewer than 3 slices, 4D, ``no-usable-volume``), no
segmentation after the card, its retry and the CPU (``no-segmentation``),
or no arm its features could feed (a failed feature step, too few voxels,
``no-image-arm``), and where the run itself failed on it, a worker that
died (``worker-died``) or a step that raised (``step-failed``), 2026-10-15.
Model r1 would answer it from its header
evidence alone, which no test has compared with a reference, and the model
answers only where it is certified (record 55 E3). The change is class III
by construction.

With ``--raw-scores`` (0.3.0) a T1-type brain stack at a site with no
calibration is converted, segmented and scored too, and its table carries
the fused log-odds before any calibration (``score_raw``) beside the
``no-calibration`` reason where it has an image arm; its ``p_given`` and
``value`` stay empty, so it is never a proposal. A run given a calibration
file (``--calibration``, or the descriptor's site file through
``NILS_PC_CALIBRATION``) merges the file's site pairs over the fusion's
first (``calibration.py``).

**Where it runs.** With a card, the tools run in waves on it (``tools.wave``)
while worker processes convert the stacks before and compute the features
after; without one, every tool runs on the CPU, at most as many SynthSeg
processes at once as the memory holds (one per 96 GB). On a card the run
holds at most its budget, which is the memory the engine leased the unit
(``NILS_GPU_MEMORY_GB``) unless one is given: a wave runs as many SynthSeg
processes as fit beside SynthMorph and SynthStrip, and the Frangi filter
goes on the card only where every worker's share fits beside a full wave
(``card_plan``). A smaller card runs fewer SynthSeg processes, then
SynthSeg alone after each wave, then the CPU. A worker that dies (its memory,
a signal) takes its pool with it: the pool is made again and the stacks
it held are tried again, so one stack never ends the run.
"""

from __future__ import annotations

import base64
import hashlib
import json
import logging
import multiprocessing
import os
import queue
import shutil
import sys
import tempfile
import threading
import time
from concurrent.futures import FIRST_COMPLETED, ProcessPoolExecutor, wait
from concurrent.futures.process import BrokenProcessPool
from pathlib import Path

import numpy as np

from . import AXIS, GIVEN, NOT_GIVEN, calibration, evidence, features, manifest, parts, tools
from .arms import Arms, ArmsError
from .fusion import Fusion, FusionError, calibrated, log_odds
from .results import FAILED, SKIPPED, Run, Unit

logger = logging.getLogger("nils_postcontrast.full")
VALUE = {"given": GIVEN, "not given": NOT_GIVEN}
NO_VOLUME, NO_SEGMENTATION, NO_ARM = "no-usable-volume", "no-segmentation", "no-image-arm"
WORKER_DIED, STEP_FAILED = "worker-died", "step-failed"
# what the conversion says of the data itself: no usable volume
DATA_FAILS = ("dcm2niix", "4D", "fewer than 3 slices")


def killed(rc) -> bool:
    """A tool's return code says the kernel or a signal ended it (a
    negative code from Python, or the shell's 128 and up, 137 the memory
    limit's), not that it judged its input."""
    return isinstance(rc, int) and (rc < 0 or rc >= 128) and rc != 127


def no_arm_reason(arms: str, convert_fail: str | None, segmented: bool, died: bool = False,
                  step_failed: bool = False) -> str | None:
    """Why a candidate is "not sure" though model r1 could answer it from
    its header evidence alone: no image arm at all, for any reason (record
    55 E3). The run's own failures are told from the data's: a worker that
    died on the stack (its memory, a signal) is ``worker-died`` and a step
    that raised is ``step-failed``; only what the data itself lacks is
    ``no-usable-volume`` (no conversion, fewer than 3 slices, 4D) or
    ``no-segmentation``; else ``no-image-arm`` (no arm its features could
    feed). A tool the kernel or a signal ended (dcm2niix, a CPU Synth tool)
    is a dead worker's; a CPU tool step that raised is ``step-failed``,
    before any missing segmentation it caused. None where it keeps an arm."""
    if arms:
        return None
    if died or convert_fail == WORKER_DIED:
        return WORKER_DIED
    if convert_fail:
        return NO_VOLUME if convert_fail in DATA_FAILS else STEP_FAILED
    if step_failed:
        return STEP_FAILED
    if not segmented:
        return NO_SEGMENTATION
    return NO_ARM
CPU_SYNTHSEG_GB = 96  # the most one CPU SynthSeg process was seen to hold: 85 GB on a large deferred volume


class FullParts:
    """The run's model inputs: the fusion, the arms (with the atlases they
    were fitted on) and the cascade that names both."""

    def __init__(self, inputs: Path):
        ps = parts.parts(inputs)
        for need in ("fusion", "arms", "cascade"):
            if need not in ps:
                raise parts.PartsError(f"the run was given no {need} model")
        self.cascade, self.fusion, self.arms_part = (parts.checked(ps[k]) for k in ("cascade", "fusion", "arms"))
        try:
            doc = json.loads(self.cascade.artifact.read_text(encoding="utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as e:
            raise parts.PartsError("the cascade is not JSON") from e
        if not isinstance(doc, dict) or not isinstance(doc.get("parts"), dict):
            raise parts.PartsError("the cascade file names no parts")
        if doc["parts"].get("fusion") != self.fusion.digest:
            raise parts.PartsError("the cascade names another fusion than the one the run was given")
        cp = self.cascade.card.get("parts")
        if isinstance(cp, dict):
            for k, part in (("fusion", self.fusion), ("arms", self.arms_part)):
                if k in cp and cp[k] != part.digest:
                    raise parts.PartsError(f"the cascade's card names another {k} than the one the run was given")
        self.F = Fusion.load(self.fusion.artifact)
        ct = self.cascade.card.get("threshold")
        if isinstance(ct, bool) or not isinstance(ct, (int, float)) or abs(float(ct) - self.F.t) > 1e-9:
            raise parts.PartsError("the cascade's card names no threshold, or not the fusion's")
        self.A = Arms.load(self.arms_part.artifact)
        fdoc = json.loads(self.fusion.artifact.read_text(encoding="utf-8"))
        if fdoc.get("source_sha256") and self.A.source_sha256 and fdoc["source_sha256"] != self.A.source_sha256:
            raise parts.PartsError("the arms and the fusion are not of the same frozen model")
        self.atlas_doc = json.loads(self.arms_part.artifact.read_text(encoding="utf-8")).get("atlas") or {}
        for k in ("HP", "VX"):
            a = self.atlas_doc.get(k)
            if not isinstance(a, dict) or not a.get("nifti_gz") or not isinstance(a.get("names"), dict):
                raise parts.PartsError(f"the arms carry no {k} atlas")

    def write_atlas(self, folder: Path) -> tuple[dict, dict, dict]:
        """The atlases as the files SynthMorph places, each checked against
        its sha256; their region names."""
        folder.mkdir(parents=True, exist_ok=True)
        out = {}
        for k in ("HP", "VX"):
            a = self.atlas_doc[k]
            data = base64.b64decode(a["nifti_gz"])
            if a.get("sha256") and hashlib.sha256(data).hexdigest() != a["sha256"]:
                raise parts.PartsError(f"the {k} atlas is not the one its sha256 names")
            p = folder / f"{k.lower()}_mni.nii.gz"
            p.write_bytes(data)
            out[k] = str(p)
        return out, self.atlas_doc["HP"]["names"], self.atlas_doc["VX"]["names"]


# ---------- the workers ----------

_W: dict = {}


def _init(env: tools.Env, frangi: str, cards: str) -> None:
    os.environ["CUDA_VISIBLE_DEVICES"] = cards if frangi == "card" else ""
    _W["env"] = env
    if frangi == "card":
        features.frangi_on_card()


def _convert(sid: int, d: str, files: list) -> dict:
    t0 = time.time()
    try:
        info = tools.convert(_W["env"], d, files)
        # dcm2niix the kernel or a signal ended (its memory) on its last
        # attempt is the run's failure, not the data's
        last = info.get("rc2", info.get("rc1"))
        ended = killed(last)
        why = tools.check_native(d) if info.get("ok") else (WORKER_DIED if ended else "dcm2niix")
    except Exception as e:  # noqa: BLE001
        info, why = {"ok": False}, f"convert: {type(e).__name__}"
    return {"stack_id": sid, "convert": info, "fail": why, "s": round(time.time() - t0, 2)}


def _cpu(sid: int, d: str, atlas: dict, which: list) -> dict:
    t0 = time.time()
    try:
        r = tools.cpu_tools(_W["env"], d, atlas, which)
    except Exception as e:  # noqa: BLE001
        r = {"error": type(e).__name__}
    return {"stack_id": sid, "cpu": r, "s": round(time.time() - t0, 2)}


def _finish(sid: int, d: str, hpn: dict, vxn: dict) -> dict:
    t0 = time.time()
    b = features.base(d, hpn, vxn)
    st = tools.paths(d)["strip"]
    t = features.tail(st) if os.path.exists(st) else {"ok": False, "error": "synthstrip"}
    return {"stack_id": sid, "base": b, "t": t, "cols": features.columns(b, t), "s": round(time.time() - t0, 2),
            "frangi_card_failed": features._FRANGI.get("card_failed", 0)}


def _marked(marks: str, key, fn, *args):
    """A task in a worker, its key marked while it runs, so a pool that
    breaks tells which tasks were running (the suspects) from those that
    only waited."""
    m = os.path.join(marks, str(key))
    open(m, "w").close()
    try:
        return fn(*args)
    finally:
        try:
            os.remove(m)
        except OSError:
            pass


class Lane:
    """A pool of worker processes whose tasks survive a worker's death: the
    pool is made again and its unfinished tasks given again. A task that was
    running when the pool broke has one try less, one that only waited
    keeps its tries; a task out of tries is dead."""

    def __init__(self, n: int, initargs: tuple, tries: int = 3):
        self.n, self.initargs, self.tries = max(1, n), initargs, tries
        self.ctx = multiprocessing.get_context("spawn")
        self.marks = tempfile.mkdtemp(prefix="nils-pc-lane-")
        self.pool = self._make()
        self.pending: dict = {}  # future -> (key, fn, args, tries left)
        self.rebuilt = 0

    def _make(self):
        return ProcessPoolExecutor(self.n, mp_context=self.ctx, initializer=_init, initargs=self.initargs)

    def submit(self, key, fn, *args, tries: int | None = None) -> None:
        left = self.tries if tries is None else tries
        try:
            f = self.pool.submit(_marked, self.marks, key, fn, *args)
        except BrokenProcessPool:
            self._rebuild()
            f = self.pool.submit(_marked, self.marks, key, fn, *args)
        self.pending[f] = (key, fn, args, left)

    def _rebuild(self) -> list:
        self.rebuilt += 1
        old = self.pending
        self.pending = {}
        try:
            self.pool.shutdown(wait=False, cancel_futures=True)
        except Exception:  # noqa: BLE001
            pass
        running = set(os.listdir(self.marks))
        for name in running:
            try:
                os.remove(os.path.join(self.marks, name))
            except OSError:
                pass
        self.pool = self._make()
        dead = []
        for f, (key, fn, args, left) in old.items():
            if f.done() and not f.cancelled() and f.exception() is None:
                self.pending[f] = (key, fn, args, left)  # finished before the break: kept
                continue
            left = left - 1 if str(key) in running else left
            if left > 0:
                self.submit(key, fn, *args, tries=left)
            else:
                dead.append(key)
        return dead

    def done(self, timeout: float) -> tuple[list, list]:
        """(key, result) of the tasks that finished, and the keys of those
        that died out of tries."""
        if not self.pending:
            return [], []
        fin, _ = wait(list(self.pending), timeout=timeout, return_when=FIRST_COMPLETED)
        out, dead = [], []
        broken = False
        for f in fin:
            key, fn, args, left = self.pending[f]
            try:
                r = f.result()
            except BrokenProcessPool:
                broken = True
                continue
            except Exception as e:  # noqa: BLE001 - the task's own error
                r = {"stack_id": key, "error": type(e).__name__}
            del self.pending[f]
            out.append((key, r))
        if broken:
            dead += self._rebuild()
        return out, dead

    def shutdown(self) -> None:
        self.pool.shutdown(wait=True, cancel_futures=True)
        shutil.rmtree(self.marks, ignore_errors=True)


# ---------- the run ----------


def candidates(stacks, context, F: Fusion, features_all: bool = False, raw: bool = False) -> tuple[dict, dict, list]:
    """(the header rows by stack, the reasons of the stacks that are not
    candidates, the entries whose features are computed: the candidates', or
    with ``features_all`` every unsealed MR stack's with files, a research
    check's, whose answers still follow the rule; with ``raw``, the
    raw-score mode's, the T1-type brain stacks with files at a site with no
    calibration too, for their log-odds alone)."""
    rows = evidence.rows_for(stacks, context, F)
    why: dict = {}
    todo = []
    for e in stacks:
        r = rows[e.stack_id]
        mod = e.modality or (e.header.get("fingerprint") or {}).get("modality")
        if e.sealed:
            why[e.stack_id] = "sealed"
        elif str(mod or "").upper() != "MR":
            why[e.stack_id] = "not-mr"
        elif not (r["acquired"] and r["family"] == "T1-type" and r["brain"]):
            why[e.stack_id] = "not-t1-brain"
        elif r["site"] is None or r["site"] not in F.cal:
            why[e.stack_id] = "no-calibration"
        elif not e.files:
            why[e.stack_id] = "no-files"
        if e.stack_id not in why or (features_all and why[e.stack_id] in ("not-t1-brain", "no-calibration") and e.files) \
                or (raw and why[e.stack_id] == "no-calibration" and e.files):
            todo.append(e)
    return rows, why, todo


def frangi_card_env(env: tools.Env, work: Path) -> None:
    """What the feature workers need to run the Frangi filter on the card,
    set before they start. CuPy compiles the filter's kernels with NVRTC:
    the CUDA 12 libraries beside the card's TensorFlow go on the path the
    workers start with. CuPy also writes every kernel it compiles to a
    cache, ``~/.cupy/kernel_cache`` unless ``CUPY_CACHE_DIR`` says
    otherwise; in a container the home is often missing on a read-only
    root (apptainer ``--no-home``, an image's nobody user), and every
    stack that reached the filter lost arms U and V to the ``OSError``
    (2026-10-16, the fallback's image on a card). The cache goes in the
    run's work folder, which is on disk and writable, unless the caller
    named one."""
    site = tools._site_nvidia(env.tf_python) if env.tf_python else ""
    libs = tools._nvidia_libs(site)
    if libs:
        os.environ["LD_LIBRARY_PATH"] = libs + (":" + os.environ["LD_LIBRARY_PATH"] if os.environ.get("LD_LIBRARY_PATH") else "")
        os.environ["CUDA_PATH"] = os.path.join(site, "cuda_runtime")
    if not os.environ.get("CUPY_CACHE_DIR"):
        cache = Path(work).absolute() / "cupy-cache"
        cache.mkdir(parents=True, exist_ok=True)
        os.environ["CUPY_CACHE_DIR"] = str(cache)


def features_of(todo: list, work: Path, atlas: dict, hpn: dict, vxn: dict, env: tools.Env, cards: list, opts: dict) -> tuple[dict, dict]:
    """Every candidate's arm columns and what happened to it, by stack id;
    and the run's counts."""
    stats: dict = {"pool_rebuilt": 0}
    rec: dict = {e.stack_id: {"stage": {}} for e in todo}
    files = {e.stack_id: [p for p, _g, _f in manifest_files(e)] for e in todo}
    sd = {sid: str(work / "stacks" / str(sid)) for sid in files}
    logdir = work / "card"
    logdir.mkdir(parents=True, exist_ok=True)
    on_card = bool(cards)
    frangi = opts["frangi"]
    if frangi == "auto":
        frangi = "card" if on_card else "cpu"
    card_names = ",".join(c.device for c in cards)
    if frangi == "card":
        frangi_card_env(env, work)
    heavy = max(1, min(opts["workers"], opts["memory_gb"] // CPU_SYNTHSEG_GB))
    lane_a = Lane(opts["convert_workers"], (env, "cpu", ""))
    lane_c = Lane(opts["workers"], (env, frangi, card_names))
    lane_h = Lane(heavy, (env, "cpu", ""))
    lock = threading.Lock()
    waves: queue.Queue = queue.Queue()
    after_card: queue.Queue = queue.Queue()
    card_s: dict = {}
    retry_s: dict = {"segmented": 0}

    def card_worker(card: tools.Card) -> None:
        while True:
            w = waves.get()
            if w is None:
                return
            try:
                r = tools.wave(env, card, w, str(work), atlas, str(logdir), opts["synthseg_procs"], opts["synthseg_mib"], opts["other_mib"], lock, stats,
                               keep_free=opts["keep_free_mib"])
                for k, v in r.items():
                    card_s[k] = card_s.get(k, 0.0) + v
                retry_s["segmented"] += tools.synthseg_retry(env, card, w, str(work), str(logdir), opts["synthseg_retry_mib"], lock,
                                                             keep_free=opts["keep_free_mib"])
            except Exception as e:  # noqa: BLE001 - what the card left undone is the CPU's
                stats["card_error"] = type(e).__name__
            for sid in w:
                after_card.put(sid)

    threads = []
    if on_card:
        for c in cards:
            for _ in range(max(1, opts["card_waves"])):
                th = threading.Thread(target=card_worker, args=(c,), daemon=True)
                th.start()
                threads.append(th)
    for sid in files:
        lane_a.submit(sid, _convert, sid, sd[sid], files[sid])
    buf: list = []
    converting = len(files)
    in_card = 0
    finished = 0
    total = len(files)
    t_last = time.time()
    while finished < total:
        progressed = False
        # conversions
        out, dead = lane_a.done(0.2)
        for sid, r in out:
            converting -= 1
            progressed = True
            rec[sid]["convert"] = r.get("convert")
            rec[sid]["stage"]["convert"] = r.get("s")
            if r.get("fail") or r.get("error"):
                rec[sid]["fail"] = r.get("fail") or r.get("error")
                lane_c.submit(sid, _finish, sid, sd[sid], hpn, vxn)  # the arms it can have (none), as model r1
                continue
            if on_card:
                buf.append(sid)
            else:
                lane_h.submit(sid, _cpu, sid, sd[sid], atlas, tools.missing(sd[sid]))
        for sid in dead:
            converting -= 1
            rec[sid]["fail"] = WORKER_DIED
            lane_c.submit(sid, _finish, sid, sd[sid], hpn, vxn)
        if on_card and buf and (len(buf) >= opts["wave"] or converting == 0):
            while buf:
                w, buf = sorted(buf[: opts["wave"]]), buf[opts["wave"]:]
                waves.put(w)
                in_card += len(w)
        # the card's waves
        while True:
            try:
                sid = after_card.get_nowait()
            except queue.Empty:
                break
            progressed = True
            in_card -= 1
            m = tools.missing(sd[sid])
            if m:
                rec[sid]["cpu_tools"] = m
                lane_h.submit(sid, _cpu, sid, sd[sid], atlas, m)
            else:
                lane_c.submit(sid, _finish, sid, sd[sid], hpn, vxn)
        # the CPU's tools
        out, dead = lane_h.done(0.05)
        for sid, r in out:
            progressed = True
            rec[sid]["stage"]["cpu_tools"] = r.get("s")
            cpu = r.get("cpu") or {}
            if cpu.get("error"):
                rec[sid]["tools_error"] = cpu["error"]
            if any(killed(v) for v in (cpu.get("rc") or {}).values()):
                rec[sid]["tools_killed"] = True
            lane_c.submit(sid, _finish, sid, sd[sid], hpn, vxn)
        for sid in dead:
            rec[sid]["tools_died"] = True
            lane_c.submit(sid, _finish, sid, sd[sid], hpn, vxn)
        # the features
        out, dead = lane_c.done(0.05)
        for sid, r in out:
            progressed = True
            finished += 1
            rec[sid]["stage"]["features"] = r.get("s")
            rec[sid]["cols"] = r.get("cols") or {}
            rec[sid]["base_fail"] = (r.get("base") or {}).get("fail")
            rec[sid]["t_fail"] = None if (r.get("t") or {}).get("ok") else ((r.get("t") or {}).get("error") or "features")
            if r.get("error"):
                rec[sid]["features_error"] = r["error"]
            stats["frangi_card_failed"] = stats.get("frangi_card_failed", 0) + int(r.get("frangi_card_failed") or 0)
        for sid in dead:
            finished += 1
            rec[sid]["cols"] = {}
            rec[sid]["features_died"] = True
        if progressed:
            t_last = time.time()
        elif time.time() - t_last > opts["stall_s"]:
            stats["stalled"] = True
            break
        else:
            time.sleep(0.2)  # nothing finished: the lanes' waits return at once when they hold nothing
    for _ in threads:
        waves.put(None)
    for th in threads:
        th.join(timeout=1 if stats.get("stalled") else None)
    for lane in (lane_a, lane_c, lane_h):
        stats["pool_rebuilt"] += lane.rebuilt
        lane.shutdown()
    stats["card_s"] = {k: round(v, 1) for k, v in card_s.items()}
    stats["synthseg_retried_on_card"] = retry_s["segmented"]
    stats["cpu_tool_stacks"] = sum(1 for r in rec.values() if r.get("cpu_tools"))
    stats["heavy_workers"] = heavy
    stats["frangi"] = frangi
    return rec, stats


def manifest_files(e) -> list:
    """The stack's files in the study's order (by instance number, then path)."""
    return [(t[0], None, None) for t in manifest.ordered_files(e)]


# a feature worker's Frangi filter on the card: its CUDA context (559 MiB
# measured on 2026-10-14) under its memory pool's cap (1,536 MiB,
# features.frangi_on_card)
FRANGI_CARD_MIB = 2200


def card_plan(cards: list, workers: int, frangi: str, ss_procs: int, ss_mib: int, other_mib: int) -> tuple[str, list]:
    """How the cards' budgets are spent (record 55 T25: a full wave, two
    SynthSeg processes of 20,000 MiB beside 20,000 for SynthMorph and
    SynthStrip, holds 60,000 MiB). The tools come first: a wave runs as many
    SynthSeg processes as its card's budget holds (``tools.ss_procs_for``),
    and a card whose budget does not hold SynthMorph and SynthStrip is left
    out, its stacks' tools run on the CPU. With ``auto`` the Frangi filter
    goes on the first card only where every feature worker's share fits
    there beside a full wave, which that card's budget then gives up; else
    it runs on the CPU, as model r1 itself ran it. ``card`` puts it there
    regardless, as research runs did. Returns the Frangi filter's place and
    the cards kept."""
    kept = [c for c in cards if c.budget_mib >= other_mib]
    if not kept:
        return ("cpu" if frangi == "auto" else frangi), kept
    if frangi == "auto":
        share = workers * FRANGI_CARD_MIB
        frangi = "card" if kept[0].budget_mib - (other_mib + ss_procs * ss_mib) >= share else "cpu"
        if frangi == "card":
            kept[0].budget_mib -= share
    return frangi, kept


def _cards(asked: str, budget_frac: float, budget_mib: int) -> list:
    if asked == "none":
        return []
    vis = tools.cards_visible()
    if asked not in ("auto", "all"):
        want = [x.strip() for x in asked.split(",") if x.strip()]
        vis = [(u, m) for u, m in vis if u in want] or [(w, 0) for w in want]
    out = []
    for u, total in vis:
        b = budget_mib or int(total * budget_frac) or 20000
        out.append(tools.Card(u, b))
    return out


def _table(sid, P: FullParts, raw: bool = False, **kw) -> dict:
    doc = {"p_given": None, "value": None, "confidence": None, "answers": 0, "stage": None, "reason": None, "arms": None, "failed_steps": None,
           "seconds": None}
    if raw:
        # the raw-score mode's columns, on every row of such a run
        doc.update(score_raw=None, site=None, make=None)
    doc.update(kw)
    doc.update(cascade_digest=P.cascade.digest, fusion_digest=P.fusion.digest, arms_digest=P.arms_part.digest, stack_id=sid)
    return doc


def cmd_full(a) -> int:
    params = {"workers": a.threads, "convert_workers": a.convert_workers, "wave": a.wave, "synthseg_procs": a.synthseg_procs, "cards": a.cards}
    raw = bool(a.raw_scores)
    if raw:
        params["raw_scores"] = True
    run = Run("postcontrast-full", a.output, params, "cpu")
    keep_work = a.keep_work
    work = (Path(a.work) if a.work else Path(tempfile.mkdtemp(prefix="nils-postcontrast-full-"))).absolute()
    try:
        P = FullParts(a.inputs)
        P.F = calibration.for_run(P.F, a.calibration, run.extra)
        stacks, context = manifest.read(a.stacks, a.source_root)
        env = tools.Env.from_environ()
        cards = _cards(a.cards, a.card_share, a.card_budget_mib) if a.device != "cpu" else []
        if cards:
            need = env.card_ready()
            if need:
                if a.device == "cuda":
                    raise RuntimeError("a card was asked for and its tools are not here: " + ", ".join(need))
                cards = []
        elif a.device == "cuda":
            raise RuntimeError("a card was asked for and none is usable")
        frangi = a.frangi
        if cards:
            frangi, cards = card_plan(cards, a.threads, a.frangi, a.synthseg_procs, a.synthseg_mib, a.other_mib)
            run.extra["card_plan"] = {"frangi": frangi, "cards": [{"card": c.device, "budget_mib": c.budget_mib,
                                      "synthseg_procs": tools.ss_procs_for(c.budget_mib, a.synthseg_procs, a.synthseg_mib, a.other_mib)}
                                     for c in cards]}
        run.device = "cuda" if cards else "cpu"
    except (parts.PartsError, FusionError, ArmsError, calibration.CalibrationError, manifest.ManifestError, RuntimeError) as e:
        run.extra["error"] = str(e)
        run.write()
        print(f"nils-postcontrast: {e}", file=sys.stderr)
        return 1
    t0 = time.time()
    rows, why, todo = candidates(stacks, context, P.F, a.features_for == "all", raw)
    by_id = {e.stack_id: e for e in stacks}

    def table(sid, **kw):
        if raw:
            kw.setdefault("site", rows[sid]["site"])
            kw.setdefault("make", evidence.make_of(by_id[sid]))
        return _table(sid, P, raw=raw, **kw)

    atlas, hpn, vxn = P.write_atlas(work / "atlas")
    opts = {"workers": a.threads, "convert_workers": a.convert_workers, "wave": a.wave, "card_waves": a.card_waves,
            "synthseg_procs": a.synthseg_procs, "synthseg_mib": a.synthseg_mib, "synthseg_retry_mib": a.synthseg_retry_mib,
            "other_mib": a.other_mib, "keep_free_mib": a.keep_free_mib, "memory_gb": a.memory_gb, "frangi": frangi, "stall_s": a.stall_s}
    rec, stats = features_of(todo, work, atlas, hpn, vxn, tools.Env.from_environ(), cards, opts) if todo else ({}, {})
    # the arms and model r1's fusion, as the cascade's first stage fuses
    ids = [e.stack_id for e in todo]
    frows = []
    for sid in ids:
        r = dict(rows[sid])
        r.update(rec[sid].get("cols") or {})
        frows.append(r)
    probs = P.A.probabilities(frows) if frows else {k: np.zeros(0) for k in "TUV"}
    z = log_odds(P.F, frows, probs) if frows else np.zeros(0)
    p, ans = calibrated(P.F, frows, z) if frows else (np.zeros(0), np.zeros(0, dtype=object))
    tables: dict = {}
    for i, sid in enumerate(ids):
        r = rec[sid]
        uncal = raw and why.get(sid) == "no-calibration"
        if sid in why and not uncal:
            continue
        armset = "".join(k for k in "TUV" if probs[k][i] == probs[k][i])
        failed = [k for k, v in (("convert", r.get("fail")), ("tools", r.get("tools_died") or r.get("tools_killed") or r.get("tools_error")),
                                 ("base", r.get("base_fail")), ("tail", r.get("t_fail")), ("features", r.get("features_died"))) if v]
        pi = float(p[i])
        a_ = str(ans[i])
        segmented = os.path.exists(tools.paths(str(work / "stacks" / str(sid)))["ss"])
        died = bool(r.get("tools_died") or r.get("features_died") or r.get("tools_killed"))
        lost = no_arm_reason(armset, r.get("fail") or None, segmented, died, bool(r.get("tools_error")))
        if uncal:
            # the raw-score mode: the log-odds alone where the stack has an
            # image arm, no probability, no answer, so never a proposal
            tables[sid] = table(sid, stage="full", reason="no-calibration", arms=armset, failed_steps=failed or None, seconds=r["stage"],
                                score_raw=None if lost else round(float(z[i]), 6))
            continue
        if lost:
            # record 55 E3: no image arm, so not where model r1 is certified
            tables[sid] = table(sid, stage="full", reason=lost, arms=armset, failed_steps=failed or None, seconds=r["stage"])
            continue
        if a_ in VALUE:
            tables[sid] = table(sid, p_given=round(pi, 6), value=VALUE[a_], confidence=round(max(pi, 1 - pi), 6), answers=1, stage="full",
                                arms=armset, failed_steps=failed or None, seconds=r["stage"])
        else:
            tables[sid] = table(sid, p_given=None if pi != pi else round(pi, 6), stage="full", reason="not-sure", arms=armset,
                                failed_steps=failed or None, seconds=r["stage"])
    for sid, w in why.items():
        if sid not in tables:
            tables[sid] = table(sid, reason=w)
    counts: dict = {}
    for e in stacks:
        doc = tables[e.stack_id]
        k = doc["reason"] or "answered"
        counts[k] = counts.get(k, 0) + 1
        body = (json.dumps(doc, indent=2) + "\n").encode()
        status = SKIPPED if doc["reason"] in ("sealed", "not-mr") else "succeeded"
        metrics = {k2: doc[k2] for k2 in ("value", "confidence", "stage", "reason", "arms")}
        if raw:
            metrics["score_raw"] = doc["score_raw"]
        u = Unit(e.unit or f"stack-{e.stack_id}", status=status, metrics=metrics)
        u.outputs.append(run.output_file(f"postcontrast/{e.stack_id}.json", body, "table", "application/json"))
        run.units.append(u)
        if doc["answers"]:
            pg = doc["p_given"]
            run.proposals.append({"stack_id": e.stack_id, "axis": AXIS, "value": doc["value"],
                                  "probabilities": {GIVEN: round(pg, 6), NOT_GIVEN: round(1 - pg, 6)}, "model_digest": P.cascade.digest})
    sec = time.time() - t0
    run.metrics = {"stacks": len(stacks), "candidates": len(todo), "by_outcome": counts, "seconds": round(sec, 3),
                   "candidates_per_s": round(len(todo) / sec, 4) if sec > 0 else None, "stats": stats}
    run.models = [P.cascade.ref(), P.fusion.ref(), P.arms_part.ref()]
    run.write()
    if a.records:
        with open(a.records, "w") as f:
            for i, sid in enumerate(ids):
                f.write(json.dumps({"stack_id": sid, **{k: v for k, v in rec[sid].items() if k != "cols"}, "cols": rec[sid].get("cols"),
                                    "p": tables[sid]["p_given"], "answer": tables[sid]["value"], "reason": tables[sid]["reason"],
                                    "arms_p": {k: (None if probs[k][i] != probs[k][i] else float(probs[k][i])) for k in "TUV"}},
                                   default=str) + "\n")
    if not keep_work:
        shutil.rmtree(work, ignore_errors=True)
    return 0


def _leased_mib() -> int:
    """The card memory the engine leased a unit on a card (NILS_GPU_MEMORY_GB,
    record 55 T25), in MiB; 0 where it leased none."""
    try:
        return max(0, int(float(os.environ.get("NILS_GPU_MEMORY_GB") or 0) * 1024))
    except ValueError:
        return 0


def add_parser(sub) -> None:
    c = sub.add_parser("full", help="model r1's full pipeline on the stacks the cascade defers")
    c.add_argument("--stacks", type=Path, default=Path("/input/stacks.json"))
    c.add_argument("--inputs", type=Path, default=Path("/inputs"))
    c.add_argument("--output", type=Path, default=Path("/output"))
    c.add_argument("--source-root", type=Path, default=None, help="where the manifest's source mounts are, outside a container")
    c.add_argument("--work", default=None, help="the stacks' work folder (default: a new one under TMPDIR, removed at the end)")
    c.add_argument("--keep-work", action="store_true")
    c.add_argument("--threads", type=int, default=12, help="worker processes computing the features")
    c.add_argument("--convert-workers", type=int, default=8)
    c.add_argument("--wave", type=int, default=24, help="stacks a card wave takes")
    c.add_argument("--card-waves", type=int, default=2, help="waves on each card at once")
    c.add_argument("--synthseg-procs", type=int, default=2, help="SynthSeg processes a wave runs")
    c.add_argument("--synthseg-mib", type=int, default=20000, help="each SynthSeg process's share of its card")
    c.add_argument("--synthseg-retry-mib", type=int, default=50000, help="the share of the card a failed segmentation's second try takes")
    c.add_argument("--other-mib", type=int, default=20000, help="a wave's SynthMorph and SynthStrip on the card, at most")
    c.add_argument("--keep-free-mib", type=int, default=0, help="a wave starts only when its card keeps this much free after it (a shared card's lease)")
    c.add_argument("--cards", default="auto", help="auto, none, or a comma list of the card ids to use")
    c.add_argument("--card-share", type=float, default=0.75, help="the share of each card's memory the run may hold")
    c.add_argument("--card-budget-mib", type=int, default=_leased_mib(),
                   help="the memory the run may hold of each card, at once (overrides the share); default: what the engine leased "
                        "the unit (NILS_GPU_MEMORY_GB)")
    c.add_argument("--memory-gb", type=int, default=64, help="the memory the run may use; CPU SynthSeg runs at most one per 96 GB of it")
    c.add_argument("--device", choices=["auto", "cpu", "cuda"], default="auto")
    c.add_argument("--frangi", choices=["auto", "cpu", "card"], default="auto",
                   help="where the Frangi filter runs; auto: on the card where every worker's share fits beside a full wave")
    c.add_argument("--stall-s", type=float, default=7200.0, help="seconds without progress before the run gives up on the rest")
    c.add_argument("--features-for", choices=["candidates", "all"], default="candidates",
                   help="all: every unsealed MR stack's features too (a research check's); the answers still follow the rule")
    c.add_argument("--records", default=None, help="a file for each candidate's record (a research run's)")
    calibration.add_run_options(c)
    c.set_defaults(fn=cmd_full)

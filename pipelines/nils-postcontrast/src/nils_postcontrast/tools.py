# SPDX-License-Identifier: AGPL-3.0-only
"""The tools the full pipeline runs on a stack, and where they run.

On the CPU each tool is FreeSurfer 8.2.0's own command, as model r1's study
ran it: dcm2niix, ``mri_synthmorph`` (the affine to the MNI template and
the two atlases placed by it), ``mri_synthseg --cpu`` and
``mri_synthstrip``.

On a card the same scripts and weights run in waves of stacks, in
long-lived processes (the post-contrast speed study's rounds 2 and 3,
class I against the CPU on 1,000 stacks): SynthMorph in FreeSurfer's own
Python with the CUDA 11.8 libraries its TensorFlow needs; SynthStrip in a
PyTorch built for current cards; SynthSeg's script in a current
TensorFlow, several processes at once, each held to a share of the card.

Where each runs is the image's (``NILS_PC_*`` environment variables, set
in the Dockerfile), so the same code runs outside the image in a research
environment that has the same tools elsewhere.
"""

from __future__ import annotations

import glob
import json
import os
import shlex
import shutil
import subprocess
import time
from dataclasses import dataclass, field
from pathlib import Path

HERE = Path(__file__).resolve().parent


def _split(v: str | None) -> list:
    return shlex.split(v) if v else []


@dataclass
class Env:
    fs_home: str
    fs_prefix: list = field(default_factory=list)
    fspython: list = field(default_factory=lambda: ["fspython"])
    cu118: str = ""
    tf_python: str = ""
    synthseg_script: str = ""
    st_python: str = ""
    dcm2niix: list = field(default_factory=lambda: ["dcm2niix"])

    @classmethod
    def from_environ(cls) -> "Env":
        e = os.environ
        return cls(
            fs_home=e.get("NILS_PC_FS_HOME") or e.get("FREESURFER_HOME") or "/usr/local/freesurfer/8.2.0-1",
            fs_prefix=_split(e.get("NILS_PC_FS_PREFIX")),
            fspython=_split(e.get("NILS_PC_FSPYTHON")) or ["fspython"],
            cu118=e.get("NILS_PC_CU118", ""),
            tf_python=e.get("NILS_PC_TF_PYTHON", ""),
            synthseg_script=e.get("NILS_PC_SYNTHSEG_SCRIPT", ""),
            st_python=e.get("NILS_PC_ST_PYTHON", ""),
            dcm2niix=_split(e.get("NILS_PC_DCM2NIIX")) or ["dcm2niix"],
        )

    @property
    def mni(self) -> str:
        return f"{self.fs_home}/average/mni_icbm152_nlin_asym_09c/mni_icbm152_t1_tal_nlin_asym_09c.nii.gz"

    def card_ready(self) -> list:
        """What the card's tools lack here; empty when every one is in place."""
        need = []
        for name, p in (("SynthSeg's TensorFlow", self.tf_python), ("SynthSeg's script", self.synthseg_script),
                        ("SynthStrip's PyTorch", self.st_python), ("FreeSurfer's CUDA 11.8 libraries", self.cu118)):
            if not p or not os.path.exists(p):
                need.append(name)
        return need


def run(cmd: list, log: str, env: dict | None = None, threads: str = "4") -> int:
    """A tool's command, its output to the stack's log; its return code."""
    e = dict(os.environ if env is None else env)
    e.setdefault("OMP_NUM_THREADS", threads)
    e.setdefault("ITK_GLOBAL_DEFAULT_NUMBER_OF_THREADS", threads)
    e["CUDA_VISIBLE_DEVICES"] = e.get("NILS_PC_CPU_CARDS", "")  # the CPU's tools never see a card
    with open(log, "a") as lf:
        try:
            return subprocess.run([str(c) for c in cmd], stdout=lf, stderr=lf, env=e).returncode
        except OSError as err:
            lf.write(f"could not start: {type(err).__name__}\n")
            return 127


def convert(env: Env, d: str, files: list) -> dict:
    """dcm2niix of the stack's own files into ``<d>/nat.nii.gz``; where it
    writes several volumes (a suffix), the largest is the stack's. A failed
    conversion is tried once more in a fresh folder (round 2: a production
    path needs the retry)."""
    out = d + "/nat.nii.gz"
    if os.path.exists(out):
        return {"ok": True, "convert": "cached"}
    info: dict = {"files": len(files)}
    for attempt in (1, 2):
        os.makedirs(d, exist_ok=True)
        for f in glob.glob(d + "/nat*"):
            os.remove(f)
        lk = d + "/dcm"
        shutil.rmtree(lk, ignore_errors=True)
        os.makedirs(lk)
        for i, f in enumerate(files):
            os.symlink(f, f"{lk}/{i:05d}.dcm")
        t0 = time.time()
        rc = run([*env.dcm2niix, "-z", "y", "-b", "y", "-ba", "y", "-f", "nat", "-o", d, lk], d + "/dcm2niix.log", threads="1")
        shutil.rmtree(lk, ignore_errors=True)
        outs = sorted(glob.glob(d + "/nat*.nii.gz"))
        info.update({f"rc{attempt}": rc, f"s{attempt}": round(time.time() - t0, 2), "n_nifti": len(outs)})
        if not os.path.exists(out) and outs:
            best = max(outs, key=os.path.getsize)
            shutil.copyfile(best, out)
            info["pick"] = os.path.basename(best)
        if os.path.exists(out):
            info["ok"] = True
            info["attempts"] = attempt
            return info
    info["ok"] = False
    return info


def check_native(d: str) -> str | None:
    """Why a converted stack cannot go on, or None."""
    import nibabel as nib

    sh = nib.load(d + "/nat.nii.gz").shape
    if len(sh) > 3 and sh[3] > 1:
        return "4D"
    if min(sh[:3]) < 3:
        return "fewer than 3 slices"
    return None


def paths(d: str) -> dict:
    u = d + "/u"
    return {"nat": d + "/nat.nii.gz", "lta": u + "/aff_f2m.lta", "hp": u + "/hp_aff.nii.gz", "vx": u + "/vx_aff.nii.gz",
            "ss": u + "/synthseg.nii.gz", "strip": d + "/strip.nii.gz", "mask": d + "/mask.nii.gz"}


def missing(d: str) -> list:
    """The tools' outputs a stack still lacks: synthmorph, synthseg, synthstrip."""
    p = paths(d)
    out = []
    if not all(os.path.exists(p[k]) for k in ("lta", "hp", "vx")):
        out.append("synthmorph")
    if not os.path.exists(p["ss"]):
        out.append("synthseg")
    if not os.path.exists(p["strip"]):
        out.append("synthstrip")
    return out


def cpu_tools(env: Env, d: str, atlas: dict, which: list, threads: str = "4") -> dict:
    """FreeSurfer's own commands on the CPU for the outputs a stack lacks,
    as model r1's study ran them."""
    p = paths(d)
    os.makedirs(d + "/u", exist_ok=True)
    fs = env.fs_prefix
    rc: dict = {}
    t0 = time.time()
    if "synthmorph" in which:
        if not os.path.exists(p["lta"]):
            rc["SM-affine"] = run([*fs, "mri_synthmorph", "register", "-m", "affine", "-j", threads, "-T", p["lta"], p["nat"], env.mni],
                                  d + "/u/SM-affine.log", threads=threads)
        for k, src in (("hp", atlas["HP"]), ("vx", atlas["VX"])):
            if os.path.exists(p["lta"]) and not os.path.exists(p[k]):
                rc[k] = run([*fs, "mri_synthmorph", "apply", "-m", "nearest", p["lta"], src, p[k]], d + f"/u/{k}.log", threads=threads)
    if "synthseg" in which and not os.path.exists(p["ss"]):
        rc["synthseg"] = run([*fs, "mri_synthseg", "--i", p["nat"], "--o", p["ss"], "--cpu", "--threads", "8"], d + "/u/synthseg.log", threads="8")
    if "synthstrip" in which and not os.path.exists(p["strip"]):
        rc["synthstrip"] = run([*fs, "mri_synthstrip", "-i", p["nat"], "-o", p["strip"], "-m", p["mask"], "--threads", threads],
                               d + "/synthstrip.log", threads=threads)
    return {"rc": rc, "s": round(time.time() - t0, 2)}


def _nvidia_libs(site: str) -> str:
    if not site or not os.path.isdir(site):
        return ""
    return ":".join(sorted(os.path.join(site, n, "lib") for n in os.listdir(site) if os.path.isdir(os.path.join(site, n, "lib"))))


def _site_nvidia(python: str) -> str:
    """The pip CUDA libraries beside a Python environment's interpreter."""
    root = Path(python).absolute().parent.parent  # the environment's root; its interpreter may link elsewhere
    hits = sorted(root.glob("lib/python3*/site-packages/nvidia"))
    return str(hits[0]) if hits else ""


@dataclass
class Card:
    """One card and what this run may hold of it."""

    device: str  # what CUDA_VISIBLE_DEVICES names it by (a UUID or an index)
    budget_mib: int  # what the run may hold of it at once
    reserved: int = 0


def _take(card: Card, need: int, lock, keep_free: int) -> None:
    """Wait until the run's share of the card holds ``need`` more MiB and,
    on a shared card, the card keeps ``keep_free`` MiB free after it."""
    while True:
        with lock:
            fits = card.reserved + need <= card.budget_mib or card.reserved == 0
            if fits and keep_free > 0:
                free = card_free_mib(card.device)
                fits = free is not None and free - need >= keep_free
            if fits:
                card.reserved += need
                return
        time.sleep(5)


def wave(env: Env, card: Card, sids: list, work: str, atlas: dict, logdir: str, ss_procs: int, ss_mem_mib: int, other_mib: int,
         lock, stats: dict, keep_free: int = 0) -> dict:
    """One wave of converted stacks on a card: SynthSeg in ``ss_procs``
    processes (each capped at ``ss_mem_mib``) while SynthMorph and
    SynthStrip run beside them, each one process over the wave. Returns the
    seconds each took; what a card step leaves undone is the CPU's."""
    sids = [s for s in sids if missing(f"{work}/stacks/{s}")]
    if not sids:
        return {}
    tag = f"w{sids[0]}"
    jm, js, ss = [], [], []
    for sid in sids:
        d = f"{work}/stacks/{sid}"
        p = paths(d)
        os.makedirs(d + "/u", exist_ok=True)
        m = missing(d)
        if "synthmorph" in m:
            jm.append({"id": sid, "tool": "synthmorph", "argv": ["register", "-m", "affine", "-j", "4", "-T", p["lta"], "-g", p["nat"], env.mni]})
            jm.append({"id": sid, "tool": "synthmorph", "argv": ["apply", "-m", "nearest", p["lta"], atlas["HP"], p["hp"], atlas["VX"], p["vx"]]})
        if "synthstrip" in m:
            js.append({"id": sid, "tool": "synthstrip", "argv": ["-i", p["nat"], "-o", p["strip"], "-m", p["mask"], "-g"]})
        if "synthseg" in m:
            ss.append((p["nat"], p["ss"]))
    # the run's share of the card: a wave starts when its processes fit
    n_ss = min(ss_procs, len(ss)) if ss else 0
    need = other_mib + n_ss * ss_mem_mib
    _take(card, need, lock, keep_free)
    res: dict = {}
    t0 = time.time()
    procs = []
    try:
        if n_ss:
            tf_site = _site_nvidia(env.tf_python)
            tenv = dict(os.environ, LD_LIBRARY_PATH=_nvidia_libs(tf_site), TF_USE_LEGACY_KERAS="1", TF_CPP_MIN_LOG_LEVEL="2",
                        TF_CUDNN_USE_AUTOTUNE="0", FREESURFER_HOME=env.fs_home, CUDA_VISIBLE_DEVICES=card.device, OMP_NUM_THREADS="4",
                        NILS_PC_TF_MEM_MB=str(ss_mem_mib), PYTHONPATH=str(HERE / "tfcap"))
            tenv.pop("TF_FORCE_GPU_ALLOW_GROWTH", None)
            for i in range(n_ss):
                part = ss[i::n_ss]
                fi, fo = f"{logdir}/{tag}.ss{i}.in.txt", f"{logdir}/{tag}.ss{i}.out.txt"
                Path(fi).write_text("".join(a + "\n" for a, _ in part))
                Path(fo).write_text("".join(b + "\n" for _, b in part))
                lf = open(f"{logdir}/{tag}.ss{i}.log", "a")
                procs.append((subprocess.Popen([env.tf_python, env.synthseg_script, "--i", fi, "--o", fo, "--threads", "4"], stdout=lf,
                                               stderr=lf, env=tenv), lf))
        t1 = time.time()
        pm = ps = None
        if jm:
            jf = f"{logdir}/{tag}.synthmorph.jobs.jsonl"
            Path(jf).write_text("".join(json.dumps(j) + "\n" for j in jm))
            # FreeSurfer's TensorFlow takes the whole card unless it grows into it (the study ran it so)
            menv = dict(os.environ, CUDA_VISIBLE_DEVICES=card.device, OMP_NUM_THREADS="4", FREESURFER_HOME=env.fs_home,
                        LD_LIBRARY_PATH=_nvidia_libs(env.cu118), TF_FORCE_GPU_ALLOW_GROWTH="true")
            lf1 = open(f"{logdir}/{tag}.synthmorph.log", "a")
            pm = (subprocess.Popen([*env.fspython, str(HERE / "synth_batch.py"), jf, f"{logdir}/{tag}.synthmorph.out.jsonl"], stdout=lf1,
                                   stderr=lf1, env=menv), lf1)
        if js:
            jf2 = f"{logdir}/{tag}.synthstrip.jobs.jsonl"
            Path(jf2).write_text("".join(json.dumps(j) + "\n" for j in js))
            senv = dict(os.environ, CUDA_VISIBLE_DEVICES=card.device, OMP_NUM_THREADS="4", FREESURFER_HOME=env.fs_home,
                        NILS_PC_TORCH_FRAC=os.environ.get("NILS_PC_STRIP_FRAC", "0.15"))
            lf2 = open(f"{logdir}/{tag}.synthstrip.log", "a")
            ps = (subprocess.Popen([env.st_python, str(HERE / "synth_batch.py"), jf2, f"{logdir}/{tag}.synthstrip.out.jsonl"], stdout=lf2,
                                   stderr=lf2, env=senv), lf2)
        for name, pr in (("synthmorph", pm), ("synthstrip", ps)):
            if pr is not None:
                pr[0].wait()
                pr[1].close()
                res[name] = round(time.time() - t1, 2)
        for p, lf in procs:
            p.wait()
            lf.close()
        res["synthseg"] = round(time.time() - t0, 2)
    finally:
        for p, lf in procs:
            if p.poll() is None:
                p.kill()
                p.wait()
            lf.close()
        with lock:
            card.reserved -= need
    for name in ("synthmorph", "synthstrip"):
        f = f"{logdir}/{tag}.{name}.out.jsonl"
        for line in open(f) if os.path.exists(f) else []:
            r = json.loads(line)
            k = f"{name}_{'ok' if r.get('ok') else 'failed'}"
            stats[k] = stats.get(k, 0) + 1
    stats["synthseg_written"] = stats.get("synthseg_written", 0) + sum(os.path.exists(o) for _, o in ss)
    stats["synthseg_asked"] = stats.get("synthseg_asked", 0) + len(ss)
    return res


def synthseg_retry(env: Env, card: Card, sids: list, work: str, logdir: str, mem_mib: int, lock, keep_free: int = 0) -> int:
    """The stacks a wave's SynthSeg did not segment (a card's scratch space
    that was not there, a process's share too small), once more on the
    card in one process with a larger share; what is still missing goes to
    the CPU. Returns how many it segmented."""
    miss = [s for s in sids if os.path.exists(f"{work}/stacks/{s}/nat.nii.gz") and not os.path.exists(paths(f"{work}/stacks/{s}")["ss"])]
    if not miss:
        return 0
    tag = f"r{miss[0]}"
    fi, fo = f"{logdir}/{tag}.ss.in.txt", f"{logdir}/{tag}.ss.out.txt"
    Path(fi).write_text("".join(f"{work}/stacks/{s}/nat.nii.gz\n" for s in miss))
    Path(fo).write_text("".join(paths(f"{work}/stacks/{s}")["ss"] + "\n" for s in miss))
    _take(card, mem_mib, lock, keep_free)
    try:
        tenv = dict(os.environ, LD_LIBRARY_PATH=_nvidia_libs(_site_nvidia(env.tf_python)), TF_USE_LEGACY_KERAS="1", TF_CPP_MIN_LOG_LEVEL="2",
                    TF_CUDNN_USE_AUTOTUNE="0", FREESURFER_HOME=env.fs_home, CUDA_VISIBLE_DEVICES=card.device, OMP_NUM_THREADS="4",
                    NILS_PC_TF_MEM_MB=str(mem_mib), PYTHONPATH=str(HERE / "tfcap"))
        with open(f"{logdir}/{tag}.ss.log", "a") as lf:
            subprocess.run([env.tf_python, env.synthseg_script, "--i", fi, "--o", fo, "--threads", "4"], stdout=lf, stderr=lf, env=tenv)
    finally:
        with lock:
            card.reserved -= mem_mib
    return sum(os.path.exists(paths(f"{work}/stacks/{s}")["ss"]) for s in miss)


def cards_visible() -> list:
    """The cards this process may use, by what CUDA_VISIBLE_DEVICES names
    them (the runner's choice), with each one's total memory in MiB."""
    try:
        out = subprocess.run(["nvidia-smi", "--query-gpu=index,uuid,memory.total", "--format=csv,noheader,nounits"], capture_output=True,
                             text=True, timeout=30).stdout
    except (OSError, subprocess.SubprocessError):
        return []
    rows = []
    for line in out.splitlines():
        parts = [p.strip() for p in line.split(",")]
        if len(parts) == 3 and parts[2].isdigit():
            rows.append((parts[0], parts[1], int(parts[2])))
    vis = os.environ.get("CUDA_VISIBLE_DEVICES")
    if vis is None:
        return [(u, m) for _i, u, m in rows]
    if vis.strip() in ("", "-1", "none"):
        return []
    keep = []
    for v in (x.strip() for x in vis.split(",") if x.strip()):
        for i, u, m in rows:
            if v in (i, u):
                keep.append((u, m))
    return keep


def card_free_mib(device: str) -> int | None:
    try:
        out = subprocess.run(["nvidia-smi", "--query-gpu=uuid,index,memory.free", "--format=csv,noheader,nounits"], capture_output=True,
                             text=True, timeout=30).stdout
    except (OSError, subprocess.SubprocessError):
        return None
    for line in out.splitlines():
        parts = [p.strip() for p in line.split(",")]
        if len(parts) == 3 and device in (parts[0], parts[1]) and parts[2].isdigit():
            return int(parts[2])
    return None

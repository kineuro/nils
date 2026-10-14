# SPDX-License-Identifier: AGPL-3.0-only
"""FreeSurfer's SynthMorph and SynthStrip scripts run many times in one
long-lived process, instead of one start and one framework import a stack
(the post-contrast speed study's route R4, class I). Each job runs the
tool's own script unchanged (runpy) with that job's arguments, so the
computation is the tool's; SynthMorph's network and weights are built once
and reused (a Keras model's inference holds no state between calls).

    python synth_batch.py <jobs.jsonl> <log.jsonl>

A job: {"id": <stack id>, "tool": "synthmorph" | "synthstrip", "argv": [the
tool's arguments]}. Writes one line a job: id, tool, ok, seconds, and the
error's type when it failed. Runs under FreeSurfer's own Python (3.8) as
well as a current one, so it uses nothing newer.
"""

import contextlib
import io
import json
import os
import runpy
import sys
import time

FS = os.environ.get("FREESURFER_HOME", "")
SCRIPTS = {"synthmorph": os.path.join(FS, "python", "scripts", "mri_synthmorph"),
           "synthstrip": os.path.join(FS, "python", "scripts", "mri_synthstrip")}


def cache_synthmorph():
    os.environ.setdefault("NEURITE_BACKEND", "tensorflow")
    os.environ.setdefault("VXM_BACKEND", "tensorflow")
    os.environ.setdefault("TF_CPP_MIN_LOG_LEVEL", "3")
    from synthmorph import registration as R

    nets = {}
    orig = R.vxm.networks.VxmAffineFeatureDetector

    def net(**prop):
        k = json.dumps(prop, sort_keys=True, default=str)
        if k not in nets:
            nets[k] = orig(**prop)
        return nets[k]

    R.vxm.networks.VxmAffineFeatureDetector = net
    load = R.load_weights
    done = set()

    def load_once(model, weights):
        k = (id(model), str(weights))
        if k in done:
            return
        load(model, weights)
        done.add(k)

    R.load_weights = load_once


def main():
    frac = os.environ.get("NILS_PC_TORCH_FRAC")
    if frac:  # this process's share of the card (the PyTorch tool)
        import torch

        if torch.cuda.is_available():
            torch.cuda.set_per_process_memory_fraction(float(frac), 0)
    jobs = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]
    log = open(sys.argv[2], "a")
    if any(j["tool"] == "synthmorph" for j in jobs):
        cache_synthmorph()
    for j in jobs:
        t0 = time.time()
        rec = {"id": j["id"], "tool": j["tool"]}
        sys.argv = [SCRIPTS[j["tool"]]] + [str(a) for a in j["argv"]]
        buf = io.StringIO()
        try:
            with contextlib.redirect_stdout(buf):
                runpy.run_path(SCRIPTS[j["tool"]], run_name="__main__")
            rec["ok"] = True
        except SystemExit as e:
            rec["ok"] = e.code in (None, 0)
            if not rec["ok"]:
                rec["error"] = "SystemExit"
        except BaseException as e:  # noqa: BLE001 - one job fails, the batch goes on
            rec["ok"] = False
            rec["error"] = type(e).__name__
        rec["s"] = round(time.time() - t0, 3)
        log.write(json.dumps(rec) + "\n")
        log.flush()


if __name__ == "__main__":
    main()

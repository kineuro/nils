#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""v0's stack partition of a tree, instance by instance. Writes one JSON line per
readable file with the instance's UIDs and its v0 stack signature; the output
stays on the host beside the corpus. Prints counts only.

    v0_partition.py ROOT OUT.jsonl [--workers N] [--v0 DIR]

DIR holds `stack_utils.py` and `dicom_mappings.py` from v0 0.5.0
(`backend/src/extract/`, MIT), unchanged; 0.5.3 computes the same tuple. Every
file under ROOT is read the way v0's worker reads it (pydicom, force=True,
stop_before_pixels=True), the instance fields are extracted with v0's map and
the signature is v0's `compute_stack_signature` without the series UID, which
is the line's own field. The file's modality and whether v0 would have kept it
(study, series, SOP and SOP class present; modality MR, CT, PT or PET) go on
the line too, so the comparison can say what each side holds.
"""
import argparse
import json
import os
import sys
from collections import Counter
from multiprocessing import Pool
from pathlib import Path

ALLOWED = {"MR", "CT", "PT", "PET"}

_V0 = None


def _load_v0(v0_dir: str):
    global _V0
    if _V0 is None:
        sys.path.insert(0, v0_dir)
        import dicom_mappings  # noqa: E402
        import stack_utils  # noqa: E402

        _V0 = (dicom_mappings, stack_utils)
    return _V0


def _walk(root: Path):
    for dirpath, _, names in os.walk(root):
        for n in names:
            yield os.path.join(dirpath, n)


def _one(args):
    path, v0_dir = args
    import pydicom

    mappings, stacks = _load_v0(v0_dir)
    try:
        ds = pydicom.dcmread(path, force=True, stop_before_pixels=True)
    except Exception:
        return ("unreadable", None)
    study = getattr(ds, "StudyInstanceUID", None)
    series = getattr(ds, "SeriesInstanceUID", None)
    sop = getattr(ds, "SOPInstanceUID", None)
    sop_class = getattr(ds, "SOPClassUID", None)
    if not (series and sop):
        return ("no_uid", None)
    modality = getattr(ds, "Modality", None)
    modality = str(modality).strip().upper() if modality is not None else None
    if not modality:
        series_fields = mappings.extract_fields(ds, mappings.SERIES_FIELD_MAP)
        m = series_fields.get("modality")
        modality = str(m).strip().upper() if m else None
    kept = bool(study and sop_class) and modality in ALLOWED
    fields = mappings.extract_fields(ds, mappings.INSTANCE_FIELD_MAP)
    sig = stacks.compute_stack_signature(str(series), fields)[1:]
    line = {
        "sop": str(sop),
        "series": str(series),
        "modality": modality,
        "kept": kept,
        "sig": list(sig),
    }
    return ("ok", line)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("root")
    ap.add_argument("out")
    ap.add_argument("--workers", type=int, default=8)
    ap.add_argument("--v0", default=str(Path(__file__).resolve().parent / "v0"))
    a = ap.parse_args()

    counts = Counter()
    modalities = Counter()
    paths = list(_walk(Path(a.root)))
    counts["files"] = len(paths)
    with Pool(a.workers) as pool, open(a.out, "w") as out:
        for status, line in pool.imap_unordered(
            _one, ((p, a.v0) for p in paths), chunksize=64
        ):
            counts[status] += 1
            if line is not None:
                m = line["modality"]
                modalities[m if m in ALLOWED else ("(other)" if m else "(none)")] += 1
                counts["kept_by_v0"] += line["kept"]
                out.write(json.dumps(line) + "\n")
    print(json.dumps({"counts": counts, "modalities": modalities}, indent=1, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())

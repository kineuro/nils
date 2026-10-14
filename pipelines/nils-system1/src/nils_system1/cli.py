# SPDX-License-Identifier: AGPL-3.0-only
"""``nils-system1``: one entry point, ``score``.

The runner mounts the input read-only at ``/input`` (``stacks.json``), the
model input read-only under ``/inputs/model`` with ``/inputs/manifest.json``
naming it, and ``/output``, the only place it writes. Nothing is fetched.
An error never names a file path, since a source path may carry a code.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from .bundle import BundleError
from .score import EXPLAIN, InputError, run


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="nils-system1", description=__doc__)
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("score", help="System 1's opinion on every stack of the run")
    s.add_argument("--stacks", type=Path, default=Path("/input/stacks.json"))
    s.add_argument("--inputs", type=Path, default=Path("/inputs"))
    s.add_argument("--output", type=Path, default=Path("/output"))
    s.add_argument("--trust-code", choices=["true", "false"], default="false",
                   help="run a module predictor the bundle brings, after its bytes match (default false)")
    s.add_argument("--explain", default="default",
                   help="the fields left out one at a time, comma separated; 'none' measures none")
    s.add_argument("--bundle", type=Path, default=None, help="a bundle outside the inputs, for a trial")
    a = ap.parse_args(argv)
    fields = EXPLAIN if a.explain == "default" else ([] if a.explain == "none" else a.explain.split(","))
    try:
        out = run(a.stacks, a.inputs, a.output, a.trust_code == "true", fields, a.bundle)
    except (InputError, BundleError, json.JSONDecodeError, KeyError) as e:
        print(f"nils-system1: {e}", file=sys.stderr)
        return 2
    print(json.dumps(out["metrics"]))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

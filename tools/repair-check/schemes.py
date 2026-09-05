# SPDX-License-Identifier: AGPL-3.0-only
"""Write out one scenario's session schemes and name them.

    schemes.py MANIFEST.json SCENARIO WORKDIR

Prints one index per scheme the scenario declares, having written each to
`WORKDIR/scheme-SCENARIO-INDEX.yml`. The gate then asks nils for the sessions
under each. A scenario with no session checks prints nothing, and the gate's
loop runs zero times.
"""

from __future__ import annotations

import json
import sys


def main() -> int:
    manifest, scenario, work = sys.argv[1:4]
    want = json.load(open(manifest, encoding="utf-8"))
    for s in want["scenarios"]:
        if s["name"] != scenario:
            continue
        for i, check in enumerate(s.get("sessions", [])):
            with open(f"{work}/scheme-{scenario}-{i}.yml", "w", encoding="utf-8") as fh:
                fh.write(check["scheme"])
            print(i)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

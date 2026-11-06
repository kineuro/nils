# SPDX-License-Identifier: AGPL-3.0-only
"""How far a run is, as the engine reads it while the container works
(``contracts/job/v1/progress.schema.json``, 2026-11-05).

The engine names a file in ``NILS_PROGRESS``; the image replaces it whole,
a file beside it renamed over, with how many of the stacks it was given it
has answered: ``{"done": 312, "of": 870, "what": "stacks"}``. Without the
variable (an engine from before, or a run by hand) nothing is written. A
write is at most every few seconds, and the last is always written; a
write that fails is let go, since progress is never worth a run.
"""

from __future__ import annotations

import json
import os
import time

ENV = "NILS_PROGRESS"
EVERY_S = 2.0


class Progress:
    """Stacks done of stacks given, written where the engine reads it."""

    def __init__(self, of: int | None, what: str = "stacks", path: str | None = None, every_s: float = EVERY_S) -> None:
        self.path = path if path is not None else os.environ.get(ENV) or None
        self.of = of
        self.what = what
        self.every_s = every_s
        self.done = 0
        self._written: float | None = None

    def set(self, done: int, force: bool = False) -> None:
        """Say ``done`` stacks are answered; written now where the last write is old enough, or ``force``."""
        self.done = max(0, int(done))
        if self.path is None:
            return
        now = time.monotonic()
        if not force and self._written is not None and now - self._written < self.every_s:
            return
        self._written = now
        doc = {"done": self.done if self.of is None else min(self.done, self.of), "what": self.what}
        if self.of is not None:
            doc["of"] = self.of
        part = f"{self.path}.part"
        try:
            with open(part, "w") as f:
                f.write(json.dumps(doc))
            os.replace(part, self.path)
        except OSError:
            # never leave the half-written file behind as if it were an output
            try:
                os.remove(part)
            except OSError:
                pass

    def add(self, n: int = 1) -> None:
        """``n`` more stacks answered."""
        self.set(self.done + n)

    def finish(self) -> None:
        """Every stack answered: the last count, written whatever the time."""
        self.set(self.of if self.of is not None else self.done, force=True)

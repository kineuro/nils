#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Tests of release-descriptors.py (record 55 A5): a release whose image jobs
partly fail still writes the descriptors of the images it pushed, and names
the ones it did not. Standard library only: python3 scripts/test_release_descriptors.py"""

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "release-descriptors.py"
EXPECT = "nils-bodypart,nils-postcontrast,nils-system1"


def write(refs: dict[str, str], out: Path, refs_dir: Path) -> subprocess.CompletedProcess:
    refs_dir.mkdir(parents=True, exist_ok=True)
    for image, digest in refs.items():
        (refs_dir / f"{image}.ref").write_text(f"ghcr.io/kineuro/{image}@sha256:{digest * 64}\n")
    return subprocess.run([sys.executable, str(SCRIPT), "write", "--refs", str(refs_dir), "--out", str(out), "--tag", "v1.0.0-test",
                           "--expect", EXPECT], capture_output=True, text=True)


class Write(unittest.TestCase):
    def test_a_failed_image_ships_no_descriptor_and_is_named(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            r = write({"nils-system1": "a"}, d / "out", d / "refs")
            self.assertEqual(r.returncode, 0, r.stderr)
            images = (d / "out" / "IMAGES").read_text().splitlines()
            self.assertTrue(images and all(line.split("\t")[1].startswith("ghcr.io/kineuro/nils-system1@") for line in images), images)
            shipped = {p.parent.name for p in (d / "out" / "pipelines").rglob("nils.job.yml")}
            self.assertIn("system1-score", shipped)
            self.assertFalse(any(n.startswith(("bodypart", "postcontrast")) for n in shipped), shipped)
            notes = (d / "out" / "NOT-SHIPPED").read_text()
            self.assertIn("`nils-bodypart`", notes)
            self.assertIn("`nils-postcontrast`", notes)
            self.assertIn("postcontrast-cascade", notes)
            self.assertNotIn("nils-system1", notes)

    def test_no_image_pushed_still_writes_an_empty_set(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            r = write({}, d / "out", d / "refs")
            self.assertEqual(r.returncode, 0, r.stderr)
            self.assertEqual((d / "out" / "IMAGES").read_text(), "")
            self.assertEqual(list((d / "out" / "pipelines").rglob("nils.job.yml")), [])
            self.assertEqual(len((d / "out" / "NOT-SHIPPED").read_text().splitlines()), 3)

    def test_every_image_pushed_names_none(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            r = write({"nils-bodypart": "b", "nils-postcontrast": "c", "nils-system1": "a"}, d / "out", d / "refs")
            self.assertEqual(r.returncode, 0, r.stderr)
            self.assertEqual((d / "out" / "NOT-SHIPPED").read_text(), "")


if __name__ == "__main__":
    unittest.main()

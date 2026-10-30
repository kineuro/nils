#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Tests of release-descriptors.py (record 55 A5): a release whose image jobs
partly fail still writes the descriptors of the images it pushed, and names
the ones it did not. Standard library only: python3 scripts/test_release_descriptors.py"""

import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "release-descriptors.py"
EXPECT = "nils-bodypart,nils-postcontrast,nils-system1"


FULL = "pipelines/nils-postcontrast/postcontrast-full/nils.job.yml"


def full_pin() -> str:
    text = (SCRIPT.parent.parent / FULL).read_text()
    return next(line.split('"')[1] for line in text.splitlines() if line.strip().startswith("image:"))


def write(refs: dict[str, str], out: Path, refs_dir: Path, pullable: bool = True,
          taken: dict[str, str] | None = None, from_files: bool = True) -> subprocess.CompletedProcess:
    refs_dir.mkdir(parents=True, exist_ok=True)
    for image, digest in refs.items():
        (refs_dir / f"{image}.ref").write_text(f"ghcr.io/kineuro/{image}@sha256:{digest * 64}\n")
    # the image job writes <image>.from for every image it ships: "built" or the release it took
    for image in (refs if from_files else {}):
        (refs_dir / f"{image}.from").write_text(f"{(taken or {}).get(image, 'built')}\n")
    env = dict(os.environ, NILS_RELEASE_PULLABLE=full_pin() if pullable else "")
    return subprocess.run([sys.executable, str(SCRIPT), "write", "--refs", str(refs_dir), "--out", str(out), "--tag", "v1.0.0-test",
                           "--expect", EXPECT], capture_output=True, text=True, env=env)


class Write(unittest.TestCase):
    def test_a_failed_image_ships_no_descriptor_and_is_named(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            r = write({"nils-system1": "a"}, d / "out", d / "refs")
            self.assertEqual(r.returncode, 0, r.stderr)
            images = (d / "out" / "IMAGES").read_text().splitlines()
            self.assertTrue(images and all(line.split("\t")[1].startswith(("ghcr.io/kineuro/nils-system1@", "ghcr.io/kineuro/nils-postcontrast-full@"))
                                           for line in images), images)
            shipped = {p.parent.name for p in (d / "out" / "pipelines").rglob("nils.job.yml")}
            self.assertIn("system1-score", shipped)
            self.assertFalse(any(n.startswith(("bodypart", "postcontrast")) for n in shipped - {"postcontrast-full"}), shipped)
            # the fallback's image, published apart, ships with the tree's pin
            self.assertIn("postcontrast-full", shipped)
            full = next(line for line in images if line.startswith("postcontrast-full\t"))
            tree = (SCRIPT.parent.parent / "pipelines/nils-postcontrast/postcontrast-full/nils.job.yml").read_text()
            self.assertIn(full.split("\t")[1], tree)
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
            # only the image published apart
            self.assertEqual([line.split("\t")[0] for line in (d / "out" / "IMAGES").read_text().splitlines()], ["postcontrast-full"])
            self.assertEqual(len((d / "out" / "NOT-SHIPPED").read_text().splitlines()), 3)

    def test_a_pin_no_one_may_pull_is_not_shipped(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            r = write({"nils-bodypart": "b", "nils-postcontrast": "c", "nils-system1": "a"}, d / "out", d / "refs", pullable=False)
            self.assertEqual(r.returncode, 0, r.stderr)
            shipped = {p.parent.name for p in (d / "out" / "pipelines").rglob("nils.job.yml")}
            self.assertNotIn("postcontrast-full", shipped)
            self.assertIn("postcontrast-cascade", shipped)
            notes = (d / "out" / "NOT-SHIPPED").read_text()
            self.assertIn("nils-postcontrast-full", notes)
            self.assertIn("cannot be pulled without credentials", notes)

    def test_every_image_pushed_names_none(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            r = write({"nils-bodypart": "b", "nils-postcontrast": "c", "nils-system1": "a"}, d / "out", d / "refs")
            self.assertEqual(r.returncode, 0, r.stderr)
            self.assertEqual((d / "out" / "NOT-SHIPPED").read_text(), "")

    def test_the_notes_say_which_image_was_built_and_which_taken(self):
        """A release that builds only what changed says so for each image: built by
        this release, taken unchanged from the release named, or published apart."""
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            r = write({"nils-bodypart": "b", "nils-postcontrast": "c", "nils-system1": "a"}, d / "out", d / "refs",
                      taken={"nils-bodypart": "v1.0.0-alpha.86", "nils-postcontrast": "built"})
            self.assertEqual(r.returncode, 0, r.stderr)
            notes = (d / "out" / "MODEL-IMAGES.md").read_text().splitlines()
            self.assertEqual(notes[0], "### Model images")
            lines = {line.split("`")[1]: line for line in notes[2:]}
            self.assertEqual(set(lines), {"nils-bodypart", "nils-postcontrast", "nils-postcontrast-full", "nils-system1"})
            self.assertIn(f"`ghcr.io/kineuro/nils-bodypart@sha256:{'b' * 64}`, unchanged since v1.0.0-alpha.86, whose image it is "
                          "(bodypart-embed, bodypart-infer, bodypart-infer-fusion, bodypart-seed, bodypart-train)", lines["nils-bodypart"])
            self.assertIn(f"`ghcr.io/kineuro/nils-postcontrast@sha256:{'c' * 64}`, built by this release (postcontrast-cascade)",
                          lines["nils-postcontrast"])
            # built, with no release named
            self.assertIn(f"`ghcr.io/kineuro/nils-system1@sha256:{'a' * 64}`, built by this release (system1-score)", lines["nils-system1"])
            self.assertIn(f"`{full_pin()}`, published apart from the release, as the tree pins it (postcontrast-full)",
                          lines["nils-postcontrast-full"])
            # every digest the notes name is one IMAGES names
            images = {line.split("\t")[1] for line in (d / "out" / "IMAGES").read_text().splitlines()}
            self.assertEqual({line.split("`")[3] for line in notes[2:]}, images)

    def test_a_built_image_without_its_from_file_fails_clearly(self):
        """The image job always writes <image>.from; a missing one is a broken
        workflow, not a build to guess."""
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            r = write({"nils-system1": "a"}, d / "out", d / "refs", from_files=False)
            self.assertNotEqual(r.returncode, 0)
            self.assertIn("nils-system1.from is missing", r.stderr)

    def test_no_image_at_all_writes_no_notes(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            r = write({}, d / "out", d / "refs", pullable=False)
            self.assertEqual(r.returncode, 0, r.stderr)
            self.assertEqual((d / "out" / "MODEL-IMAGES.md").read_text(), "")


class Pullable(unittest.TestCase):
    """Only a clear 401, 403 or 404 says a pin is not public; a network
    error is tried again and then fails the step, never read as private."""

    def test_the_command_line_answers_by_its_exit_code(self):
        ref = "ghcr.io/kineuro/x@sha256:" + "a" * 64
        env = dict(os.environ, NILS_RELEASE_PULLABLE=ref)
        self.assertEqual(subprocess.run([sys.executable, str(SCRIPT), "pullable", ref], env=env).returncode, 0)
        other = "ghcr.io/kineuro/x@sha256:" + "b" * 64
        self.assertEqual(subprocess.run([sys.executable, str(SCRIPT), "pullable", other], env=env).returncode, 1)

    def setUp(self):
        import importlib.util
        import unittest.mock
        spec = importlib.util.spec_from_file_location("release_descriptors", SCRIPT)
        self.m = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.m)
        self.mock = unittest.mock
        self.env = self.mock.patch.dict(os.environ, {}, clear=False)
        self.env.start()
        os.environ.pop("NILS_RELEASE_PULLABLE", None)
        self.ref = "ghcr.io/kineuro/x@sha256:" + "a" * 64

    def tearDown(self):
        self.env.stop()

    def answer(self, error):
        return self.mock.patch.object(self.m.urllib.request, "urlopen", side_effect=error)

    def test_a_clear_refusal_is_not_public(self):
        import urllib.error
        for code in (401, 403, 404):
            with self.answer(urllib.error.HTTPError("u", code, "no", None, None)):
                self.assertFalse(self.m.pullable(self.ref, wait=0), code)

    def test_a_network_error_fails_the_step_after_its_tries(self):
        import urllib.error
        for error in (urllib.error.URLError("down"), urllib.error.HTTPError("u", 503, "busy", None, None)):
            with self.answer(error) as called, self.assertRaises(SystemExit):
                self.m.pullable(self.ref, tries=3, wait=0)
            self.assertEqual(called.call_count, 3)


if __name__ == "__main__":
    unittest.main()

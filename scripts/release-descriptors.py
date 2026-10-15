#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""The model pipelines' descriptors a release ships (record 55 A5, 2026-10-14).

A tag's release builds every model image (`release.yml`, the `model-images`
job) and pushes it, and this script writes the descriptors that name those
images, each by the digest the release pushed, so a release never ships a
descriptor naming an older image than its own (1.0.0-alpha.80's tag named the
image of 1.0.0-alpha.71, and its own was pinned one commit after it).

    release-descriptors.py encoders <bodypart-embed/nils.job.yml>
        the encoder digests a descriptor pins, as a comma list: the build
        argument ENCODER_DIGESTS, so the image's build fails where the
        weights it bakes are other ones
    release-descriptors.py write --refs <dir> --out <dir> [--tag <tag>]
                                 [--expect <image>,...]
        <dir>/<image>.ref holds `ghcr.io/kineuro/<image>@sha256:<hex>`, one
        for each image the release pushed; writes <out>/pipelines/<package>/
        <name>/nils.job.yml for every descriptor of this tree that names one
        of them, pinned to it, and <out>/IMAGES (`<pipeline>\\t<image>`).
        A kineuro image the release does not build at all (not in --expect)
        ships with the digest the tree pins. An expected image the release
        did not push (its job failed) ships no
        descriptor; <out>/NOT-SHIPPED says which, for the release's notes,
        and the binaries and packs go out regardless.
    release-descriptors.py added <out>/IMAGES < <nils pipeline add --json>
        the release's own binary took a descriptor into a catalog: the image
        it keeps is the one IMAGES names for that pipeline

Only the standard library: it runs on a bare runner. A descriptor is changed
in its `container-image.image` line alone, and gains one comment line.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REGISTRY = "ghcr.io/kineuro/"
DIGEST = re.compile(r"sha256:[0-9a-f]{64}")
REF = re.compile(r"(ghcr\.io/kineuro/[a-z0-9][a-z0-9._-]*)@(sha256:[0-9a-f]{64})")
IMAGE_LINE = re.compile(r'^(\s+image:\s*)"?([^"\s]+)"?\s*$')
NAME_LINE = re.compile(r'^name:\s*"?([A-Za-z0-9._-]+)"?\s*$')


def fail(words: str) -> None:
    sys.exit(f"release-descriptors: {words}")


def image_line(lines: list[str]) -> int | None:
    """The index of the `image:` line of the `container-image:` block."""
    inside = False
    for i, line in enumerate(lines):
        if line.startswith("container-image:"):
            inside = True
            continue
        if inside:
            if line.strip() and not line[0].isspace() and not line.lstrip().startswith("#"):
                return None
            if IMAGE_LINE.match(line):
                return i
    return None


def pipeline_name(lines: list[str]) -> str | None:
    for line in lines:
        m = NAME_LINE.match(line)
        if m:
            return m.group(1)
    return None


def descriptors() -> list[Path]:
    """Every descriptor of a pipeline image of this tree."""
    return sorted(ROOT.glob("pipelines/*/*/nils.job.yml"))


def cmd_encoders(a: argparse.Namespace) -> None:
    pins: list[str] = []
    for line in Path(a.descriptor).read_text().splitlines():
        if line.strip().startswith("encoders:"):
            pins += DIGEST.findall(line)
    if not pins:
        fail(f"{a.descriptor} pins no encoder")
    print(",".join(pins))


def cmd_write(a: argparse.Namespace) -> None:
    built: dict[str, str] = {}
    for f in sorted(Path(a.refs).glob("*.ref")):
        ref = f.read_text().strip()
        m = REF.fullmatch(ref)
        if not m:
            fail(f"{f.name} holds {ref!r}, not ghcr.io/kineuro/<image>@sha256:<hex>")
        built[m.group(1)] = ref
    out = Path(a.out)
    (out / "pipelines").mkdir(parents=True, exist_ok=True)
    expected = [x.strip() for x in (a.expect or "").split(",") if x.strip()]
    missing = sorted(x for x in expected if REGISTRY + x not in built)
    shipped: dict[str, str] = {}
    named: set[str] = set()
    left: list[str] = []
    for path in descriptors():
        text = path.read_text()
        lines = text.splitlines(keepends=True)
        at = image_line(lines)
        if at is None:
            fail(f"{path.relative_to(ROOT)} has no container-image.image line")
        current = IMAGE_LINE.match(lines[at].rstrip("\n")).group(2)
        repository = current.split("@", 1)[0]
        # an image the release does not build at all (post-contrast's
        # fallback, published from a machine with room) ships with the pin
        # the tree has, which must name a digest
        published = (repository.startswith(REGISTRY) and repository not in built
                     and repository[len(REGISTRY):] not in expected)
        if published and not REF.fullmatch(current):
            fail(f"{path.relative_to(ROOT)} names {current}, which no release builds and no digest pins")
        if repository not in built and not published:
            if repository.startswith(REGISTRY):
                left.append(f"{path.relative_to(ROOT)} ({repository}, which this release does not build)")
            continue
        name = pipeline_name(lines)
        package, folder = path.parent.parent.name, path.parent.name
        if name != folder:
            fail(f"{path.relative_to(ROOT)} names the pipeline {name}, not its folder's {folder}")
        if name in shipped:
            fail(f"two descriptors name the pipeline {name}")
        ref = current if published else built[repository]
        prefix = IMAGE_LINE.match(lines[at].rstrip("\n")).group(1)
        lines[at] = f'{prefix}"{ref}"\n'
        # one comment line says where the pin comes from, after the SPDX line
        first = 1 if lines and lines[0].startswith("# SPDX-License-Identifier:") else 0
        said = ("container-image.image is the image published apart from it, as the tree pins it" if published
                else "container-image.image is the image that release built and pushed")
        lines.insert(first, f"# Shipped by the release {a.tag}: {said}.\n")
        dest = out / "pipelines" / package / folder / "nils.job.yml"
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text("".join(lines))
        again = dest.read_text().splitlines()
        at_again = image_line(again)
        if at_again is None or IMAGE_LINE.match(again[at_again]).group(2) != ref:
            fail(f"{dest}: the image line does not name {ref}")
        if [x for i, x in enumerate(again) if i not in (first, at_again)] != [x for i, x in enumerate(text.splitlines()) if i != at]:
            fail(f"{dest}: more than its image line changed")
        shipped[name] = ref
        if not published:
            named.add(repository)
    unnamed = sorted(set(built) - named)
    if unnamed:
        fail(f"no descriptor of this tree names {', '.join(unnamed)}, which the release built")
    (out / "IMAGES").write_text("".join(f"{n}\t{r}\n" for n, r in sorted(shipped.items())))
    for n, r in sorted(shipped.items()):
        print(f"{n}\t{r}")
    for line in left:
        print(f"not shipped: {line}", file=sys.stderr)
    notes = []
    for image in missing:
        lost = sorted(Path(x.split(" (", 1)[0]).parent.name for x in left if f"({REGISTRY}{image}," in x)
        notes.append(f"- `{image}`: its image was not built by this release, so it ships no descriptor"
                     + (f" ({', '.join(lost)})" if lost else "") + "\n")
    (out / "NOT-SHIPPED").write_text("".join(notes))


def cmd_added(a: argparse.Namespace) -> None:
    want = dict(line.split("\t", 1) for line in Path(a.images).read_text().splitlines() if line.strip())
    doc = json.load(sys.stdin)
    name, image = doc.get("name"), doc.get("image")
    if name not in want:
        fail(f"the catalog took {name}, which IMAGES does not name")
    if image != want[name]:
        fail(f"the catalog keeps {name} with the image {image}, and the release built {want[name]}")
    print(f"{doc.get('label')}: {image}")


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = p.add_subparsers(dest="cmd", required=True)
    e = sub.add_parser("encoders")
    e.add_argument("descriptor")
    e.set_defaults(fn=cmd_encoders)
    w = sub.add_parser("write")
    w.add_argument("--refs", required=True)
    w.add_argument("--out", required=True)
    w.add_argument("--tag", default="being made")
    w.add_argument("--expect", default="", help="the images the release builds, as a comma list")
    w.set_defaults(fn=cmd_write)
    d = sub.add_parser("added")
    d.add_argument("images")
    d.set_defaults(fn=cmd_added)
    a = p.parse_args()
    a.fn(a)


if __name__ == "__main__":
    main()

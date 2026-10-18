#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Whether a release builds a model's image, or takes the one a release before it shipped
# (a release builds only what changed).
#
#   scripts/release-image-reuse.sh <image> <sha>    prints "build", or "reuse<TAB><ref><TAB><tag>"
#
# The image's inputs are its package's files, pipelines/<image> less its tests and its README,
# which the image does not hold: its Dockerfile, its requirements, its code, and its
# descriptors, whose pins are its build arguments. Nothing outside the package goes into the
# image, so nothing outside it decides; a change to how the workflow builds (the platforms,
# the build action) rebuilds nothing by itself, and a build that is wanted anyway changes the
# package (its version). The release that last shipped the image is the newest release tag
# among <sha>'s ancestors whose IMAGES names ghcr.io/kineuro/<image> by digest; a release
# whose image job failed names it nowhere, so the one before it is read (twelve tags back at
# most). Where the package's files at <sha> are the same as at that tag, and the registry says
# anyone may pull that digest, the release takes it. Where they differ, where no release before
# shipped the image, or where the digest cannot be pulled without credentials, the release
# builds. The reasons go to stderr. Needs GH_TOKEN and GITHUB_REPOSITORY, and a checkout that
# holds the release tags.
set -euo pipefail
image=$1 sha=$2
repo=${GITHUB_REPOSITORY:?}
here=$(cd "$(dirname "$0")" && pwd)
cd "$(git rev-parse --show-toplevel)"

say() { echo "release-image-reuse: $*" >&2; }

inputs=("pipelines/$image" ":(exclude)pipelines/$image/tests" ":(exclude)pipelines/$image/README.md")

# the release tags among the ancestors, newest first, as the history orders them
tags() {
  git rev-list --first-parent --max-count=400 "$sha^" \
    | while read -r c; do git tag --points-at "$c" --list 'v*'; done \
    | head -n 12
}

base="" ref=""
while read -r tag; do
  [ -n "$tag" ] || continue
  found=$(gh release download "$tag" --repo "$repo" -p IMAGES -O - 2>/dev/null \
    | cut -f2 | grep -m1 -E "^ghcr\.io/kineuro/$image@sha256:[0-9a-f]{64}$" || true)
  if [ -n "$found" ]; then base=$tag ref=$found; break; fi
  say "$tag shipped no $image image"
done < <(tags)

if [ -z "$base" ]; then
  say "no release before this one shipped $image, so it is built"
  echo build
  exit 0
fi
if ! git diff --quiet "$base" "$sha" -- "${inputs[@]}"; then
  say "$image's files changed since $base, so it is built:"
  git diff --stat "$base" "$sha" -- "${inputs[@]}" | tail -20 >&2
  echo build
  exit 0
fi
if ! python3 "$here/release-descriptors.py" pullable "$ref"; then
  say "$ref, which $base shipped, cannot be pulled without credentials, so $image is built"
  echo build
  exit 0
fi
say "$image's files are $base's: its image $ref is taken, not built"
printf 'reuse\t%s\t%s\n' "$ref" "$base"

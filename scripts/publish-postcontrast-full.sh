#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
#
# publish-postcontrast-full.sh: build post-contrast's fallback image
# (pipelines/nils-postcontrast/Dockerfile.full) and push it to
# ghcr.io/kineuro/nils-postcontrast-full, tagged with the package's version;
# print the digest it pushed, which postcontrast-full's descriptor pins.
#
# The release does not build this image: its build pulls FreeSurfer's whole
# image (41 GB unpacked) and writes 34 GB, more than a hosted runner holds.
# Run it on a machine with the room, logged in to ghcr.io with a token that
# may write packages, whenever the fallback's code or image changes, and
# before the tag:
#
#   scripts/publish-postcontrast-full.sh            build, push, print
#   scripts/publish-postcontrast-full.sh --pin      and pin the digest in
#                                                   postcontrast-full/nils.job.yml
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PKG=$ROOT/pipelines/nils-postcontrast
REPO=ghcr.io/kineuro/nils-postcontrast-full
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$PKG/pyproject.toml" | head -1)
[ -n "$version" ] || { echo "no version in $PKG/pyproject.toml" >&2; exit 1; }
revision=$(git -C "$ROOT" rev-parse HEAD)
if [ -n "$(git -C "$ROOT" status --porcelain -- pipelines/nils-postcontrast)" ]; then
  echo "pipelines/nils-postcontrast has changes not committed: an image is published from a commit" >&2
  exit 1
fi
docker build -f "$PKG/Dockerfile.full" -t "$REPO:$version" \
  --label "org.opencontainers.image.source=https://github.com/kineuro/nils" \
  --label "org.opencontainers.image.revision=$revision" \
  --label "org.opencontainers.image.version=$version" \
  "$PKG" >&2
docker push "$REPO:$version" >&2
ref=$(docker image inspect --format '{{range .RepoDigests}}{{println .}}{{end}}' "$REPO:$version" | grep "^$REPO@sha256:" | head -1)
[ -n "$ref" ] || { echo "the push named no digest" >&2; exit 1; }
echo "$ref"
if [ "${1:-}" = --pin ]; then
  desc=$PKG/postcontrast-full/nils.job.yml
  sed -i "s|^  image: \"$REPO@sha256:[0-9a-f]*\"$|  image: \"$ref\"|" "$desc"
  grep -q "image: \"$ref\"" "$desc" || { echo "$desc: the image line was not pinned" >&2; exit 1; }
  echo "pinned in ${desc#"$ROOT"/}" >&2
fi

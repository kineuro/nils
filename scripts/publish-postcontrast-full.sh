#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
#
# publish-postcontrast-full.sh: build post-contrast's fallback image
# (pipelines/nils-postcontrast/Dockerfile.full) and push it to
# ghcr.io/kineuro/nils-postcontrast-full, tagged with the package's version;
# print the digest the push reported, which postcontrast-full's descriptor
# pins, and say whether anyone may pull it. A version is published once: a
# tag that already names another image is refused (raise the version).
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
# a version is published once: a tag already there names its own image, and
# a changed image needs the package's version raised first
there=$(docker buildx imagetools inspect "$REPO:$version" --format '{{json .Manifest.Digest}}' 2>/dev/null | tr -d '"' || true)
docker build -f "$PKG/Dockerfile.full" -t "$REPO:$version" \
  --label "org.opencontainers.image.source=https://github.com/kineuro/nils" \
  --label "org.opencontainers.image.revision=$revision" \
  --label "org.opencontainers.image.version=$version" \
  "$PKG" >&2
if [ -n "$there" ]; then
  if docker image inspect --format '{{range .RepoDigests}}{{println .}}{{end}}' "$REPO:$version" | grep -qx "$REPO@$there"; then
    echo "$REPO:$version is already this image ($there): nothing to push" >&2
    pushed=$there
  else
    echo "$REPO:$version is already published as $there, another image: raise the version in $PKG/pyproject.toml first" >&2
    exit 1
  fi
else
  # the digest the push itself reports
  pushed=$(docker push "$REPO:$version" | tee /dev/stderr | sed -n 's/.*digest: \(sha256:[0-9a-f]\{64\}\).*/\1/p' | tail -1)
  [ -n "$pushed" ] || { echo "the push reported no digest" >&2; exit 1; }
fi
ref="$REPO@$pushed"
echo "$ref"
# anyone must be able to pull it: a site pulls without credentials
token=$(curl -fsS "https://ghcr.io/token?scope=repository:${REPO#ghcr.io/}:pull&service=ghcr.io" | python3 -c 'import json, sys; print(json.load(sys.stdin).get("token", ""))' || true)
code=$(curl -s -o /dev/null -w '%{http_code}' -I -H "Authorization: Bearer $token" \
  -H "Accept: application/vnd.oci.image.index.v1+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json" \
  "https://ghcr.io/v2/${REPO#ghcr.io/}/manifests/$pushed")
if [ "$code" = 200 ]; then
  echo "public: $ref can be pulled without credentials" >&2
else
  echo "NOT PUBLIC: $ref cannot be pulled without credentials (HTTP $code); make the package public in its settings, or the release will not ship its descriptor" >&2
fi
if [ "${1:-}" = --pin ]; then
  desc=$PKG/postcontrast-full/nils.job.yml
  sed -i "s|^  image: \"$REPO@sha256:[0-9a-f]*\"$|  image: \"$ref\"|" "$desc"
  grep -q "image: \"$ref\"" "$desc" || { echo "$desc: the image line was not pinned" >&2; exit 1; }
  echo "pinned in ${desc#"$ROOT"/}" >&2
fi

#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
#
# publish-lst-ai.sh: build the thin LST-AI image (pipelines/lst-ai/Dockerfile,
# on LST-AI's own image by its digest) and push it to
# ghcr.io/kineuro/nils-lst-ai, tagged with LST-AI's version (the descriptor's
# tool-version); print the digest the push reported, which the descriptor
# pins, and say whether anyone may pull it. A tag is published once: where it
# is already there, nothing is built. The image is rebuilt for the same LST-AI
# version with --revision N, which tags it <version>-N.
#
# The release does not build this image: its base is LST-AI's whole image
# (30 GB unpacked, 11 GB to pull and to push), more than a hosted runner's disk
# holds beside the engine build. Run it on a machine with the room, logged in
# to ghcr.io with a token that may write packages, whenever the Dockerfile
# changes, and before the descriptor is used:
#
#   scripts/publish-lst-ai.sh            build, push, print
#   scripts/publish-lst-ai.sh --pin      and pin the digest in
#                                        pipelines/lst-ai/nils.job.yml
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PKG=$ROOT/pipelines/lst-ai
DESC=$PKG/nils.job.yml
REPO=ghcr.io/kineuro/nils-lst-ai
pin=0
revision=
while [ $# -gt 0 ]; do
  case "$1" in
    --pin) pin=1 ;;
    --revision)
      revision=${2:-}
      case "$revision" in '' | *[!0-9]*) echo "--revision takes a number" >&2; exit 2 ;; esac
      shift ;;
    *) echo "usage: scripts/publish-lst-ai.sh [--pin] [--revision N]" >&2; exit 2 ;;
  esac
  shift
done
version=$(sed -n 's/^tool-version: "\(.*\)"$/\1/p' "$DESC" | head -1)
[ -n "$version" ] || { echo "no tool-version in $DESC" >&2; exit 1; }
tag=$version${revision:+-$revision}
# the Dockerfile and the descriptor name the same base image
base=$(sed -n 's/^ARG LST_DIGEST=\(sha256:[0-9a-f]\{64\}\)$/\1/p' "$PKG/Dockerfile")
[ -n "$base" ] || { echo "no base digest (ARG LST_DIGEST) in $PKG/Dockerfile" >&2; exit 1; }
grep -qF "$base" "$DESC" || { echo "$DESC does not name the base image $base that $PKG/Dockerfile builds on" >&2; exit 1; }
revision_id=$(git -C "$ROOT" rev-parse HEAD)
if [ -n "$(git -C "$ROOT" status --porcelain -- pipelines/lst-ai)" ]; then
  echo "pipelines/lst-ai has changes not committed: an image is published from a commit" >&2
  exit 1
fi
# a tag is published once: where it is already there, nothing is built
if there=$(docker buildx imagetools inspect "$REPO:$tag" --format '{{json .Manifest.Digest}}' 2>/dev/null); then
  echo "$REPO:$tag is already published as $(echo "$there" | tr -d '"'): publish a rebuilt image with --revision N" >&2
  exit 1
fi
docker build -f "$PKG/Dockerfile" -t "$REPO:$tag" \
  --label "org.opencontainers.image.source=https://github.com/kineuro/nils" \
  --label "org.opencontainers.image.revision=$revision_id" \
  --label "org.opencontainers.image.version=$tag" \
  "$PKG" >&2
# the digest the push itself reports
pushed=$(docker push "$REPO:$tag" | tee /dev/stderr | sed -n 's/.*digest: \(sha256:[0-9a-f]\{64\}\).*/\1/p' | tail -1)
[ -n "$pushed" ] || { echo "the push reported no digest" >&2; exit 1; }
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
  echo "NOT PUBLIC: $ref cannot be pulled without credentials (HTTP $code); make the package public in its settings, or no site can run the pipeline" >&2
fi
if [ "$pin" = 1 ]; then
  # the image line takes the pushed digest, and the note that it is not
  # pinned yet (between its two marker lines) goes
  sed -i -e "s|^  image: \".*\"\$|  image: \"$ref\"|" -e '/^# --- not pinned yet/,/^# ---$/d' "$DESC"
  grep -qxF "  image: \"$ref\"" "$DESC" || { echo "$DESC: the image line was not pinned" >&2; exit 1; }
  ! grep -q '^# --- not pinned yet' "$DESC" || { echo "$DESC: the note that it is not pinned yet is still there" >&2; exit 1; }
  echo "pinned in ${DESC#"$ROOT"/}" >&2
fi

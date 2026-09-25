#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Writes the multi-frame pyramid fixtures in this folder again: make.py writes
# each file of files.txt as a native file, and DCMTK and GDCM, in a Debian
# container, encode it as the line says. Run it from anywhere; it needs Docker.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
image=nils-fixture-codecs
docker build -q -t "$image" - > /dev/null <<'DOCKERFILE'
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends dcmtk libgdcm-tools python3 && rm -rf /var/lib/apt/lists/*
DOCKERFILE
docker run --rm --user "$(id -u):$(id -g)" -v "$here:/f" -w /f "$image" bash -euo pipefail -c '
work=$(mktemp -d)
python3 make.py "$work"
grep -v "^#" files.txt | while read -r name _ _ how; do
  [ -n "$name" ] || continue
  in="$work/$name.native"
  out="$name.dcm"
  case "$how" in
    native) cp "$in" "$out" ;;
    jpeg-lossless) dcmcjpeg +el +sv 6 "$in" "$out" ;;
    jpeg-lossless-sv1) dcmcjpeg +e1 "$in" "$out" ;;
    jpeg-baseline) dcmcjpeg +eb +q 90 "$in" "$out" ;;
    jpeg-extended) dcmcjpeg +ee +q 90 "$in" "$out" ;;
    jpeg-ls) dcmcjpls +el "$in" "$out" ;;
    jpeg-ls-near) dcmcjpls +en +md 2 "$in" "$out" ;;
    j2k) gdcmconv --j2k "$in" "$out" ;;
    j2k-lossy) gdcmconv --j2k --lossy -q 30 "$in" "$out" ;;
    rle) dcmcrle "$in" "$out" ;;
    deflate) dcmconv +td "$in" "$out" ;;
    big-endian) dcmconv +tb "$in" "$out" ;;
    *) echo "no encoder for $how" >&2; exit 1 ;;
  esac
  # a lossy copy is written as DERIVED, which would split its series
  dcmodify -nb -i "(0008,0008)=ORIGINAL\\PRIMARY" "$out"
done
rm -rf "$work"
'

#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
#
# The official BIDS validator, at one pinned version (Wave 7a section 8.2).
#
#     tools/release-check/bids-validator.sh TREE [validator options]
#
# It is the Deno build the BIDS project publishes on JSR. Deno fetches it once
# into its cache (DENO_DIR) and runs it from there; with the cache filled the
# validator needs no network, so a machine with no route out can be given the
# cache. The version is pinned here and only here: CI, the gate and a person
# checking a release by hand all run this file.
#
# DENO names the deno binary when it is not on the path.
set -euo pipefail

VERSION="3.0.2"

if [[ $# -lt 1 ]]; then
  echo "usage: bids-validator.sh TREE [validator options]" >&2
  exit 2
fi
if [[ "$1" == "--version-pinned" ]]; then
  echo "$VERSION"
  exit 0
fi

deno="${DENO:-$(command -v deno || true)}"
if [[ -z "$deno" ]]; then
  echo "bids-validator: deno is not installed (https://deno.com, or set DENO)" >&2
  exit 3
fi
export DENO_NO_UPDATE_CHECK=1

# Read the tree and the environment the validator asks for, and nothing
# else: no network and no writes. Deno fetches the module itself on first use.
exec "$deno" run --no-prompt --allow-read --allow-env --allow-sys \
  "jsr:@bids/validator@$VERSION" "$@"

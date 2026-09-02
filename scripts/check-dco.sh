#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Every commit in the range that touches contracts/ or sdk/ carries a Signed-off-by
# line, the Developer Certificate of Origin of the Apache-2.0 parts (CONTRIBUTING.md).
# Usage: scripts/check-dco.sh <base-sha> <head-sha>
set -euo pipefail
base=$1
head=$2
fail=0
for c in $(git rev-list "$base".."$head"); do
  if git diff-tree --no-commit-id --name-only -r "$c" | grep -qE '^(contracts|sdk)/'; then
    if ! git show -s --format=%B "$c" | grep -q '^Signed-off-by: '; then
      echo "no Signed-off-by on $(git show -s --format='%h %s' "$c"), which touches contracts/ or sdk/"
      fail=1
    fi
  fi
done
exit $fail

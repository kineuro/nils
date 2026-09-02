#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Text hygiene for every tracked file, as .editorconfig promises: LF endings, a final
# newline, no trailing whitespace (Markdown excepted, where two spaces break a line).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
fail=0
while IFS= read -r f; do
  [ -f "$f" ] || continue
  grep -qI . "$f" || continue # binary or empty
  if grep -q $'\r' "$f"; then echo "CRLF line endings: $f"; fail=1; fi
  if [ -n "$(tail -c1 "$f")" ]; then echo "no final newline: $f"; fail=1; fi
  case "$f" in
    *.md | LICENSE | */LICENSE) ;;
    *) if grep -qE '[[:blank:]]+$' "$f"; then echo "trailing whitespace: $f"; fail=1; fi ;;
  esac
done < <(git ls-files)
exit $fail

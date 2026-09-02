#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Every source file opens with an SPDX header: AGPL-3.0-only for the engine, Apache-2.0
# under contracts/ and sdk/ (docs/decisions/10 and 15, R6).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
fail=0
while IFS= read -r f; do
  case "$f" in
    *.rs | *.go | *.py | *.ts | *.tsx | *.js | *.mjs | *.sh | *.sql | *.c | *.h | *.cpp) ;;
    *) continue ;;
  esac
  case "$f" in
    contracts/* | sdk/*) want=Apache-2.0 ;;
    *) want=AGPL-3.0-only ;;
  esac
  if ! head -n 5 "$f" | grep -q "SPDX-License-Identifier: $want\$"; then
    echo "missing or wrong SPDX header, want $want: $f"; fail=1
  fi
done < <(git ls-files)
exit $fail

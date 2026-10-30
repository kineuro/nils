#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# The release's proof run: the ci run that passed on a tagged commit's tree, and the one that
# still keeps the binaries it built.
#
#   scripts/release-ci-run.sh <sha>    prints "passed=<run id>" and "run=<run id, or empty>"
#                                      (lines for GITHUB_OUTPUT); exits 1 where no ci run
#                                      passed on the commit's tree
#
# A release is tagged on a commit whose checks passed. The run that proves it is main's own
# ci run on the commit or, where main's has not finished (the tag landed on the pull
# request's checks) or failed on a flake, the ci run of the pull request the commit is the
# squash merge of, which ran on exactly this tree: scripts/release-pr-head.sh says when, and
# accepts nothing else. ci.yml runs the same jobs on a pull request and on main, so every
# check main requires is green on some run of the same tree either way; the rule is never
# weaker than that. Where neither passed, the script fails and the release with it: nothing is
# built, pushed or published from a tree no ci run passed. Once main's run on the commit has
# passed, whoever tagged starts the release run again (`gh run rerun <id>`); nothing re-runs it. The binaries come from the first of those runs
# that still keeps all six (an artifact lives seven days), main's first; `run` is empty where
# none does, and the release builds them. Needs GH_TOKEN and GITHUB_REPOSITORY.
set -euo pipefail
sha=$1
repo=${GITHUB_REPOSITORY:?}
here=$(cd "$(dirname "$0")" && pwd)

say() { echo "release-ci-run: $*" >&2; }
kept() { gh api "repos/$repo/actions/runs/$1/artifacts?per_page=100" --jq '[.artifacts[] | select(.expired | not) | select(.name | startswith("nils-"))] | length'; }

main_run=$(gh api "repos/$repo/actions/workflows/ci.yml/runs?head_sha=$sha&branch=main&event=push&status=success" --jq '.workflow_runs[0].id // empty')
main_kept=0
if [ -n "$main_run" ]; then
  main_kept=$(kept "$main_run")
  say "ci run $main_run passed on main for $sha, and keeps $main_kept of the six binaries"
fi
pr="" head="" pr_run=""
if [ -z "$main_run" ] || [ "$main_kept" -lt 6 ]; then
  # the pull request's own run, made against main, on this very tree
  read -r pr head pr_run < <("$here/release-pr-head.sh" "$sha") || true
  if [ -n "$pr_run" ]; then say "ci run $pr_run passed on pull request #$pr, whose head $head has this commit's tree"; fi
fi

passed=${main_run:-$pr_run}
if [ -z "$passed" ]; then
  say "no ci run passed for $sha, on main or on a pull request whose head has this commit's tree:" \
      "a release is tagged on a commit whose checks passed. Once main's ci run on this commit has" \
      "passed, start this release run again; a commit whose checks fail is not released"
  exit 1
fi

run=""
for candidate in $main_run $pr_run; do
  if [ "$candidate" = "$main_run" ]; then n=$main_kept; else n=$(kept "$candidate"); fi
  if [ "$n" -ge 6 ]; then run=$candidate; break; fi
  say "ci run $candidate keeps $n of the six binaries, not all (an artifact lives seven days)"
done
if [ -n "$run" ]; then say "the binaries come from ci run $run"; else say "no ci run keeps this tree's binaries, so the release builds them"; fi
echo "passed=$passed"
echo "run=$run"

#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# The pull request whose checks already ran on exactly a tagged commit's tree (pr-head-tree).
#
#   scripts/release-pr-head.sh <sha>     prints "<pr> <head sha> <run id>", or nothing and why on stderr
#
# It prints the PR only where all of these hold, read from GitHub's API alone:
#   - <sha> is on main, has one parent, and is the squash merge of a merged PR into main;
#   - its tree is byte for byte the PR head's tree;
#   - main's tip before the merge (the parent) is in the head, so the merge of main into the
#     head that the PR's ci ran on was the head's own tree;
#   - the PR's head is a branch of this repository, not a fork's.
# A pull_request run builds merge(head, base at run time), so the run must also be this
# PR's own and have had main as its base. A successful ci run is taken only where:
#   - it ran on the PR head's sha, from this repository and the PR's head branch;
#   - where GitHub still lists the run's pull requests: PR #N is among them with base ref
#     main, and its base sha at run time is an ancestor of the head (compare ahead or
#     identical). A merged PR's runs list none, so there the PR itself must never have been
#     retargeted (no base_ref_changed event) and every PR with that head branch, open or
#     closed, must target main. Main's older tip is then in the head too, since the parent is.
# The PR's ci run then tested and built this commit's tree, and a release or a rules
# release may take it in place of main's run for the same tree. Where no run qualifies the
# script prints nothing, and the caller builds or waits as before. Needs GH_TOKEN and
# GITHUB_REPOSITORY.
set -euo pipefail
sha=$1
repo=${GITHUB_REPOSITORY:?}

say() { echo "release-pr-head: $*" >&2; }

on_main=$(gh api "repos/$repo/compare/main...$sha" --jq .status)
case $on_main in
  behind | identical) ;;
  *) say "$sha is not on main ($on_main)"; exit 0 ;;
esac
parents=$(gh api "repos/$repo/commits/$sha" --jq '.parents | length')
[ "$parents" = 1 ] || { say "$sha has $parents parents, not a squash merge"; exit 0; }
parent=$(gh api "repos/$repo/commits/$sha" --jq '.parents[0].sha')
pr=$(gh api "repos/$repo/commits/$sha/pulls" --jq "[.[] | select(.merged_at != null and .merge_commit_sha == \"$sha\" and .base.ref == \"main\")][0].number // empty")
[ -n "$pr" ] || { say "$sha is no pull request's squash merge into main"; exit 0; }
head=$(gh api "repos/$repo/pulls/$pr" --jq .head.sha)
headrepo=$(gh api "repos/$repo/pulls/$pr" --jq '.head.repo.full_name // empty')
[ "$headrepo" = "$repo" ] || { say "PR #$pr's head is in '${headrepo:-a deleted repository}', not $repo"; exit 0; }
branch=$(gh api "repos/$repo/pulls/$pr" --jq .head.ref)
tree=$(gh api "repos/$repo/git/commits/$sha" --jq .tree.sha)
theirs=$(gh api "repos/$repo/git/commits/$head" --jq .tree.sha)
[ "$tree" = "$theirs" ] || { say "$sha's tree is not PR #$pr's head's tree"; exit 0; }
up=$(gh api "repos/$repo/compare/$parent...$head" --jq .status)
case $up in
  ahead | identical) ;;
  *) say "PR #$pr's head was behind main when merged ($up)"; exit 0 ;;
esac

# the base of the PR is main now; a merged PR's runs list no pull requests, so show the PR
# was never anywhere else
retargeted=$(gh api --paginate "repos/$repo/issues/$pr/events" --jq '[.[] | select(.event == "base_ref_changed")] | length' | awk '{s+=$1} END{print s+0}')
others=$(gh api --paginate "repos/$repo/pulls?head=${repo%%/*}:$branch&state=all&per_page=100" --jq '[.[] | select(.base.ref != "main")] | length' | awk '{s+=$1} END{print s+0}')
bare_ok=no
if [ "${retargeted:-1}" = 0 ] && [ "${others:-1}" = 0 ]; then bare_ok=yes; fi

run=""
while IFS=$'\t' read -r id listed basesha; do
  [ -n "$id" ] || continue
  if [ "$listed" = yes ]; then
    # GitHub lists this PR on the run: its base at run time was main (checked in jq)
    # and must be an ancestor of the head
    st=$(gh api "repos/$repo/compare/$basesha...$head" --jq .status 2>/dev/null || true)
    case $st in
      ahead | identical) run=$id; break ;;
      *) say "run $id had base $basesha, which is not an ancestor of the head ($st)" ;;
    esac
  elif [ "$listed" = none ] && [ "$bare_ok" = yes ]; then
    run=$id; break
  else
    say "run $id is not shown to be PR #$pr's own run on main"
  fi
done < <(gh api "repos/$repo/actions/workflows/ci.yml/runs?head_sha=$head&event=pull_request&status=success&per_page=100" --jq "
  .workflow_runs[]
  | select(.head_repository.full_name == \"$repo\" and .head_branch == \"$branch\")
  | if (.pull_requests | length) == 0 then [.id, \"none\", \"\"]
    elif any(.pull_requests[]; .number == $pr and .base.ref == \"main\") then
      [.id, \"yes\", ([.pull_requests[] | select(.number == $pr)][0].base.sha)]
    else [.id, \"other\", \"\"] end
  | @tsv")
[ -n "$run" ] || { say "no ci run of PR #$pr's own on main qualifies (retargeted: ${retargeted:-?}, other bases: ${others:-?})"; exit 0; }
say "$sha has PR #$pr's head's tree ($head), and its ci run $run ran on that tree"
echo "$pr $head $run"

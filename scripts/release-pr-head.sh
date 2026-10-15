#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# The pull request whose checks already ran on exactly a tagged commit's tree (pr-head-tree).
#
#   scripts/release-pr-head.sh <sha>     prints "<pr> <head sha>", or nothing and why on stderr
#
# It prints the PR only where all of these hold, read from GitHub's API alone:
#   - <sha> is on main, has one parent, and is the squash merge of a merged PR into main;
#   - its tree is byte for byte the PR head's tree;
#   - main's tip before the merge (the parent) is in the head, so the merge of main into the
#     head that the PR's ci ran on was the head's own tree.
# The PR's ci run then tested and built this commit's tree, and a release or a rules
# release may take it in place of main's run for the same tree. Needs GH_TOKEN and
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
tree=$(gh api "repos/$repo/git/commits/$sha" --jq .tree.sha)
theirs=$(gh api "repos/$repo/git/commits/$head" --jq .tree.sha)
[ "$tree" = "$theirs" ] || { say "$sha's tree is not PR #$pr's head's tree"; exit 0; }
up=$(gh api "repos/$repo/compare/$parent...$head" --jq .status)
case $up in
  ahead | identical) ;;
  *) say "PR #$pr's head was behind main when merged ($up)"; exit 0 ;;
esac
say "$sha has PR #$pr's head's tree ($head), and its ci ran on that tree"
echo "$pr $head"

#!/bin/bash
# Keep Olo's latest release on GitHub and the two before it, and delete
# the older ones (Ben, 2026-11-26: "for the releases in github let's just
# keep latest + 2 previous ones"; each is 2.3 GB of files, and there were
# 35).
#
#   tools/release-prune.sh [--dry-run] [KEEP]
#
# KEEP: how many to keep, the newest first by when each was published (3
# unless said). --dry-run says what would go and deletes nothing.
#
# What goes is the release and its files. Its tag stays: a tag is what
# names a release (tools/publish-release.sh gives a day's next one the
# first name with neither a release nor a tag, so a name is never given
# twice), and it still names the commit that release was built from.
# Drafts and pre-releases are someone's work in hand: neither counted
# nor deleted.
#
# It prints "kept TAG" and "deleted TAG" a line each, and its last line is
# "pruned: N deleted, M kept" (or, with --dry-run, "would delete N"). A
# release GitHub wouldn't delete is said ("couldn't delete TAG") and the
# rest still go; its exit is 0 only when every one that should go went.
# tools/publish-release.sh runs it once a new release is up whole.
set -euo pipefail
DRY=0
if [ "${1:-}" = --dry-run ]; then DRY=1; shift; fi
KEEP=${1:-3}
case "$KEEP" in
    '' | *[!0-9]* | 0) echo "usage: tools/release-prune.sh [--dry-run] [KEEP] (KEEP: 1 or more)" >&2; exit 2 ;;
esac
cd "$(dirname "$0")/.."
# (Asked once, whole: a list GitHub couldn't give must not read as "no
# releases", nor as fewer than there are.)
if ! list=$(gh release list --limit 1000 --exclude-drafts --exclude-pre-releases --json tagName,publishedAt -q 'sort_by(.publishedAt) | reverse | .[].tagName'); then
    echo "release-prune: GitHub didn't list the releases; nothing deleted" >&2
    exit 1
fi
mapfile -t tags < <(printf '%s\n' "$list" | sed '/^$/d')
deleted=0 failed=0 kept=0
for i in "${!tags[@]}"; do
    tag=${tags[$i]}
    if [ "$i" -lt "$KEEP" ]; then
        echo "kept $tag"
        kept=$((kept + 1))
    elif [ "$DRY" = 1 ]; then
        echo "would delete $tag"
        deleted=$((deleted + 1))
    elif gh release delete "$tag" --yes > /dev/null 2>&1; then
        echo "deleted $tag"
        deleted=$((deleted + 1))
    else
        echo "couldn't delete $tag"
        failed=$((failed + 1))
    fi
done
if [ "$DRY" = 1 ]; then
    echo "would delete $deleted, keep $kept"
    exit 0
fi
echo "pruned: $deleted deleted, $kept kept$([ "$failed" -gt 0 ] && echo ", $failed GitHub wouldn't delete")"
[ "$failed" = 0 ]

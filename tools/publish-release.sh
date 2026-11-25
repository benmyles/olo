#!/bin/bash
# Make and publish Olo's release on GitHub from this tree's HEAD, in one
# go and the same way each time (Ben, 2026-11-24: "add a script/tool or
# something to publish the release more reliably in the future too, and
# make sure you always run it in the background so you can move on with
# work instead of waiting for a slow upload").
#
#   tools/publish-release.sh NOTES.md [--dry-run]
#
# NOTES.md: the release's notes, as the last release's are written
# ("What's new", "What's fixed", "Files"); VERSION in it is put in as the
# release's own (olo-VERSION-x86_64.tar.zst). --dry-run makes and checks
# everything and publishes nothing.
#
# Run it in the background: building, packing and uploading 2.3 GB take a
# while, and nothing in it needs anyone. Everything it does is written to
# $WORK/publish.log (WORK: /var/tmp/olo-release unless OLO_RELEASE_WORK
# says), its last line "published URL", "dry run: … made and checked", or
# "failed: WHY", and its exit is 0 only when the release is up whole.
#
# What it does, each step stopping it where it fails:
#  1. HEAD is on origin (a release's tag names a commit GitHub has), and
#     is checked out on its own (tools/hooks/checkout-at, $WORK/src): the
#     build reads that, not the working tree, so work goes on meanwhile.
#  2. Builds the workspace in release in a Fedora 44 container
#     (tools/release-image: the oldest Fedora a release is for, its
#     programs linking that Fedora's own libraries and glibc: built on
#     another system, a release may not start on Fedora at all), with
#     Jato, in a target folder and a Jato cache of the release's own.
#  3. Makes the files with tools/release.sh, in the same container.
#  4. Unpacks them as Setup does (olo-release's a_real_release_unpacks).
#  5. Unpacks Olo in plain Fedora 44 and Fedora 45 containers and asks
#     every program what it can't find (ldd): none may lack a library.
#  6. Names it vYYYY.MM.DD, "Olo YYYY.MM.DD"; a second one on a day
#     vYYYY.MM.DD.2, "Olo YYYY.MM.DD (update 2)", and so on.
#  7. Publishes it at HEAD's full hash with the five files, as the
#     latest; a file that didn't go up is sent again (three tries).
#  8. Checks GitHub has each file whole (its size).
set -euo pipefail
NOTES=${1:?usage: tools/publish-release.sh NOTES.md [--dry-run]}
NOTES=$(realpath "$NOTES")
cd "$(dirname "$0")/.."
REPO=$(pwd)
DRY=0
[ "${2:-}" = --dry-run ] && DRY=1
WORK=${OLO_RELEASE_WORK:-/var/tmp/olo-release}
mkdir -p "$WORK/target" "$WORK/jato" "$HOME/.cargo/registry"
LOG="$WORK/publish.log"
: > "$LOG"
say() { echo "$(date -u +%H:%M:%S) $*" | tee -a "$LOG"; }
fail() { say "failed: $*"; exit 1; }
step() { say "== $*"; }
IMAGE=olo-fedora44-build
DOCKER="docker"
docker info > /dev/null 2>&1 || DOCKER="sudo docker"
USERNS=()
if $DOCKER --version 2>/dev/null | grep -qi podman && [ "$(id -u)" != 0 ]; then
    USERNS=(--userns=keep-id)
fi
# The container sees the tree at its own path (paths in the programs are
# the tree's), the release's target folder over its target/, and Jato.
in_image() {
    # (The checkout is a worktree: its .git names the repository's own,
    # which it must see, read-only, to say what commit it is.)
    $DOCKER run --rm "${USERNS[@]}" --user "$(id -u):$(id -g)" \
        -v "$ROOT:$ROOT" -v "$WORK/target:$ROOT/target" -v "$REPO/.git:$REPO/.git:ro" -v "$HOME/.cargo/registry:/home/builder/.cargo/registry" \
        -v "$(command -v jato):/usr/local/bin/jato:ro" -v "$WORK/jato:/jato" -e JATO_CACHE_DIR=/jato -e HOME=/home/builder \
        "$@" -w "$ROOT" "$IMAGE" sh -c "git config --global --add safe.directory '$ROOT'; git config --global --add safe.directory '$REPO'; $IN"
}

step "1. the commit"
[ -f "$NOTES" ] || fail "no notes at $NOTES"
git fetch -q origin
SHA=$(git rev-parse HEAD)
git merge-base --is-ancestor "$SHA" origin/main || fail "HEAD ($SHA) isn't on origin/main: push it first"
# (Built from a checkout of its own: the working tree may change meanwhile.)
ROOT=$(tools/hooks/checkout-at "$SHA" "$WORK/src" 2>> "$LOG") || fail "couldn't check out $SHA (see $LOG)"
cd "$ROOT"
say "HEAD $SHA, on origin/main, checked out in $ROOT"

step "2. the release build, in Fedora 44"
$DOCKER build -q --build-arg UID="$(id -u)" --build-arg GID="$(id -g)" -t "$IMAGE" tools/release-image >> "$LOG" 2>&1 || fail "the build image couldn't be made"
IN="jato build --release --workspace" in_image >> "$LOG" 2>&1 || fail "the release build failed (see $LOG)"
say "built"

step "3. the files"
rm -rf "$WORK/target/dist"
IN="tools/release.sh target/dist" in_image >> "$LOG" 2>&1 || fail "tools/release.sh failed (see $LOG)"
DIST="$WORK/target/dist"
ARCHIVE=$(ls "$DIST"/olo-*-x86_64.tar.zst | grep -v wallpapers | head -1)
VERSION=$(basename "$ARCHIVE" | sed -E 's/^olo-(.*)-x86_64\.tar\.zst$/\1/')
FILES=("$DIST/olo-installer-x86_64" "$DIST/olo-installer-x86_64.sha256" "$ARCHIVE" "$ARCHIVE.sha256" "$DIST/olo-wallpapers-$VERSION.tar.zst")
for f in "${FILES[@]}"; do [ -s "$f" ] || fail "tools/release.sh didn't make $(basename "$f")"; done
say "Olo $VERSION: $(du -ch "${FILES[@]}" | tail -1 | cut -f1) in five files"

step "4. unpacked as Setup does"
IN="jato test --release -p olo-release a_real_release -- --ignored --nocapture" in_image -e "OLO_RELEASE=$ROOT/target/dist/$(basename "$ARCHIVE")" >> "$LOG" 2>&1 || fail "the release doesn't unpack (see $LOG)"
grep -q "test tests::a_real_release_unpacks ... ok" "$LOG" || fail "the unpacking test didn't run"
say "$(grep -E '^version .* unpacked' "$LOG" | tail -1)"

step "5. every program's libraries, on Fedora 44 and 45"
cat > "$WORK/ldd-check.sh" <<'EOF'
set -e
dnf -y -q install libinput libseat libdisplay-info libxkbcommon mesa-libgbm mesa-libEGL libdrm pixman freetype harfbuzz fontconfig pipewire-libs elfutils-libelf pam vulkan-loader sqlite-libs openssl-libs zstd tar > /dev/null 2>&1
mkdir -p /x && cd /x && zstd -dc /dist/$ARCHIVE | tar -xf -
n=0; for b in olo/bin/*; do out=$(ldd "$b" 2>&1 | grep "not found" || true); if [ -n "$out" ]; then echo "$b: $out"; n=$((n+1)); fi; done
echo "missing: $n of $(ls olo/bin | wc -l), $(ldd --version | head -1)"
EOF
for v in 44 45; do
    out=$($DOCKER run --rm -e "ARCHIVE=$(basename "$ARCHIVE")" -v "$DIST:/dist:ro" -v "$WORK/ldd-check.sh:/c.sh:ro" "registry.fedoraproject.org/fedora:$v" sh /c.sh 2>&1 | tail -20)
    echo "$out" >> "$LOG"
    echo "$out" | grep -q "^missing: 0 of" || fail "on Fedora $v a program lacks a library: $(echo "$out" | grep -v '^missing' | head -3)"
    say "Fedora $v: $(echo "$out" | grep '^missing')"
done

step "6. its name"
DAY=$(echo "$VERSION" | cut -d- -f1)
TAG="v$DAY"; TITLE="Olo $DAY"; n=1
while gh release view "$TAG" > /dev/null 2>&1 || git ls-remote --exit-code --tags origin "refs/tags/$TAG" > /dev/null 2>&1; do
    n=$((n + 1)); TAG="v$DAY.$n"; TITLE="Olo $DAY (update $n)"
done
sed "s/VERSION/$VERSION/g" "$NOTES" > "$WORK/notes.md"
say "$TAG, \"$TITLE\""

if [ "$DRY" = 1 ]; then
    say "dry run: $TAG made and checked in $DIST, nothing published"
    exit 0
fi

step "7. published"
if ! gh release create "$TAG" --title "$TITLE" --target "$SHA" --notes-file "$WORK/notes.md" --latest "${FILES[@]}" >> "$LOG" 2>&1; then
    gh release view "$TAG" > /dev/null 2>&1 || fail "GitHub didn't take the release (see $LOG)"
    say "the release is there, some of its files may not be: sent again below"
fi
# (A file that didn't go up, or went up short, is sent again. What gh
# says when it can't answer goes to the log: an empty answer was read as
# "not whole", and update 8's check failed on a file that was.)
whole() { gh release view "$TAG" --json assets -q ".assets[] | select(.name == \"$(basename "$1")\") | .size" 2>>"$LOG"; }
for f in "${FILES[@]}"; do
    for try in 1 2 3; do
        [ "$(whole "$f")" = "$(stat -c %s "$f")" ] && break
        say "$(basename "$f") isn't whole there: sending it again ($try of 3)"
        gh release upload "$TAG" "$f" --clobber >> "$LOG" 2>&1 || true
    done
done

step "8. checked"
# (Asked again, a few seconds apart, before a file is said not to be
# whole: GitHub's answer just after a large upload can lag or fail, and
# update 8's wallpapers, whole there, were said not to be 7 s after they
# were seen whole.)
for f in "${FILES[@]}"; do
    want=$(stat -c %s "$f")
    for try in 1 2 3 4 5; do
        got=$(whole "$f")
        [ "$got" = "$want" ] && break
        [ "$try" = 5 ] && fail "$(basename "$f") isn't whole on GitHub: it says ${got:-nothing} of $want bytes"
        say "$(basename "$f"): GitHub says ${got:-nothing} of $want bytes; asking again in 10 s ($try of 5)"
        sleep 10
    done
done
URL=$(gh release view "$TAG" --json url -q .url)
say "published $URL"

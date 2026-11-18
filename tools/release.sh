#!/bin/sh
# Make Olo's release: what the installer (apps/installer, olo-installer)
# downloads and installs on a stock Fedora, from this tree's release
# build.
#
#   tools/release.sh [OUT]     (default OUT: target/dist)
#
# Writes, for this machine's architecture (x86_64 or aarch64):
#   olo-VERSION-ARCH.tar.zst         Olo itself
#   olo-wallpapers-VERSION.tar.zst   its wallpapers (any architecture): a
#                                    part of their own, as with them Olo
#                                    is more than the 2 GiB a file GitHub
#                                    takes
#   olo-VERSION-ARCH.tar.zst.sha256  both archives' SHA-256, as
#                                    `sha256sum` writes them: the
#                                    installer takes every other archive
#                                    named there as a part, fetches it
#                                    from beside the first and unpacks it
#                                    over it
#   olo-installer-ARCH               the installer (one program)
#   olo-installer-ARCH.sha256
# VERSION is the date and the commit (2026.11.07-60cbfff5). The bundle
# holds, under olo/:
#   bin/       every olo-* program, stripped, pam_olo_keyring.so and
#              libpam_olo_face.so
#   share/olo/ Olo's data: icons, brand, fonts, the models that read
#              text in pictures, the themes' icons (what an upgrade
#              installs: crates/session/src/upgrade.rs); the wallpapers'
#              archive holds olo/share/olo/wallpapers
#   system/    what goes outside the home folder: the login screen's
#              session entry, udev rules, PAM services, the login screen's
#              unit, user and folders (data/session, udev, pam, systemd,
#              sysusers, tmpfiles)
#   VERSION, LICENSE
# It builds nothing: run `cargo build --release --workspace` first (the
# installer is part of the workspace). Publishing (a GitHub release with
# these five files) is a separate, deliberate step.
set -eu
cd "$(dirname "$0")/.."
OUT=${1:-target/dist}
ARCH=$(uname -m)
VERSION="$(date -u +%Y.%m.%d)-$(git rev-parse --short=8 HEAD)"
STAGE="$OUT/stage"
rm -rf "$STAGE"
mkdir -p "$STAGE/olo/bin" "$STAGE/olo/share/olo/themes" "$STAGE/olo/system"
for b in target/release/olo-*; do
    [ -x "$b" ] && [ -f "$b" ] || continue
    name=$(basename "$b")
    # (Not the scheduler, scx_lavd as Olo carries it: a GPL program is
    # passed on with its source, which is the publisher's to arrange:
    # tools/build-scheduler.sh.)
    case "$name" in *.*) continue ;; olo-installer | olo-scx-lavd) continue ;; esac
    strip -o "$STAGE/olo/bin/$name" "$b"
done
strip -o "$STAGE/olo/bin/pam_olo_keyring.so" target/release/libpam_olo_keyring.so
# Face unlock's PAM module, beside the programs, where Preferences ▸
# Security looks for it to install (with olo-faced and the models,
# share/olo/face): a released Olo had neither, and its Face Unlock
# said "isn't in this build".
strip -o "$STAGE/olo/bin/libpam_olo_face.so" target/release/libpam_olo_face.so
# Olo's data, as an upgrade installs it (crates/session/src/upgrade.rs,
# sync_data), never a folder whole: the tree also holds what the
# pictures are made from (data/icons/src, the wallpapers' src: 2.5 GB,
# not in the repository), which went into the bundle and made it 4.7 GB
# (2026-11-17; GitHub takes 2 GiB a file).
SHARE="$STAGE/olo/share/olo"
# An icon set's drawings (svg/), and a theme's colours for the classic
# set's (recolour.txt); not its sources.
drawings() {
    mkdir -p "$2"
    if [ -d "$1/svg" ]; then cp -r "$1/svg" "$2/"; fi
    if [ -f "$1/recolour.txt" ]; then cp "$1/recolour.txt" "$2/"; fi
}
drawings data/icons "$SHARE/icons"
for theme in data/themes/*/; do
    theme=${theme%/}
    if [ -d "$theme/icons" ]; then drawings "$theme/icons" "$SHARE/themes/${theme##*/}/icons"; fi
done
# The brand's pictures, the fonts, the models that read the text in
# pictures and that know a face (both were left out: a released Olo
# read no text and had no face unlock).
cp -r data/brand data/fonts data/ocr data/face "$SHARE/"
# The wallpaper pack, in an archive of its own: its list, both shapes
# (with their HDR gain maps), the previews and the tiles.
WALLS="$OUT/stage-wallpapers/olo/share/olo/wallpapers"
rm -rf "$OUT/stage-wallpapers"
mkdir -p "$WALLS"
cp data/wallpapers/pack.txt "$WALLS/"
for sub in 4k ultrawide thumbs tiles; do
    cp -r --reflink=auto "data/wallpapers/$sub" "$WALLS/"
done
for d in session udev pam systemd sysusers tmpfiles; do
    cp -r "data/$d" "$STAGE/olo/system/"
done
echo "$VERSION" > "$STAGE/olo/VERSION"
# (MIT: the notice goes with every copy.)
cp LICENSE "$STAGE/olo/LICENSE"
BUNDLE="olo-$VERSION-$ARCH.tar.zst"
tar -C "$STAGE" --owner=0 --group=0 --numeric-owner --sort=name -cf - olo | zstd -q -19 -T0 -o "$OUT/$BUNDLE" -f
# (The pictures are compressed already: packed fast.)
WALLPAPERS="olo-wallpapers-$VERSION.tar.zst"
tar -C "$OUT/stage-wallpapers" --owner=0 --group=0 --numeric-owner --sort=name -cf - olo | zstd -q -3 -T0 -o "$OUT/$WALLPAPERS" -f
rm -rf "$OUT/stage-wallpapers"
(cd "$OUT" && sha256sum "$BUNDLE" "$WALLPAPERS" > "$BUNDLE.sha256")
# (A GitHub release takes files under 2 GiB.)
for f in "$BUNDLE" "$WALLPAPERS"; do
    SIZE=$(stat -c %s "$OUT/$f")
    if [ "$SIZE" -ge 2147483648 ]; then
        echo "release.sh: $f is $SIZE bytes: 2 GiB or more, which GitHub won't take" >&2
        exit 1
    fi
done
strip -o "$OUT/olo-installer-$ARCH" target/release/olo-installer
(cd "$OUT" && sha256sum "olo-installer-$ARCH" > "olo-installer-$ARCH.sha256")
rm -rf "$STAGE"
ls -l "$OUT/$BUNDLE" "$OUT/$WALLPAPERS" "$OUT/olo-installer-$ARCH"
echo "Olo $VERSION for $ARCH in $OUT."

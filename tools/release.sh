#!/bin/sh
# Make Olo's release: what the installer (apps/installer, olo-installer)
# downloads and installs on a stock Fedora, from this tree's release
# build.
#
#   tools/release.sh [OUT]     (default OUT: target/dist)
#
# Writes, for this machine's architecture (x86_64 or aarch64):
#   olo-VERSION-ARCH.tar.zst         Olo itself
#   olo-VERSION-ARCH.tar.zst.sha256  its SHA-256, as `sha256sum` writes it
#   olo-installer-ARCH               the installer (one program)
#   olo-installer-ARCH.sha256
# VERSION is the date and the commit (2026.11.07-60cbfff5). The bundle
# holds, under olo/:
#   bin/       every olo-* program, stripped, and pam_olo_keyring.so
#   share/olo/ Olo's data: icons, brand, fonts, the themes' icons and
#              wordmarks, the wallpapers (as tools/install-session.sh
#              copies them)
#   system/    what goes outside the home folder: the login screen's
#              session entry, udev rules, PAM services, the login screen's
#              unit, user and folders (data/session, udev, pam, systemd,
#              sysusers, tmpfiles)
#   VERSION, LICENSE
# It builds nothing: run `cargo build --release --workspace` first (the
# installer is part of the workspace). Publishing (a GitHub release with
# these four files) is a separate, deliberate step.
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
    case "$name" in *.*) continue ;; olo-installer) continue ;; esac
    strip -o "$STAGE/olo/bin/$name" "$b"
done
strip -o "$STAGE/olo/bin/pam_olo_keyring.so" target/release/libpam_olo_keyring.so
cp -r data/icons data/brand data/fonts data/wallpapers "$STAGE/olo/share/olo/"
for theme in data/themes/*/; do
    theme=${theme%/}
    mkdir -p "$STAGE/olo/share/olo/themes/${theme##*/}"
    cp -r "$theme/icons" "$theme/brand" "$STAGE/olo/share/olo/themes/${theme##*/}/"
done
for d in session udev pam systemd sysusers tmpfiles; do
    cp -r "data/$d" "$STAGE/olo/system/"
done
echo "$VERSION" > "$STAGE/olo/VERSION"
# (MIT: the notice goes with every copy.)
cp LICENSE "$STAGE/olo/LICENSE"
BUNDLE="olo-$VERSION-$ARCH.tar.zst"
tar -C "$STAGE" --owner=0 --group=0 --numeric-owner --sort=name -cf - olo | zstd -q -19 -T0 -o "$OUT/$BUNDLE" -f
(cd "$OUT" && sha256sum "$BUNDLE" > "$BUNDLE.sha256")
strip -o "$OUT/olo-installer-$ARCH" target/release/olo-installer
(cd "$OUT" && sha256sum "olo-installer-$ARCH" > "olo-installer-$ARCH.sha256")
rm -rf "$STAGE"
ls -l "$OUT/$BUNDLE" "$OUT/olo-installer-$ARCH"
echo "Olo $VERSION for $ARCH in $OUT."

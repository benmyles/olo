#!/bin/sh
# Install Tabula for the login screen: release binaries into
# $PREFIX/bin, icons into $PREFIX/share/tabula and the session entry into
# $PREFIX/share/wayland-sessions (GDM and SDDM look there via
# XDG_DATA_DIRS). Re-running it upgrades a running session in place:
# afterwards `tabula-session reload` picks up the new binaries.
set -eu
PREFIX=${PREFIX:-/usr/local}
cd "$(dirname "$0")/.."
if [ "$(id -u)" = 0 ] && [ -n "${SUDO_USER:-}" ]; then
    sudo -u "$SUDO_USER" cargo build --release --workspace
else
    cargo build --release --workspace
fi
install -d "$PREFIX/bin" "$PREFIX/share/wayland-sessions" "$PREFIX/share/tabula"
for b in target/release/tabula-*; do
    [ -x "$b" ] && [ -f "$b" ] || continue
    # Install to a temporary name and rename, so running programs keep
    # their old file and notice the new one (in-place reload).
    install -m 755 "$b" "$PREFIX/bin/.$(basename "$b").new"
    mv -f "$PREFIX/bin/.$(basename "$b").new" "$PREFIX/bin/$(basename "$b")"
done
cp -r data/icons "$PREFIX/share/tabula/"
install -m 644 data/session/tabula.desktop "$PREFIX/share/wayland-sessions/tabula.desktop"
echo "Installed to $PREFIX. Choose Tabula on the login screen, or run 'tabula-session reload' in a running session."

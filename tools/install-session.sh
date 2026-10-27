#!/bin/sh
# Install Tabula for the login screen.
#
#   tools/install-session.sh          (as yourself; asks for sudo once)
#
# Builds release binaries and installs them into $PREFIX/bin (default
# ~/.local/bin, so upgrades never need root), icons into
# $PREFIX/share/tabula, and records this tree's target/release in
# $PREFIX/share/tabula/build-dir: a running session watches it and offers
# newer builds in Deskbar's tray (or run `tabula-session upgrade`). The
# login screen's entry goes into /usr/local/share/wayland-sessions (GDM
# and SDDM look there), which needs root.
#
# Re-running it upgrades in place; `tabula-session reload` (or the tray)
# picks the new programs up without logging out.
set -eu
cd "$(dirname "$0")/.."
TREE=$(pwd -P)
# Whose session: the invoking user (also when run with sudo).
USER_NAME=${SUDO_USER:-$(id -un)}
USER_HOME=$(getent passwd "$USER_NAME" | cut -d: -f6)
PREFIX=${PREFIX:-$USER_HOME/.local}
SESSIONS=${SESSIONS:-/usr/local/share/wayland-sessions}

as_user() {
    if [ "$(id -u)" = 0 ] && [ "$USER_NAME" != root ]; then sudo -u "$USER_NAME" "$@"; else "$@"; fi
}
as_root() {
    if [ "$(id -u)" = 0 ]; then "$@"; else sudo "$@"; fi
}

as_user cargo build --release --workspace
as_user mkdir -p "$PREFIX/bin" "$PREFIX/share/tabula"
for b in target/release/tabula-*; do
    [ -x "$b" ] && [ -f "$b" ] || continue
    name=$(basename "$b")
    case "$name" in *.*) continue ;; esac
    # Under a temporary name, then renamed: running programs keep their
    # old file and notice the new one (in-place reload). A clone where
    # the file system can (btrfs), else a copy.
    as_user cp --reflink=auto "$b" "$PREFIX/bin/.$name.new"
    as_user chmod 755 "$PREFIX/bin/.$name.new"
    as_user mv -f "$PREFIX/bin/.$name.new" "$PREFIX/bin/$name"
done
as_user cp -r data/icons "$PREFIX/share/tabula/"
echo "$TREE/target/release" | as_user tee "$PREFIX/share/tabula/build-dir" > /dev/null

# The login screen runs the session from where it was installed (no
# TryExec: the greeter can't look inside a private home).
BIN=$(cd "$PREFIX/bin" && pwd -P)
ENTRY=$(mktemp)
sed -e "s|^Exec=.*|Exec=$BIN/tabula-session|" -e '/^TryExec=/d' data/session/tabula.desktop > "$ENTRY"
as_root install -D -m 644 "$ENTRY" "$SESSIONS/tabula.desktop"
rm -f "$ENTRY"
# Brightness of USB-controlled monitors (Studio Display and the like).
as_root install -D -m 644 data/udev/70-tabula-displays.rules /etc/udev/rules.d/70-tabula-displays.rules
as_root udevadm control --reload 2>/dev/null || true
as_root udevadm trigger --subsystem-match=hidraw --action=change 2>/dev/null || true
echo "Installed to $PREFIX; Tabula is on the login screen's session menu."

#!/bin/sh
# Install Olo for the login screen.
#
#   tools/install-session.sh          (as yourself; asks for sudo once)
#
# Builds release binaries and installs them into $PREFIX/bin (default
# ~/.local/bin, so upgrades never need root), icons into
# $PREFIX/share/olo, and records this tree's target/release in
# $PREFIX/share/olo/build-dir: a running session watches it and offers
# newer builds in Deskbar's tray (or run `olo-session upgrade`). The
# login screen's entry goes into /usr/local/share/wayland-sessions (GDM
# and SDDM look there), which needs root.
#
# Re-running it upgrades in place; `olo-session reload` (or the tray)
# picks the new programs up without logging out.
set -eu
cd "$(dirname "$0")/.."
TREE=$(pwd -P)
# Whose session: the invoking user (also when run with sudo; SUDO_USER
# counts only then, not under `sudo -u someone`, where it names root).
USER_NAME=$(id -un)
[ "$(id -u)" = 0 ] && [ -n "${SUDO_USER:-}" ] && USER_NAME=$SUDO_USER
USER_HOME=$(getent passwd "$USER_NAME" | cut -d: -f6)
PREFIX=${PREFIX:-$USER_HOME/.local}
SESSIONS=${SESSIONS:-/usr/local/share/wayland-sessions}

as_user() {
    if [ "$(id -u)" = 0 ] && [ "$USER_NAME" != root ]; then sudo -u "$USER_NAME" "$@"; else "$@"; fi
}
as_root() {
    if [ "$(id -u)" = 0 ]; then "$@"; else sudo "$@"; fi
}

# The layout lint first (docs/HACKING.md, "Layout lint"): a window whose
# content doesn't fit stops the install.
as_user cargo ui-lint
as_user cargo build --release --workspace
# An install from before the rename (Tabula): its folders become Olo's,
# with links by the old names (as every Olo program does at start, but
# before this makes the new ones), so nothing is lost.
for base in "${XDG_CONFIG_HOME:-$USER_HOME/.config}" "${XDG_CACHE_HOME:-$USER_HOME/.cache}" \
    "${XDG_STATE_HOME:-$USER_HOME/.local/state}" "${XDG_DATA_HOME:-$USER_HOME/.local/share}" "$PREFIX/share"; do
    if [ -d "$base/tabula" ] && [ ! -L "$base/tabula" ] && [ ! -e "$base/olo" ]; then
        as_user mv "$base/tabula" "$base/olo" && as_user ln -s olo "$base/tabula"
    fi
done
as_user mkdir -p "$PREFIX/bin" "$PREFIX/share/olo"
for b in target/release/olo-*; do
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
# The programs' old names (Tabula) become links to the new ones: the old
# login screen entry runs tabula-session, and programs still running
# re-execute themselves by the names they were started by.
for old in "$PREFIX"/bin/tabula-*; do
    [ -e "$old" ] || [ -L "$old" ] || continue
    new=olo-${old##*/tabula-}
    [ -f "$PREFIX/bin/$new" ] || continue
    as_user ln -sfn "$new" "$PREFIX/bin/.${old##*/}.link"
    as_user mv -f "$PREFIX/bin/.${old##*/}.link" "$old"
done
# `op` answers 1Password's command from Passwords (Preferences ▸
# Security turns it off, which removes the link). Never over another op;
# ours from before the rename is made new.
[ "$(readlink "$PREFIX/bin/op" 2>/dev/null)" = tabula-op ] && as_user ln -sfn olo-op "$PREFIX/bin/op"
[ -e "$PREFIX/bin/op" ] || [ -L "$PREFIX/bin/op" ] || as_user ln -s olo-op "$PREFIX/bin/op"
as_user cp -r data/icons data/brand "$PREFIX/share/olo/"
# The themes (Preferences ▸ Themes): each one's icons and wordmark, not
# its sources.
for theme in data/themes/*/; do
    theme=${theme%/}
    as_user mkdir -p "$PREFIX/share/olo/themes/${theme##*/}"
    as_user cp -r --reflink=auto "$theme/icons" "$theme/brand" "$PREFIX/share/olo/themes/${theme##*/}/"
done
# The wallpaper pack (a reflink copy where the file system can).
as_user cp -r --reflink=auto data/wallpapers "$PREFIX/share/olo/"
echo "$TREE/target/release" | as_user tee "$PREFIX/share/olo/build-dir" > /dev/null

# The login screen runs the session from where it was installed (no
# TryExec: the greeter can't look inside a private home).
BIN=$(cd "$PREFIX/bin" && pwd -P)
ENTRY=$(mktemp)
sed -e "s|^Exec=.*|Exec=$BIN/olo-session|" -e '/^TryExec=/d' data/session/olo.desktop > "$ENTRY"
as_root install -D -m 644 "$ENTRY" "$SESSIONS/olo.desktop"
rm -f "$ENTRY"
# The entry from before the rename (the new one replaces it).
as_root rm -f "$SESSIONS/tabula.desktop"
# Brightness of USB-controlled monitors (Studio Display and the like).
as_root install -D -m 644 data/udev/70-olo-displays.rules /etc/udev/rules.d/70-olo-displays.rules
as_root rm -f /etc/udev/rules.d/70-tabula-displays.rules
as_root udevadm control --reload 2>/dev/null || true
as_root udevadm trigger --subsystem-match=hidraw --action=change 2>/dev/null || true
echo "Installed to $PREFIX; Olo Desktop is on the login screen's session menu."

#!/bin/sh
# Keep a build of Olo as a login option of its own, to fall back to
# (before a change that touches everything, so that if the new build
# has a problem the old one is one choice away on the login screen).
#
#   tools/keep-build.sh [--with-symbols] NAME "Label" TREE RELEASE
#       NAME     a word for it ("before-kits"): its folder and its entry
#       Label    as the login screen shows it
#       TREE     the checkout it was built from (its data/: icons, fonts,
#                themes, wallpapers, the session's entry)
#       RELEASE  where its programs are (TREE/target/release, or another
#                CARGO_TARGET_DIR's release folder)
#   tools/keep-build.sh --remove NAME
#   tools/keep-build.sh --list
#
# It goes into ~/.local/olo-kept/NAME (bin/ and share/olo/, as
# tools/install-session.sh lays the current one out in ~/.local), each
# program stripped as a release's are (a build with its line tables is
# 13 GB, without them 1.5: it's there to be fallen back to, not
# debugged); --with-symbols keeps them, as a clone of each program
# where the file system can (btrfs: no space until the build it came
# from is deleted), else a copy. Its session
# finds its own programs beside itself and its own data first
# (OLO_DATA), and it has no build-dir: it never watches a tree and
# never offers an upgrade, so it stays as it was kept. The login
# screen's entry (olo-kept-NAME.desktop) needs root, asked for once:
# kept again with the entry as it is, nothing asks.
#
# What it shares with the current build: the person's settings and state
# (~/.config/olo, ~/.local/state/olo), so it comes up as their desktop;
# what's installed for every build (the root helpers, PAM, polkit, udev
# rules); and other programs' desktop entries, which start the current
# build's apps. A setting the kept build doesn't know is left alone by
# it; one it writes whole (a file of its own format) is as it was then.
set -eu
KEPT="$HOME/.local/olo-kept"
if [ -e /run/ostree-booted ]; then
    SESSIONS=${SESSIONS:-/usr/local/share/wayland-sessions}
else
    SESSIONS=${SESSIONS:-/usr/share/wayland-sessions}
fi
as_root() {
    # (Root only where it is needed: a sessions folder of the caller's own, a
    # test's, is written as it is.)
    if [ "$(id -u)" = 0 ] || [ -w "$SESSIONS" ]; then "$@"; else sudo "$@"; fi
}

case "${1:-}" in
--list)
    for d in "$KEPT"/*/; do
        [ -d "$d" ] || continue
        name=$(basename "$d")
        entry="$SESSIONS/olo-kept-$name.desktop"
        echo "$name: $(sed -n 's/^Name=//p' "$d/olo-kept.desktop" 2>/dev/null) ($(du -sh "$d" 2>/dev/null | cut -f1)); on the login screen: $([ -e "$entry" ] && echo yes || echo no)"
    done
    exit 0
    ;;
--remove)
    NAME=${2:?which one (tools/keep-build.sh --list)}
    case "$NAME" in */* | .* | "") echo "not a name: $NAME" >&2; exit 1 ;; esac
    [ -e "$SESSIONS/olo-kept-$NAME.desktop" ] && as_root rm -f "$SESSIONS/olo-kept-$NAME.desktop"
    rm -rf "${KEPT:?}/$NAME"
    rmdir "$KEPT" 2>/dev/null || true
    echo "Removed the kept build \"$NAME\" and its login screen entry."
    exit 0
    ;;
--with-symbols)
    SYMBOLS=1
    shift
    ;;
"" | -*)
    sed -n '2,36p' "$0" | sed 's/^# \{0,1\}//'
    exit 1
    ;;
esac

NAME=$1
LABEL=${2:?a label for the login screen}
TREE=$(cd "${3:?the tree it was built from}" && pwd -P)
RELEASE=$(cd "${4:?where its programs are}" && pwd -P)
case "$NAME" in */* | .* | *" "*) echo "a name is one word: $NAME" >&2; exit 1 ;; esac
[ -x "$RELEASE/olo-session" ] && [ -x "$RELEASE/olo-app-server" ] || { echo "no olo-session and olo-app-server in $RELEASE: build it first (cargo build --release --workspace)" >&2; exit 1; }
[ -d "$TREE/data/icons" ] || { echo "no data/icons in $TREE" >&2; exit 1; }

P="$KEPT/$NAME"
mkdir -p "$P/bin" "$P/share/olo"
n=0
for b in "$RELEASE"/olo-*; do
    [ -x "$b" ] && [ -f "$b" ] || continue
    name=$(basename "$b")
    case "$name" in *.*) continue ;; esac
    if [ -n "${SYMBOLS:-}" ] || ! strip -o "$P/bin/.$name.new" "$b" 2> /dev/null; then
        cp --reflink=auto "$b" "$P/bin/.$name.new"
    fi
    chmod 755 "$P/bin/.$name.new"
    mv -f "$P/bin/.$name.new" "$P/bin/$name"
    n=$((n + 1))
done
cp -r --reflink=auto "$TREE/data/icons" "$TREE/data/brand" "$TREE/data/fonts" "$P/share/olo/"
for theme in "$TREE"/data/themes/*/; do
    theme=${theme%/}
    mkdir -p "$P/share/olo/themes/${theme##*/}"
    cp -r --reflink=auto "$theme/icons" "$theme/brand" "$P/share/olo/themes/${theme##*/}/"
done
[ -d "$TREE/data/wallpapers" ] && cp -r --reflink=auto "$TREE/data/wallpapers" "$P/share/olo/"
# (No build-dir: nothing for its session to watch, so it's never upgraded.)
rm -f "$P/share/olo/build-dir"

COMMIT=$(git -C "$TREE" log -1 --format='%h of %ad' --date=short 2>/dev/null || echo unknown)
cat > "$P/olo-kept.desktop" <<EOF
[Desktop Entry]
Name=$LABEL
Comment=A build of Olo kept to fall back to ($COMMIT)
Exec=/usr/bin/env OLO_DATA=$P/share/olo $P/bin/olo-session
Type=Application
DesktopNames=Olo;GNOME;
EOF
cat > "$P/README" <<EOF
A build of Olo kept as a login option: "$LABEL".
From $TREE at $COMMIT.
Kept $(date +%Y-%m-%d) by tools/keep-build.sh; $n programs in bin/.

Choose it on the login screen's session menu. It never upgrades itself.
To remove it and its entry:
    tools/keep-build.sh --remove $NAME
(or: rm -rf $P; sudo rm $SESSIONS/olo-kept-$NAME.desktop)
EOF
echo "Kept $n programs in $P ($(du -sh "$P" | cut -f1))."
if cmp -s "$P/olo-kept.desktop" "$SESSIONS/olo-kept-$NAME.desktop"; then
    echo "\"$LABEL\" is on the login screen's session menu already ($SESSIONS/olo-kept-$NAME.desktop)."
elif as_root install -D -m 644 "$P/olo-kept.desktop" "$SESSIONS/olo-kept-$NAME.desktop"; then
    echo "\"$LABEL\" is on the login screen's session menu ($SESSIONS/olo-kept-$NAME.desktop)."
else
    echo "The login screen's entry wasn't installed (it needs root). To finish:" >&2
    echo "    sudo install -D -m 644 $P/olo-kept.desktop $SESSIONS/olo-kept-$NAME.desktop" >&2
    exit 1
fi

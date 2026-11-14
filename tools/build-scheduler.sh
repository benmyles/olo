#!/bin/sh
# Build the scheduler Preferences > Performance offers: scx_lavd
# (sched-ext/scx, GPL-2.0), the Latency-criticality Aware Virtual
# Deadline scheduler, loaded over the kernel's own through sched_ext.
#
#   tools/build-scheduler.sh
#
# It isn't Olo's code and isn't in Olo's workspace: one version, named
# here, is built from its published source (crates.io, with the lock
# file it was published with: `cargo install --locked`), and its program
# put beside Olo's in target/release as olo-scx-lavd, so whatever
# carries Olo's programs carries it (tools/install-session.sh, an
# upgrade from the Deskbar). Nothing runs it from there: when LAVD is
# chosen, Olo Doctor's helper copies it where root owns it
# (/usr/local/libexec/olo/scx_lavd) and a service of the system's runs
# that (apps/doctor/src/scheduler.rs). Without it Olo builds and runs as
# ever, and the panel says LAVD isn't with this copy of Olo.
#
# It needs what Olo's own build does and libelf's headers (its BPF part
# is compiled by clang and loaded with the libbpf it carries):
# tools/setup.sh has them. Two minutes the first time (and 4 GB while
# it builds, removed after); nothing when this version is built already.
#
# A release doesn't carry it (tools/release.sh leaves it out): passing a
# GPL program on means passing its source on with it, which is for
# whoever publishes the release to arrange.
set -eu
cd "$(dirname "$0")/.."
VERSION=1.1.4
TARGET=${CARGO_TARGET_DIR:-target}
ROOT="$TARGET/scheduler"
BUILT="$ROOT/bin/scx_lavd"
OUT="$TARGET/release/olo-scx-lavd"
[ -e /sys/kernel/sched_ext ] || echo "build-scheduler: this kernel has no sched_ext (Linux 6.12 and later have): it's built all the same, for one that has." >&2
if [ -x "$BUILT" ] && [ "$(cat "$ROOT/version" 2> /dev/null)" = "$VERSION" ]; then
    echo "scx_lavd $VERSION is built already."
else
    command -v clang > /dev/null 2>&1 || { echo "build-scheduler: clang is needed (tools/setup.sh installs it)" >&2; exit 1; }
    pkg-config --exists libelf || { echo "build-scheduler: libelf's headers are needed (elfutils-libelf-devel; tools/setup.sh installs them)" >&2; exit 1; }
    mkdir -p "$ROOT"
    cargo install scx_lavd --version "=$VERSION" --locked --no-track --force --root "$ROOT" --target-dir "$ROOT/build"
    echo "$VERSION" > "$ROOT/version"
    # (What building it left: 4 GB, for a program built once.)
    rm -rf "$ROOT/build"
fi
mkdir -p "$TARGET/release"
# (Only when it differs: a newer file there is a new build to the
# session watching.)
if ! cmp -s "$BUILT" "$OUT"; then
    cp --reflink=auto "$BUILT" "$OUT.new"
    chmod 755 "$OUT.new"
    mv -f "$OUT.new" "$OUT"
fi
echo "scx_lavd $VERSION is in $OUT."

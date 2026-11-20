#!/usr/bin/env bash
# Compile a full worktree. Never installs or loads modules.
set -euo pipefail
base=$(cd -- "$(dirname -- "$0")" && pwd -P)
tree=$(realpath -e "${1:?pass the path of a clean upstream or patched build worktree}")
case "$tree/" in "$base/"?*/) ;; *) echo "place the worktree under $base" >&2; exit 2;; esac
label=${tree##*/}
krel=${KERNEL_UNAME:-7.2.15-300.fc45.x86_64}
headers=${SYSSRC:-/usr/src/kernels/$krel}
jobs=${JOBS:-16}
test "$(cat "$headers/include/config/kernel.release")" = "$krel"
test "$(git -C "$tree" rev-parse HEAD)" = 61dcc93722ecb418bb5f2e00923f05b4b8051dd1
test -z "$(find "$tree/kernel-open" -maxdepth 1 -name '*.ko' -print -quit)" || { echo "use a fresh worktree for timings" >&2; exit 1; }
export PATH=/usr/sbin:/usr/bin:/sbin:/bin LC_ALL=C
export TMPDIR="$base/compiler-tmp-$label" CARGO_TARGET_DIR="$base/cargo-target"
mkdir -p "$TMPDIR"
# The public top-level plural target builds both cores and invokes the
# interface layer's modules target. The packaged module target is its alias.
cd "$tree"
/usr/bin/time -f 'exit=%x wall_s=%e user_s=%U system_s=%S max_child_rss_kb=%M' -o "$base/$label.time" \
    nice -n 10 make -O -j"$jobs" V=1 VERBOSE=1 KERNEL_UNAME="$krel" SYSSRC="$headers" \
    IGNORE_CC_MISMATCH=1 IGNORE_XEN_PRESENCE=1 IGNORE_PREEMPT_RT_PRESENCE=1 modules \
    >"$base/$label.build.log" 2>&1
set_magic=
for name in nvidia nvidia-modeset nvidia-drm nvidia-uvm nvidia-peermem; do
    f="kernel-open/$name.ko"
    version=$(modinfo -F version "./$f")
    magic=$(modinfo -F vermagic "./$f")
    [[ "$version" = 615.71.09 && "$magic" = "$krel "* ]]
    if [[ -z "${set_magic:-}" ]]; then set_magic=$magic; fi
    [[ "$magic" = "$set_magic" ]]
    stock="$base/stock-rpm/lib/modules/$krel/extra/nvidia/$name.ko.xz"
    if [[ -f "$stock" ]]; then
        [[ "$magic" = "$(modinfo -F vermagic "$stock")" ]]
        [[ "$version" = "$(modinfo -F version "$stock")" ]]
    fi
    printf '%s\t%s\t%s\t%s\t%s\n' "$name" "$(stat -c %s "$f")" "$version" "$magic" "$(modinfo -F srcversion "./$f")"
done >"$base/$label.modules.tsv"
cat "$base/$label.time" "$base/$label.modules.tsv"

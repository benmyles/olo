#!/bin/bash
# Resolve duplicates using real kmod indexes in an isolated staging root.
set -euo pipefail
base=$(cd -- "$(dirname "$0")/.." && pwd -P)
k=7.2.15-300.fc45.x86_64
root="$base/tests/module-root"
mods="$root/lib/modules/$k"
if [[ ! -d "$base/stock-rpm/lib/modules/$k/extra/nvidia" ]]; then
    rpmfile="/var/cache/akmods/nvidia/kmod-nvidia-$k-615.71.09-3.fc45.x86_64.rpm"
    mkdir -p "$base/stock-rpm"
    (cd "$base/stock-rpm"; rpm2cpio "$rpmfile" | cpio -id --quiet '*.ko.xz')
fi
mkdir -p "$mods/extra/nvidia" "$mods/updates/olo"
cp --reflink=auto "$base/stock-rpm/lib/modules/$k/extra/nvidia/"*.ko.xz "$mods/extra/nvidia/"
cp --reflink=auto "$base/${1:-verified}/kernel-open/"*.ko "$mods/updates/olo/"
for file in modules.order modules.builtin modules.builtin.modinfo; do cp "/lib/modules/$k/$file" "$mods/$file"; done
for variant in fork stock; do
    /usr/sbin/depmod -b "$root" "$k"
    for n in nvidia nvidia_modeset nvidia_uvm nvidia_drm nvidia_peermem; do
        p=$(/usr/sbin/modinfo -b "$root" -k "$k" -F filename "$n")
        case "$variant:$p" in
            fork:*"/updates/olo/"*|stock:*"/extra/nvidia/"*) ;;
            *) echo "unexpected $p" >&2; exit 1 ;;
        esac
        printf '%s %s\n' "$variant" "${p#"$root"}"
    done
    if [[ $variant = fork ]]; then mv "$mods/updates/olo" "$root/disabled-olo"; fi
done
mv "$root/disabled-olo" "$mods/updates/olo"

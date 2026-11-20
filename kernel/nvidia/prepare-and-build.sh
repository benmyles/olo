#!/usr/bin/env bash
# Scratch-only. A matching kernel-devel tree and the normal Fedora compiler
# toolchain must already be installed. No install/load, PCI detection, or sudo.
set -euo pipefail
base=$(cd -- "$(dirname -- "$0")" && pwd -P)
cd "$base"
export PATH=/usr/sbin:/usr/bin:/sbin:/bin LC_ALL=C
export TMPDIR="$base/prepare-tmp" CARGO_TARGET_DIR="$base/cargo-target"
mkdir -p "$TMPDIR"
variant=${1:?use pristine, prevention, patched or fedora}
case "$variant" in pristine|prevention|patched|fedora) ;; *) exit 2;; esac
label=${2:-$variant}
[[ "$label" =~ ^[A-Za-z0-9_-]+$ && ! -e "$base/$label" ]]
repo="$base/upstream.git"
tag=615.71.09
commit=61dcc93722ecb418bb5f2e00923f05b4b8051dd1
if [[ ! -d "$repo" ]]; then
    git -c credential.helper= -c core.hooksPath=/dev/null clone --bare --depth=1 --branch "$tag" \
        https://github.com/NVIDIA/open-gpu-kernel-modules.git "$repo"
fi
[[ "$(git --git-dir="$repo" rev-parse refs/tags/$tag)" = d2a24f90e843359450ab313a286f43a88d95c6ce ]]
[[ "$(git --git-dir="$repo" rev-parse "refs/tags/$tag^{commit}")" = "$commit" ]]
[[ "$(git --git-dir="$repo" rev-parse "$commit^{tree}")" = 9199a9f620b76343c35c207354ffcb3d1570053d ]]
archive_digest=$(git --git-dir="$repo" archive --format=tar "$commit" | sha256sum)
[[ "${archive_digest%% *}" = 29c36a0a46b91fd17f96f977e046f303317cd6eedb5cbbefdcf0c95cfe7a0f38 ]]
git --git-dir="$repo" -c core.hooksPath=/dev/null worktree add --detach "$base/$label" "$commit"
if [[ "$variant" != pristine ]]; then
    sha256sum -c series.sha256
    while read -r digest patch; do
        if [[ "$variant" = prevention ]]; then
            case "${patch##*/}" in 000[123]-*) ;; *) continue;; esac
        fi
        git -C "$base/$label" apply --index --check --whitespace=error-all "$base/$patch"
        git -C "$base/$label" apply --index --whitespace=error-all "$base/$patch"
        echo "applied $patch"
    done < series.sha256
fi
if [[ "$variant" = fedora || "$variant" = prevention ]]; then
    # Only the open-interface hunk from RPM Fusion's 615.71.09-3 prep is relevant.
    # Two other hunks in the same file target the proprietary driver.
    printf '%s  %s\n' a4c6bca2823b715f2e46196e1b4a5919da757c16a5282b49a1ff5ea08eb7c11d packaging/set_driver_defaults.patch | sha256sum -c -
    git -C "$base/$label" apply --index --check --include='kernel-open/*' "$base/packaging/set_driver_defaults.patch"
    git -C "$base/$label" apply --index --include='kernel-open/*' "$base/packaging/set_driver_defaults.patch"
fi
git -C "$base/$label" diff --cached --check
"$base/build-modules.sh" "$base/$label"

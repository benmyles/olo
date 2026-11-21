#!/bin/bash
# Rebuild vendor/ghostty-vt/lib/<arch>-linux-gnu/libghostty-vt.a and headers
# from the pinned upstream Ghostty commit using the pinned Zig toolchain.
set -euo pipefail
cd "$(dirname "$0")/.."

ZIG_VERSION="0.17.1"
GHOSTTY_COMMIT="6d244209978e599277904ff3ced7b8a2ebcc528d"

ARCH="$(uname -m)"
case "$ARCH" in
    x86_64)
        ZIG_ARCH="x86_64-linux"
        ZIG_SHA256="3581863e3cbcfd0fce0efc682ce7e9e93da96668a977791d356a26d2a7e9b17a"
        TARGET_DIR="x86_64-linux-gnu"
        ;;
    aarch64)
        ZIG_ARCH="aarch64-linux"
        ZIG_SHA256=""
        TARGET_DIR="aarch64-linux-gnu"
        ;;
    *)
        echo "Unsupported architecture: $ARCH" >&2
        exit 1
        ;;
esac

WORK="$(mktemp -d /var/tmp/olo-build-ghostty-vt.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

echo "Downloading Zig ${ZIG_VERSION} (${ZIG_ARCH})..."
ZIG_TAR="$WORK/zig.tar.xz"
curl -fsSL "https://ziglang.org/download/${ZIG_VERSION}/zig-${ZIG_ARCH}-${ZIG_VERSION}.tar.xz" -o "$ZIG_TAR"
if [ -n "$ZIG_SHA256" ]; then
    echo "${ZIG_SHA256}  ${ZIG_TAR}" | sha256sum -c -
fi
mkdir -p "$WORK/zig"
tar -xf "$ZIG_TAR" -C "$WORK/zig" --strip-components=1
ZIG="$WORK/zig/zig"

echo "Fetching Ghostty ${GHOSTTY_COMMIT}..."
mkdir -p "$WORK/ghostty"
git -C "$WORK/ghostty" init -q
git -C "$WORK/ghostty" remote add origin https://github.com/ghostty-org/ghostty.git
git -C "$WORK/ghostty" fetch -q --depth 1 origin "$GHOSTTY_COMMIT"
git -C "$WORK/ghostty" checkout -q FETCH_HEAD

echo "Building libghostty-vt.a (ReleaseFast, SIMD, stripped)..."
(
    cd "$WORK/ghostty"
    "$ZIG" build \
        -Demit-lib-vt=true \
        -Doptimize=ReleaseFast \
        -Dstrip=true \
        --prefix "$WORK/out"
)

mkdir -p "vendor/ghostty-vt/include" "vendor/ghostty-vt/lib/${TARGET_DIR}"
rm -rf vendor/ghostty-vt/include/ghostty
cp -r "$WORK/out/include/ghostty" vendor/ghostty-vt/include/
cp "$WORK/out/lib/libghostty-vt.a" "vendor/ghostty-vt/lib/${TARGET_DIR}/libghostty-vt.a"
cp "$WORK/ghostty/LICENSE" vendor/ghostty-vt/LICENSE

echo "Installed vendor/ghostty-vt/lib/${TARGET_DIR}/libghostty-vt.a ($(du -h "vendor/ghostty-vt/lib/${TARGET_DIR}/libghostty-vt.a" | cut -f1))"

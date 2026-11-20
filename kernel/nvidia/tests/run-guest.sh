#!/usr/bin/env bash
# Host actions compile fixtures and start only a no-passthrough, headless KVM guest.
set -euo pipefail
base=$(cd -- "$(dirname -- "$0")/.." && pwd -P)
cd "$base"
k=7.2.15-300.fc45.x86_64
tree=${1:-verified}
export PATH=/usr/sbin:/usr/bin:/sbin:/bin LC_ALL=C TMPDIR="$base/prepare-tmp"
mkdir -p "$TMPDIR"
/usr/bin/python3 tests/ref-fixture/generate.py "$tree"
for f in tests/pm-fixture tests/ref-fixture; do
    make -s -C "/usr/src/kernels/$k" M="$base/$f" modules
done
exec /usr/bin/python3 tests/kvm-guest --quiet --bin uname --mem 640 --timeout 50 \
    --ko "$tree/kernel-open/nvidia.ko" --ko tests/pm-fixture/olo_pm_fixture.ko \
    --ko tests/ref-fixture/olo_ref_fixture.ko tests/guest-check.sh

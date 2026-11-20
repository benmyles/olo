#!/bin/bash
set -eu
test "$$" -gt 1 && test -c /dev/ttyS1 && test -f /modules/order && test -d /ko
uname -r
if grep -qs 0x10de /sys/bus/pci/devices/*/vendor; then exit 100; fi
if test -f /ko/olo_ref_fixture.ko; then
    insmod /ko/olo_ref_fixture.ko
    rmmod olo_ref_fixture
    dmesg | grep 'olo-ref-test:'
fi
insmod /ko/olo_pm_fixture.ko
rmmod olo_pm_fixture
dmesg | grep 'olo-pm-test:'
if insmod /ko/nvidia.ko test_defer_gcoff_minor=248 2>/invalid; then exit 101; fi
cat /invalid
grep -Eiq 'range|invalid' /invalid
if insmod /ko/nvidia.ko test_defer_gcoff_minor=0 2>/nodev; then exit 102; fi
cat /nodev
grep -qi 'No such device' /nodev
dmesg | grep -E 'No NVIDIA GPU found|invalid for parameter' | tail -3
echo 'Guest parameter parsing and expected no-GPU initialization exit passed'

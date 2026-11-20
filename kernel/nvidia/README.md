# NVIDIA's open kernel modules, patched: a review draft

**Nothing here is built by Olo yet, and none of it has run on a card.**
It is the design and the patch series for Olo's own build of NVIDIA's
open kernel modules (MIT/GPL: `github.com/NVIDIA/open-gpu-kernel-modules`),
made with Astra 6.2 at its highest effort on 2026-11-20, the day a
runtime suspend of an RTX PRO 2000 Blackwell failed for good and an
unload then stuck in the kernel (docs/HACKING.md, "A graphics card that
stopped answering"; `report-615.71.09-runtime-suspend.md` here is the
write-up for NVIDIA). Ben: "can we create our own olo version of the
nvidia driver that fixes this bug?"; "let's do the patch as well …
make it all rock solid bulletproof"; "make sure you have astra 6.2 max
review all this stuff too".

## What was shown, and how

All against the tag 615.71.09 (commit 61dcc937…, pinned by hash in
`prepare-and-build.sh`), with the running kernel's headers
(7.2.15-300.fc45), in scratch; nothing was loaded on the computer, and
its card was not touched.

- **It builds.** NVIDIA's `modules` target builds all five modules
  from source, both cores included (Fedora's akmod package carries the
  cores prelinked, so they can't be patched there): one to three
  minutes on this computer. Version and vermagic as the stock ones.
  Loaded in a KVM guest (no card there): "No NVIDIA GPU found", as it
  should.
- **The fault, reproduced in the driver's own code.** NVIDIA's timeout
  setup, check and dispatch, the RISC-V halt poll and its caller,
  compiled into a harness with the register and the clock made up
  (`tests/halt-*.c`, `halt-results.txt`): with the thread's deadline
  already spent and HALTED coming true 10 ms later, the stock code
  fails at the first read (as on the card that day, 3.049 s past its
  deadline); patched, it succeeds after 10 ms; a processor that never
  halts still times out, after 4 s of its own.
- **The stages before engine unload abort cleanly** (`tests/pm-stages.c`,
  `stage-results.txt`): a failed framebuffer reservation, failed save
  metadata, a spent budget, and the test parameter each end with
  nothing unloaded, interrupts restored, and `-EAGAIN` to Linux, which
  (measured in a guest with a made-up device) leaves no
  `runtime_error` and tries again later.
- **The reference that makes removal deadlock**, and the display
  ring's wait with no end, in fixtures (`tests/channel-test.c`,
  `guest-final.txt`).

## The series (`patches/`, in order; `series.sha256`)

| | | where it stands |
| --- | --- | --- |
| 0001 | the halt wait after a runtime suspend gets a deadline of its own | **the fix for what failed.** Ten lines. First to try on a card. |
| 0002 | the PCI remove callback's stack reserved at probe | first trial with it |
| 0003 | a suspend whose budget is spent, or whose allocation fails, before any engine is unloaded is given up cleanly and tried again | **what "no suspend while memory is short" is, done where it can be: in the driver.** First trial with it. |
| 0004 | errors of the transition kept and reported; no new opens or calls after a failed one | needs NVIDIA's eye on what is already mapped |
| 0005 | the suspend command's queue and bus waits bounded | needs testing under load |
| 0006, 0007 | the display module owns its reference to the card; a card being removed can still be let go of | shown in a guest; needs hot-unplug and concurrency testing |
| 0008–0010 | the display ring, cursor and second push buffer stop instead of waiting for ever | **a spare card first**: it can turn a display off after a long stall |
| 0011 | fault injection: one suspend refused on purpose, off by default | for the qualification build |

## What is not solved

**Bringing a card back without restarting, after its firmware failed in
an unknown state.** After a suspend command whose outcome isn't known,
the firmware may still own memory it was given; the driver's own
teardown frees that memory, wakes the card, and asks the firmware to
unload again. Nothing in the series proves the old firmware can no
longer write before its memory is reused, and Linux's PCI remove
can't refuse. `design/recovery-design.md` has what would be needed;
it is NVIDIA's to review. So Olo Doctor's in-place restart stays gone,
whatever driver is loaded.

**A whole teardown with a time limit.** `design/wait-audit.md` goes
through the waits (1,313 loop and wait sites indexed); two remain
without a bound.

## The order it would go in

Astra's, and Olo's: (1) what needs no driver change (done: Olo asks a
card only when it says it is awake; Doctor offers to turn the unused
console off, `nvidia_drm fbdev=0`); (2) patches 0001–0003, the
"prevention" build (`prepare-and-build.sh prevention`), tried on the
card with a stock driver to start back into, `design/qualification.md`
step by step; (3) the reference patches; (4) the display patches and
the recovery, with NVIDIA.

On this computer there is one card, the person's. Nothing of this is
installed or loaded there without their word, at a quiet moment, with
the stock driver one restart away.

## What Olo still has to build around it

`design/packaging-design.md`: the source fetched and checked by hash
before a system update (the tag's signature couldn't be checked
against a known key); built when a kernel or driver update is staged,
not at the first start after it (too late for the boot that loads the
driver); all five modules as one set under `updates/olo`, the stock
ones kept in `extra/nvidia` (tried with depmod: Olo's chosen with the
override there, stock again without); signed where Secure Boot is on;
the initramfs looked at; withdrawn whole if a patch no longer applies,
the build fails, or the driver's version moved; and Doctor showing
which is loaded, by build id.

The scripts here are as they were run in scratch, to be made Olo's own
(a service of the kind of `apps/bbr3`'s) when that is built.

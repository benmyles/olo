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

## The builder: `olo-nvidia` (apps/nvidia), a prototype that is held

**It changes nothing on a real computer.** `build`, `withdraw`,
`check` and `watch` refuse a real root and refuse to run as root
(`main.rs`); only `status` reads the real system. Its two unit files
are not installed by anything. It was written by a Codex agent to
`design/packaging-design.md` on 2026-11-20, then reviewed by Astra at
its highest effort, which said to hold it; what follows is what holds
and what doesn't.

**What it does, and what was shown of it** (16 tests under a made-up
root, `OLO_NVIDIA_ROOT`; `cargo test -p olo-nvidia`):

- NVIDIA's source for the one supported version, fetched once and
  checked by hash every time it is used (the archive's bytes, then the
  commit it names); the three prevention patches and Fedora's one
  change applied at their exact lines, every byte of context checked
  (`patches.rs`: a hunk that would apply "somewhere near" is refused).
- The compile handed to a user with no privilege (`build.rs`: a mount
  and a user namespace of its own, no groups, `no_new_privs`, an empty
  environment, every descriptor closed): all five modules in 70 s here
  (`real_build.rs`, an ignored test: it needs a checkout of NVIDIA's
  source, `OLO_NVIDIA_BASELINE`).
- What was built read back as modules (`modules.rs`: name, version,
  vermagic, build ID, hash) and held to the stock set's version and
  vermagic; the copy that is checked is the copy that is installed.
- What it refuses on: Secure Boot, a kernel that enforces module
  signatures or is locked down, NVIDIA's modules in the start-up image
  (`boot.rs` reads dracut's rules and the images themselves, every
  archive in them), another driver version, missing kernel headers.
- `status --json` for Olo Doctor: for each kernel, the stock version,
  Olo's, which set the index chooses, and what is loaded now, module
  by module, by build ID ("stock", "olo", "mixed", "going",
  "unknown").

**What the review found** (2026-11-20; three it called critical):

1. **A whole index can be published with modules missing.** depmod
   skips a folder it can't read and still ends well; the program
   looked only at its five names and put the index in place: in the
   review's check a storage driver was gone from `modules.dep`. (Its
   guard, applied: the old and the new index compared whole, text and
   binary, and any word from depmod an error.)
2. **The version rule held only when `check` ran.** Between the
   distribution updating its driver and the next check, the next load
   could be Olo's older set beside newer libraries; akmods loads the
   modules itself at start, before either of Olo's units.
3. **Another depmod undoes it.** Anything that runs depmod meanwhile
   (a kernel's installation, akmods) can publish an index made from
   an older look at the tree, pointing at a set that is gone: a module
   that then can't be found, with `check` saying all is well.

And: a withdrawal that reported success with an inconsistent stock
set; a start-up image's module missed where its name was padded; a
boot entry with no version not followed; the labels of the index
files it wrote; units that could hold up the start.

**So the way a set becomes the one loaded is to be replaced**, not
mended: no set under `updates/`, no index written by Olo. Two designs
are with Astra (2026-11-20): Olo's modules chosen at the moment of
loading (a `modprobe.d` line that hands each of the five names to
`olo-nvidia load`, which loads Olo's module only if everything still
agrees, and the stock one otherwise); or Olo's five exchanged with the
stock five in place, in one step at shutdown, so that any update of
the distribution's simply writes over them. Until one is chosen,
built, reviewed again and tried in a guest, the hold stays.

Running `make` and (for now) `depmod` is the kernel's own toolchain,
not a wrapped app: a module is built by the kernel's build system or
not at all (as `olo_kmod`'s are), and the index's format is kmod's.
The index part goes with the redesign.

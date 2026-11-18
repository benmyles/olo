# Kernel code

What Olo carries for the Linux kernel itself. Each is a module built for
the running kernel by a service of Olo's, which Olo Doctor installs;
none is needed for Olo to run.

Kernel modules are **GPL-2.0**, as the kernel they link into requires
(each file says so in its first line; BBRv3's is Google's, under the
kernel's "GPL-2.0 OR BSD-3-Clause"), apart from the licence of the rest
of this repository.

What the services share (the module built for the running kernel with
the kernel's own build system, installed to
`/lib/modules/KERNEL/extra/olo`, built again for another source or
another kernel, removed with its kernel) is `olo-kmod` (crates/kmod). A
module is tried in a KVM guest that runs this computer's kernel, never
in the person's own (`tools/dev/kvm-guest`).

Beside the modules, [TUNING.md](TUNING.md): what the kernels recommended
for a desktop carry, where Fedora's stands on each, the two settings of
the kernel Olo suggests because they measured as a gain with nothing
given up (full preemption, a limit on what waits for a removable disk),
and what was measured and left alone.
`tools/dev/kernel-check` makes the measurements again.

**A kernel update** (Ben, 2026-11-16: "make sure all these kernel
tunings/mods will continue to work properly after doing os updates that
update the kernel"). Each module's service builds it for the new kernel
at that kernel's first start (a few seconds; the kernel's headers come
with every kernel through `kernel-devel-matched`, which Olo Doctor's
fix installs), keeps the old kernel's build while that kernel is
installed, and removes it after. A module that can't be built or loaded
there says why (Olo Doctor shows it) and leaves the kernel as it comes:
the live patch doesn't build if the function it replaces is gone or of
another shape; BBRv3 doesn't if what it probes has changed shape.
`tools/dev/kernel-next` checks all of it, and the settings, on the
newest kernel Fedora has before it's installed, in a guest: run it when
a new kernel series is on its way (2026-11-16: 7.3.3-300.fc45, all of
it passed). [TUNING.md](TUNING.md), "After a kernel update", has the
settings' side.

- `gpu-memory/`: `olo_gpu_memory`, a live patch that keeps graphics
  memory out of swap (TTM's `ttm_backup_bytes_avail` answers "none").
  Built and loaded at each start by `olo-gpu-memory` (apps/gpu-memory).
  docs/HACKING.md, "Windows' buffers stay in memory".
- `bbr3/`: `tcp_bbr3`, Google's BBRv3 congestion control for TCP as a
  module for the kernel as it comes (the congestion control `bbr3`).
  Built, loaded and made the system's at each start by `olo-bbr3`
  (apps/bbr3). [bbr3/README.md](bbr3/README.md): what it is made of, how
  it was shown to be BBRv3, what it measures. docs/HACKING.md, "BBRv3
  for TCP".
- `ec-storm/`: `olo_ec_storm`, a guard for the embedded controller's
  events: a handler of its own in front of each of the firmware's (the
  ACPI EC driver's own way for a module to, `acpi_ec_add_query_handler`),
  so that of one event arriving over and over one runs the firmware's
  method and the rest are counted for it to run again for, eight at
  most, and a flood of one event can't hold up the others. No structure
  of the driver's is read. Built and loaded at each start by
  `olo-ec-storm` (apps/ec-storm), only on a kernel series listed in
  [ec-storm/CHECKED](ec-storm/CHECKED): the driver's two functions it
  calls are declared in the driver's own header, so nothing at build
  time says if they changed; `tools/dev/ec-storm-check --series vX.Y`
  compares that series' driver with the one it was written against
  (`driver.checked`), and checks its counting (`check.c`), its build
  and its refusal where there is no controller (a guest). What it does
  with a real controller's events can be seen only on a computer that
  has one: not tried yet (2026-11-18). docs/HACKING.md, "The embedded
  controller, watched".

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

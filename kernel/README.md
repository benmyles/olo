# Kernel code

What Olo carries for the Linux kernel itself. Each is a module built for
the running kernel by a service of Olo's, which Olo Doctor installs;
none is needed for Olo to run.

Kernel modules are **GPL-2.0**, as the kernel they link into requires
(each file says so in its first line), apart from the licence of the
rest of this repository.

- `gpu-memory/`: `olo_gpu_memory`, a live patch that keeps graphics
  memory out of swap (TTM's `ttm_backup_bytes_avail` answers "none").
  Built and loaded at each start by `olo-gpu-memory` (apps/gpu-memory).
  docs/HACKING.md, "Windows' buffers stay in memory".

# Blackwell mobile RTD3 suspend timeout leaves host RM in standby and NVKMS module exit spinning

NVIDIA open modules and GSP 615.71.09, Fedora 45, x86-64 kernel 7.2.15-300.fc45. GPU 10de:2d39, subsystem 17aa:235c, VBIOS 98.06.4f.00.79. The Intel xe GPU drives the displays. The NVIDIA PCI function has no active display and has a sibling HDA function. NVIDIA DRM modeset and fbdev are enabled. Fine-grained runtime D3 was enabled, with DynamicPowerManagement=3, DynamicPowerManagementVideoMemoryThreshold=200, PreserveVideoMemoryAllocations=2 and UseKernelSuspendNotifiers=1.

**Observed once on hardware.** The GPU had repeatedly entered and left runtime suspend over several days. Under host memory pressure a system-memory allocation failed at mem_desc.c:1348, system_mem.c:416 and nv_gpu_ops.c:5071. About 95 seconds later there was another bare mem_desc.c:1348 allocation failure. This preceded the following log by 453 ms. Times below are relative to the host RM thread deadline check.

```text
0.000000  _threadNodeCheckTimeout: currentTime: 1011593fc4e3c0 >= 1011588a08d780
0.000094  _threadNodeCheckTimeout: Timeout was set to: 4000 msecs!
0.000119  kflcnWaitForHaltRiscv_GA102: Timeout waiting for RISC-V to halt
0.000142  gpuPowerManagementEnter: GSP unload failed at suspend (bootMode 0x1, newLevel 0x3): 0x65
0.011998  can't suspend (nv_pmops_runtime_suspend [nvidia] returned -5)
0.012433  _issueRpcAndWait: rpcSendMessage failed with status 0x00000011 for fn 76
```

PCI and ACPI reported D0, the endpoint answered configuration reads, and runtime_status became error. There were 1,158 fn 76 send failures in the capture. Later an administrator helper requested ordinary non-forced unload of nvidia_drm with O_NONBLOCK. DRM unregistered the device. The caller stayed runnable in kernel mode, consuming one CPU, with the identical message every five seconds for more than 30 minutes.

```text
nvidia-modeset: ERROR: GPU:0: Error while waiting for GPU progress: 0x0000ca7d:0 2:2:0:4040
nvEvoMakeRoom <- InitTaps5ScalerCoefficientsC9 <- EvoInitChannelC9
  <- nvRmSetGc6Allowed <- nvSetDispModeEvo <- nvShutDownApiHeads
  <- nvEvoRestoreConsole <- RestoreConsole <- ReleaseOwnership
  <- nvKmsIoctl <- nvkms_ioctl_from_kapi <- ReleaseOwnership
  <- nv_drm_dev_unload <- nv_drm_dev_destroy <- nv_drm_remove_devices
  <- __do_sys_delete_module
```

**Host-source findings against 615.71.09**

1. In `src/nvidia/src/kernel/gpu/gsp/kernel_gsp.c:5890` GSP suspend is fire-and-forget. The code checks the processor-suspended mailbox, marks GSP unloaded and disables heartbeat, then executes the GH100+ final RISC-V halt check. `kernel_gsp_gh100.c:1119` uses GPU_TIMEOUT_DEFAULT with flags zero. `gpu_timeout.c:492` uses the RM thread's shared deadline. The two printed timestamps differ by exactly 3,049,000,000 ns. The duration between sending suspend and sampling HALTED was not recorded. The trace does not establish a permanent firmware hang. One candidate for the bare OOM is the 2 MiB attempt in `kernel_gsp.c:6694`, which retries with 4 KiB pages. It would be valuable to log the allocation owner, size and latency, the suspend send and its return, the mailbox result, and the elapsed time and status of the final halt check.
2. `gpu_suspend.c:579–595` sets IN_STANDBY and bInD3Cold after gpuPowerManagementEnter even when it returns an error. Stage STATE_LOAD has already been reached. The non-GC6 failure path calls memmgrFreeFbsrMemory. In `dynamic-power.c:2502–2523` GCOFF_STATE_ENTERED is only set on success and reserved backing is released on failure. Normal runtime resume dispatches on GCOFF_STATE_ENTERED and can take the GC6 no-op exit instead. `kernel_gsp.c:398–401,488–505` rejects the observed RPCs on the host before the queue send. Please provide a stage-aware rollback or terminal unavailable state with a bounded cold reinitialization protocol for this combination.
3. `src/nvidia-modeset/src/nvkms-dma.c:113–162` logs and restarts the timer without an error exit. Logged count=2, free=2, GET=0, software PUT=4040. The ring maximum is 4048 and the return requires free > count. An all-ones GET is clamped to 4048 and the 20-byte chase pad yields free=0 in this state. Console release also programs scaler coefficients when the no-console fallback shuts down heads. A correct error path must stop method emission, handle notifier/cleanup waits, and release device references safely during teardown. A plain return from the void helper would let callers write unreserved commands.
4. A concurrent PCI removal has an additional reference-lifetime risk. After DRM exit has cleared KAPI callbacks, `nv_pci_remove_helper` detaches the device from the lookup list before waiting indefinitely for its internal usage count. The pending KAPI close later needs `find_gpu_id` on that same list to drop its reference (`nv-pci.c:2813,2833`, `nv.c:1245,5507`, `nvkms-kapi.c:625`). This hazard was identified in the source. PCI removal was never attempted on the affected system.
5. The captured proc power report still returned “Video Memory: Active.” This string is selected from GCOFF_STATE_ENTERED=false under the RM API and per-device locks (`dynamic-power.c:3224–3259`). The capture is consistent with the NVKMS and NVKMS PM locks being held by the spinner and RM locks remaining available. The GC6 blocker control used before display-channel initialization is host-side. On the zero-to-one reference path `nv_indicate_not_idle` uses a no-resume reference and a void PCI shutdown wrapper, then returns NV_OK even though PCI's call to pm_runtime_resume can fail (`nv.c:5739–5744`, Linux `pci-driver.c:557–566`). Please also check error propagation at this boundary.

**Expected.** A rejected runtime suspend should leave hardware and driver usable, or expose a recovery-required state with teardown that can complete safely. A release of display ownership should have a bounded failure path even when the GPU does not consume the display push buffer. An operator should be able to identify failed health without waking the device.

**Evidence limits.** The hang and the PM error were observed. The allocation caller for the final OOM and the physical GSP state after the HALTED check are unknown. An A/B on a spare card with a fresh bounded halt deadline would separate two possibilities. A late real HALTED followed by successful resume supports a spent thread budget. A busy CPUCTL throughout the local deadline with a confirmed suspend send would direct investigation to firmware/board state. No reset, unbind or live kernel modification was attempted in this investigation.

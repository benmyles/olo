# The kernel's settings Olo suggests, and what else was looked at

Ben, 2026-11-16: "research what other linux kernel patches are
recommended for high performance, responsive desktop operating systems
and if you find any that are win-win over the fedora 45 kernel and would
be a win for olo users then also integrate them into olo like bbr3 so
that olo doctor can offer them."

The answer in short: nearly everything the desktop kernels are known for
is in Fedora 45's kernel already, or can't be had without replacing the
kernel. What was left to gain is two settings: **full preemption**, and
**a limit on what waits in memory for a removable disk**. Each was
measured on this kernel (7.2.15-300.fc45) in a KVM guest, off and on,
and is offered only because the measurement showed a gain with nothing
given up for it. Four more things were measured and left as the kernel
comes, because each gives something up or gains nothing: the
multi-generational LRU's thrash protection (the one most often
recommended: it ended a program that fitted in memory in 3 runs of 9),
dm-crypt without its work queues, split-lock mitigation off, and the
room kept free for allocations.

Everything here can be measured again: `tools/dev/kernel-check` (it
needs `cargo build --release -p olo-system --example kbench`). The
figures below are its, from a guest with 4 processors on a ThinkPad P1
Gen 9 (Core Ultra 7 356H); a guest's numbers are noisier than the bare
computer's, so each comparison was run both ways in turn and the
differences quoted are many times the spread.

## What the desktop kernels carry, and where Fedora 45's stands

XanMod, Zen/Liquorix and CachyOS (and Clear Linux before them) are the
kernels recommended for a responsive desktop. What they change:

| What | Fedora 45's kernel | For Olo |
| --- | --- | --- |
| A preemptible kernel, chosen at start (`PREEMPT_DYNAMIC`) | Built in; starts **lazy** | **A setting: full** (below) |
| A 1000 Hz tick | `HZ=1000` | Nothing to do |
| The multi-generational LRU (MGLRU), on | On (`0x0007`); its thrash protection off (`min_ttl_ms` 0) | Measured, left alone (below) |
| le9 (the working set kept under pressure) | Not there; a patch to the core of memory management | Can't be had; MGLRU's `min_ttl_ms` is mainline's way to the same end |
| BBRv3 | BBRv1 as `tcp_bbr` | Olo's module, `tcp_bbr3` ([bbr3/README.md](bbr3/README.md)) |
| Another CPU scheduler (BORE, BMQ/PDS, …) | `sched_ext`: a scheduler loads over the kernel's | Preferences ▸ Performance ▸ Scheduler (`scx_lavd`) |
| A cap on what waits for slow disks (CachyOS's udev rule) | No cap: one limit for all disks | **A setting** (below) |
| dm-crypt without its work queues (Zen) | A flag of each volume | Measured, left alone (below) |
| Split-lock mitigation off (CachyOS, SteamOS) | On | Measured, left alone (below) |
| The TEO idle governor | Not built (`menu` only), and a governor can't be a module | Can't be had |
| The ADIOS I/O scheduler | Not there; it needs the block layer's private headers | Can't be had; an NVMe disk runs with none, which is quickest |
| `-O3`, `-march=x86-64-v3`, Clang LTO | `-O2`, generic, no LTO | The kernel rebuilt: can't be had |
| Clear Linux's start-up and `accept()` patches, XanMod's defaults | Not there | Patches to code built into the kernel |
| ntsync, CAKE, per-VMA locks, zram, THP on request | All there | Nothing to do |

"Can't be had" means on the kernel as it comes. Olo doesn't replace the
kernel: a person's updates, Secure Boot and their graphics driver go by
Fedora's. What can be added to it is a module (BBRv3), a live patch of
one function (graphics memory), a scheduler through `sched_ext`, and
settings.

## The two settings

### Preemption: full

A program that was just woken (by a key, by another program's answer)
while the processors are busy has to take a processor from something.
Fedora's kernel starts **lazy**: for ordinary programs it marks the
running one to give way at the kernel's next tick, which at 1000 Hz is up
to a millisecond later, and sends the other processor no interrupt.
**Full** switches at once. Olo's window server is realtime, which
preempts at once either way; this is for the programs it shows: each
step of a program's answer to a key (woken by the window server, woken
again by what it asked) waits for a tick under lazy.

`tools/dev/kernel-check preemption --reps 6`: one thread wakes another
through a pipe and goes on working, 2,700 times in 6 s, while 4 threads
compute on the guest's 4 processors. Microseconds until the woken thread
runs (the middle of six guests each way):

| The 4 computing threads | | middle | 99 in 100 | 999 in 1000 | worst |
| --- | --- | ---: | ---: | ---: | ---: |
| at nice 19 (as the supervisor lowers a build) | lazy | 52 | 950 | 1001 | 1004 |
| | full | 2 | 4 | 152 | 156 |
| at the same priority | lazy | 2 | 369 | 808 | 1000 |
| | full | 2 | 152 | 154 | 157 |

(152 µs is the waker's own 150 µs of work: at the same priority the
woken thread shares its processor and runs when it's done.)

What the work itself got done, full against lazy:

| | lazy | full | |
| --- | ---: | ---: | ---: |
| 4 threads computing (rounds a second) | 269,748 | 270,059 | 1.001× |
| the same, beside the waking pair | 266,835 | 266,666 | 0.999× |
| pipes, 16 writers and 16 readers (messages a second) | 10,132,402 | 10,527,846 | 1.039× |
| pipes, 4 writers and 4 readers | 4,513,709 | 4,671,208 | 1.035× |
| 16 threads writing one file (writes a second) | 1,356,040 | 1,353,295 | 0.998× |

Nothing lost within the spread (the pipes' ±4% is theirs: one lazy guest
of six did 14.0 million).

With a scheduler loaded through `sched_ext` the setting changes nothing:
the scheduler asks the kernel for each switch itself (`resched_curr`,
which doesn't wait). Measured with Olo's `scx_lavd` loaded in the same
guest, lazy and full by turns: the middle 153 µs and 99 in 100 within 1
to 2 ms either way. So Olo Doctor doesn't suggest it while LAVD is
chosen. (Those are the first test's figures under LAVD: ten times the
kernel's own scheduler's with full preemption. Preferences ▸
Performance ▸ Scheduler recommends the kernel's own.)

How it's set: `preempt=full` among the kernel's start arguments (every
boot entry, `/etc/kernel/cmdline`, GRUB's defaults), and the running
kernel switched too (`/sys/kernel/debug/sched/preempt`) where it isn't
locked down. The kernel tells only root which it's on, so Olo notes a
switch made while it runs in `/run/olo-kernel/preemption`.

### Removable disks: 16 MB may wait

What a program writes waits in memory until the kernel writes it out,
up to a share of all memory (`vm.dirty_ratio`), one limit for every
disk: gigabytes on a computer of today (15.5 GB on the ThinkPad as it
was measured). A USB stick takes a few megabytes a second. A copy to it
"finishes" at once; the stick is then written for minutes while Eject
waits, or is pulled out with the files not on it. A disk can be given a
limit of its own that holds even while the whole is under the kernel's
(`bdi/strict_limit` with `bdi/max_bytes`, Linux 6.2).

`tools/dev/kernel-check slow-disks`: 600 MB written to a disk that takes
10 MB a second (QEMU's throttle), in a guest with 2 GB:

| | the copy ended after | still in memory then | on the disk after another | in all |
| --- | ---: | ---: | ---: | ---: |
| as the kernel comes | 32.1 s | 280 MB | 28.4 s | 60.4 s |
| 16 MB may wait | 59.4 s | 14 MB | 1.5 s | 60.8 s |

The whole takes as long; what changes is that the copy's end is the
truth. (With 64 GB of memory the 280 MB would be the whole 600, and a
copy of gigabytes would end at once.)

What the limit costs the disk's own speed, 1,500 MB written and made
durable, by how fast the disk is (the same throttle):

| The disk takes | the computer | no limit | 16 MB may wait | 32 MB | 64 MB |
| --- | --- | ---: | ---: | ---: | ---: |
| 400 MB a second (the most a 5 Gbit/s USB link carries) | quiet | 3.89 s | 3.92 s | 3.92 s | 3.88 s |
| | every processor busy | 4.02 s | 3.94 s | 3.89 s | 3.89 s |
| 1 GB a second (the most a 10 Gbit/s link carries) | quiet | 1.54 s | 1.60 s | 1.52 s | 1.53 s |
| | every processor busy | 1.83 s | 2.69 s | 2.23 s | 2.32 s |
| 2.7 GB a second (unthrottled) | quiet | 0.56 s | 1.58 s | | 1.02 s |

Nothing at 400 MB a second, busy or not. At 1 GB a second nothing while
the computer is quiet, and a fifth to two fifths more time while it's
busy, whatever the limit's size (the first runs, all on a quiet
computer, had missed this; `kernel-check` run during a build showed
it). So the rule leaves out a disk on a USB link of 10 Gbit/s or
faster: the kind that can take more than 400 MB a second, and that
is written in seconds anyway (an NVMe disk on Thunderbolt isn't a USB
disk at all).

Which disks (data/udev/61-olo-slow-disks.rules): those whose media comes
out (a stick, a card reader), any disk on a USB link slower than
10 Gbit/s, and SD cards. Not an eMMC, which is a small laptop's own disk. A filesystem that keeps its
own account of what waits (btrfs) and one on an encrypted volume (the
mapping is a disk of its own to the kernel) are as before.

How it's set: the udev rule, for each disk as it's plugged in, and the
same two values written now for those here.

## After a kernel update

Ben, 2026-11-16: "make sure all these kernel tunings/mods will continue
to work properly after doing os updates that update the kernel."

- **`preempt=full`** is a start argument, so it has to be in the new
  kernel's boot entry. Fedora's kernel-install makes that entry from
  `/etc/kernel/cmdline` (read from its `20-grub.install`: unless that
  file is older than GRUB's defaults, in which case `grub2-mkconfig`
  writes it again from them first). Olo Doctor's step puts the argument
  in both, the defaults first, and leaves the command line no older
  than them, so the new entry is made from the file as written. Where a
  start was nonetheless made without it (an entry made some other way;
  a computer with no `/etc/kernel/cmdline`, where the entry is made from
  the running kernel's arguments, if a kernel was installed before the
  first restart), Olo Doctor says "The kernel was started without full
  preemption" and its fix puts it in every entry again.
- **The removable disks' limit** is a udev rule and two attributes the
  kernel has had since 6.2: nothing of it is the kernel's version's. On
  a kernel without them the rule does nothing.
- **BBRv3 and the graphics-memory patch** are built again for the new
  kernel by their services at its first start (kernel/README.md).

`tools/dev/kernel-next` checks every one of these on the newest kernel
Fedora's repositories have, before it's installed: its packages
unpacked in scratch, the kernel run in a guest. 2026-11-16, on
7.3.3-300.fc45 (the running kernel 7.2.15):

    ok  started with preempt=full it is on full: (full) lazy
    ok  the running switch takes lazy and full
    ok  woken across processors under load, 99 in 100 run within 11 µs on full (943 on lazy)
    ok  a disk takes its own limit: strict_limit 1 max_bytes 16776930
    ok  and gives it back
    ok  udev with Olo's rule limits a USB stick and leaves the other disk alone
    ok  BBRv3 builds for it and passes its self-check (the full one: 1,265,738 figures over 25 links)
    ok  the live patch builds for it, loads, takes ttm_backup_bytes_avail over and comes out again
    ok  olo-bbr3 and olo-gpu-memory build for it themselves (2 of 2)

## Measured, and left as the kernel comes

### MGLRU's thrash protection (`min_ttl_ms`)

When programs want more memory than there is, the kernel makes room by
dropping what's in memory, programs' own code among it, and reads it
back from disk when it's next used. If that was a moment ago, everything
slows to the speed of the disk: thrashing. MGLRU can be told to keep
what was used within N milliseconds, and to end the largest program when
it can't (`/sys/kernel/mm/lru_gen/min_ttl_ms`; the kernel's own advice:
"N=1000 usually eliminates intolerable janks due to thrashing").
Fedora's is 0. It's the tuning most often recommended for a desktop, and
the first runs here looked like a clear gain.

`tools/dev/kernel-check thrashing`: a guest with 1 GB, swap in
compressed memory as large, `vm.swappiness` 1 (as Olo has them). One
program takes memory at 12 MB a second and holds it; another reads
through 250 MB of a file it has mapped and 150 MB of its own, a page in
every 64, ten times a second (a program in use).

| The first takes | `min_ttl_ms` | it | a round through the file: 99 in 100 within | worst | read back from disk |
| --- | ---: | --- | ---: | ---: | ---: |
| 1,400 MB (more than there is) | 0 | was ended after 92 s | 90.6 ms | 553.6 ms | 181,866 times |
| | 1000 | was ended after 69 s | 2.6 ms | 41.2 ms | 91,898 times |
| 950 MB (it fits, with the other's file pushed out and read back) | 0 | got it all (2 runs of 2; the second: 218 ms, 266 ms) | 54.9 ms | 116.0 ms | 188,230 times |
| | 1000 | **was ended after 34 s, at 408 MB** (and after 69 s in a second run) | 0.15 ms | 34.1 ms | 7,127 times |
| 700 MB (it fits) | 0 | got it all (4 runs of 4) | 0.35 ms | 104.4 ms | 77,624 times |
| | 1000 | got it all in 6 runs; **was ended after 32 to 35 s in 3** | 0.21 ms | 33.4 ms | 51,289 times |

Where the program was going to be ended anyway, it's ended sooner and
what's in use hardly waits: the gain that's reported. But it also ends
a program that fits, at the first moment memory gets tight (after 32 to
35 s each time, when it had a little over half of what it wanted): in 3
runs of 9 here, and in both runs of the case between (after 34 s and
after 69 s). The kernel ends a program when its oldest generation of
pages was made less than N ago (`lruvec_is_reclaimable`, mm/vmscan.c),
and when memory first got tight that was so with hardly anything yet
read back from disk (7,127 times, where a run that went on had 51,289):
nothing was thrashing. A program ended for nothing one time in three is
someone's work lost; never ending it costs the other program rounds of
55 to 270 ms for a minute in the worst case that still fits. Left at 0.

A smaller N may be the way: at 250 ms the program that fits got it all
in 4 runs of 4, and where it didn't fit the worst round was 135 to
152 ms (554 without, 41 at 1000). Four runs don't show it never ends a
program for nothing, and the case between wasn't run: not offered until
it has been run enough to say (docs/TODO.md).

Olo has other answers to memory running out, none of which ends
anything unasked: swap in compressed memory as large as memory, the
window server's own memory locked, windows' graphics memory kept out of
swap, and the supervisor's watch for a program taking too much, which
asks.

### dm-crypt without its work queues

An encrypted disk's reads and writes are handed to kernel threads to
decrypt and encrypt (so several processors share the work), which costs
each small read two hand-overs. A volume can be told to do it in place
(`no_read_workqueue`, `no_write_workqueue`: Cloudflare's, in Linux 5.9,
and Zen's default). `tools/dev/kernel-check left-alone`, megabytes a
second, with the queues and without:

| | on a ramdisk | | | on a disk | | |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| reads of 4 KB, 1 thread | 580 | 2,325 | 4.0× | 50 | 56 | 1.1× |
| reads of 4 KB, 8 threads | 2,650 | 8,445 | 3.2× | 112 | 274 | 2.5× |
| reads of 1 MB, 1 thread | 6,876 | 3,838 | 0.56× | 1,492 | 1,131 | 0.76× |
| writes of 1 MB, 1 thread | 6,052 | 3,567 | 0.59× | 814 | 1,151 | 1.41× |

Small reads, which is what starting a program is made of, are much
quicker (a 4 KB read took 60 µs where it took 100). Large ones are a
quarter to a half slower, because one processor does what four did: a
big file read at an NVMe disk's full speed. That is a trade, and which
side of it a person is on depends on their disk and what they do, so it
isn't offered. It would also mean writing to the header of the person's
encrypted disk, which nothing short of a clear gain justifies.

### Split-lock mitigation

An atomic operation on data that crosses a cache line's edge locks the
memory bus for every processor. Linux slows a program that does it
(`kernel.split_lock_mitigate=1`: it sleeps 10 ms at each, then runs
freely for two ticks): 30,000 of them took 1.08 s on the ThinkPad, where
each alone takes 7 µs. The gaming distributions turn the mitigation off
because some Windows games do it constantly and stutter. It's a brake on
one program for the sake of all the others, and off it helps only a
program that has the bug; Olo runs Wayland programs, where it is rare.
Left on.

### Room kept free for memory asked for

`vm.watermark_scale_factor` is how much memory the kernel keeps free
ahead of need (10: a thousandth of it); raised, a burst of allocations
finds room without waiting for the cache to be dropped.
`tools/dev/kernel-check left-alone`: 500 MB taken at once while memory
was full of a file's cache, three times, at 10 and at 100: a megabyte
came in 0.24 ms either way, 99 in 100 within 0.3 to 1.3 ms either way,
the worst 21 to 58 ms either way (the first time, when the cache was
first dropped). No difference to be had here: the kernel drops clean
cache faster than a program takes memory. Left at 10.

### Not measured, and why

- **`vm.swappiness`, the size of swap**: Olo Doctor sets them already
  (1, and swap as large as memory; docs/HACKING.md, "Swappiness and the
  size of swap").
- **`vm.page-cluster=0`** (swap read a page at a time): with swap in
  compressed memory the kernel already reads one page at a time for a
  page only one program has (`SWP_SYNCHRONOUS_IO`).
- **`vm.dirty_ratio` lowered** (CachyOS: 10): it throttles writes to a
  fast disk too. The slow disk's own limit is the case it's wanted for.
- **The NMI watchdog off**: saves one counter interrupt every ten
  seconds of work for each processor, and gives up the report of a
  hard lockup.
- **Intel's `hwp_dynamic_boost`**, the idle states' depth: the
  processor's own power management doesn't exist in a guest, and trying
  it on the person's computer needs root. Olo's power modes are
  Preferences ▸ Power's.
- **Huge pages always, or deferred** (`transparent_hugepage/defrag`):
  a gain for programs with heaps of gigabytes at the cost of memory
  for everything else; Fedora's "on request" lets those programs ask.

## Where it all is

- `crates/system/src/kernel_tuning.rs`: each setting read (what it is
  now, what the next start brings, its figures), as anyone.
- `apps/doctor/src/kernel.rs`: the steps that put each on or back, as
  root; `olo-doctor --kernel` (what there is, as JSON) and `olo-doctor
  --kernel preemption|slow-disks|bbr3|graphics-memory on|off`.
- Olo Doctor's suggestions `kernel:preemption` and `kernel:slow-disks`
  (apps/doctor/src/checks.rs, `kernel_settings`).
- Preferences ▸ Performance ▸ Kernel (apps/preferences/src/performance.rs).
- `tools/dev/kernel-check` and `crates/system/examples/kbench.rs`: the
  measurements. `tools/dev/kernel-next`: the next kernel, checked.

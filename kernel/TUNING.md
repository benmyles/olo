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
given up for it. A third came of looking at why the desktop lagged
under agents' builds (2026-11-20), measured on the computer itself and
in the guest: **short turns on the processors**. One more was a
setting for an hour that day and was withdrawn, **swap used only when
nothing else is left** ("Tried and withdrawn", below: what a guest
can't measure). Four more things were measured and left as the kernel
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
| The multi-generational LRU (MGLRU), on | On (`0x0007`); its thrash protection off (`min_ttl_ms` 0) | Measured at 1000 and 250 ms, left alone (below) |
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

## The settings

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

### Turns: 0.7 ms

A busy program keeps its processor for a turn before another that is
waiting gets it (the scheduler's base slice,
`/sys/kernel/debug/sched/base_slice_ns`). The kernel's base is 0.7 ms,
and it lengthens it with the number of processors: by as much again for
each doubling up to eight, so 2.8 ms on eight and more. Longer turns
are for getting work done with fewer switches; what they cost is that
whatever is woken while every processor is busy waits out the rest of
somebody's turn, at each step of what it does.

Olo's own threads someone waits on are realtime and wait for no turn
(docs/HACKING.md, "Realtime threads and the kept core"). What they hand
work to in the kernel isn't: a frame the compositor gives the GPU is
passed on by a kernel worker (the DRM scheduler's, an unbound
`kworker`), a byte through a terminal's pty by another
(`flush_to_ldisc`), and those are ordinary threads at nice 0. Measured
on Ben's computer (16 processors, full preemption, Olo's supervisor
on) while other agents' builds kept it at a load of 29: 17% of the
compositor's GPU jobs waited over a millisecond for that worker, 3.6%
over 4 ms, the worst 27.6 ms; typing in Terminal lagged, with the
program typed into answering each key in 2 to 4 ms and the compositor
late on no frame.

The same computer, fifteen more busy programs added at nice 10 as the
supervisor lowers a build's (`SCHED_BATCH`), each setting 12 s at a
time and in turn, three rounds (bpftrace: every wake of an unbound
kernel worker, of Terminal's and its pty service's threads, and the
compositor's GPU jobs from handed over to passed on):

| a turn | kernel workers: woken, waited over 1 ms | over 4 ms | the compositor's jobs: over 1 ms | worst | Terminal's threads: over 1 ms | worst |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 2.8 ms (as it comes) | 0.45% | 0.024% | 0.81% | 14.5 ms | 2.01% | 29.1 ms |
| **0.7 ms** | 0.07% | 0.002% | 0.33% | 5.7 ms | 0.28% | 4.3 ms |
| 0.7 ms, and a woken program let in at once (`NO_RUN_TO_PARITY`) | 0.24% | 0.002% | 0.90% | 2.1 ms | 0.26% | 15.0 ms |
| 0.4 ms | 0.09% | 0.003% | 0.48% | 1.9 ms | 0.23% | 32.4 ms |

Shorter than the kernel's own base gains nothing more, nor does letting
a woken program in before the running one's turn is up; the base it is.
With the fifteen at nice 0 (not yet lowered) the same: Terminal's
threads' worst wait went from 59.9 ms to 7.8. Putting the fifteen in
the kernel's idle class (`SCHED_IDLE`), which is preempted by anything,
did not help the kernel's workers (2.4% over a millisecond): not
pursued.

The plainest form of it, on the same computer (`kbench relay`: one
ordinary thread wakes another through a pipe and goes on working,
while 30 threads compute on its 16 processors; each way in turn, three
rounds, the middle of them, in microseconds until the woken thread
runs):

| 30 threads computing | a turn | 99 in 100 | 999 in 1000 | worst |
| --- | --- | ---: | ---: | ---: |
| at nice 10 | 2.8 ms | 270 | 1,476 | 2,726 |
| | 0.7 ms | 162 | 543 | 2,084 |
| at nice 0 | 2.8 ms | 343 | 2,648 | 4,129 |
| | 0.7 ms | 162 | 281 | 707 |

`tools/dev/kernel-check turns` asks it of a guest (4 processors: 2.1 ms
as it comes), with what the shorter turn costs. What the work itself
got done at 0.7 ms against as it comes (the middle of three guests each
way; the computer was at a load of 20 to 30 meanwhile, so the spread is
wide): 8 threads computing on 4 processors 1.08×, pipes with 16 writers
and 16 readers 1.21×, with 4 and 4 0.97×, 16 threads writing one file
1.08×. Nothing lost that shows. (That run's wake figures aren't quoted:
a guest's processors are themselves made to wait on a computer that
busy, and its tails were the host's. They are to be taken again on a
quiet one.)

With a scheduler loaded through `sched_ext` the setting changes
nothing: it has turns of its own. Olo Doctor doesn't recommend it then,
and the tab says so.

How it's set: the file is in debugfs, root's, and there is no start
argument or sysctl for it. `/etc/olo/short-turns.conf` holds two lines
in systemd-tmpfiles' form (the file's value, and Olo's note of it in
`/run/olo-kernel/turns`, since the kernel tells only root), and
`olo-short-turns.service` has systemd's own tool apply them once
debugfs is mounted (among /etc/tmpfiles.d's they could be run before it
is). Where the kernel isn't locked down it is written at once too.

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
- **Short turns** are a file in debugfs the kernel has had under this
  name since 6.6 (`sched/base_slice_ns`), written at every start by
  Olo's unit; where a kernel hasn't it the line does nothing, and
  Preferences shows the turn as the kernel comes.
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

A smaller N isn't the way either. At 250 ms (`tools/dev/kernel-check
thrashing --ttl 250`, 2026-11-17) the program that fits got it all in 7
runs of 7, and where the program doesn't fit the worst round was 37 ms
(542 without, 41 at 1000). But the case between, 950 MB, which fits
with the other's file pushed out and gets it all at 0 (2 runs of 2),
**was ended in 2 runs of 2**, after 70 s and after 68 s: someone's
program ended for nothing, as at 1000. One run is enough to say it
does: the twenty asked for were stopped there. Not offered; left at 0.

| The first takes | `min_ttl_ms` | it | a round through the file: 99 in 100 within | worst |
| --- | ---: | --- | ---: | ---: |
| 1,400 MB (more than there is) | 0 | was ended after 92 s | 89.8 ms | 541.9 ms |
| | 250 | was ended after 72 s | 10.0 ms | 36.8 ms |
| 950 MB (it fits, with the other's file pushed out) | 250 | **was ended after 70 s, and after 68 s** | 2.4 ms, 1.7 ms | 33.9 ms, 34.3 ms |
| 700 MB (it fits) | 250 | got it all (3 runs of 3; 7 of 7 with the earlier 4) | 2.2, 0.3, 0.4 ms | 34.8, 105.9, 39.1 ms |

(The computer was in use while these ran, its load between 5 and 12:
the guests' times may be longer than on a quiet one; whether a program
was ended isn't a time.)

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

- **The size of swap**: Olo Doctor suggests it already (as large as
  memory; docs/HACKING.md, "Swappiness and the size of swap").
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

## Tried and withdrawn

### Swap used last (a setting for an hour, 2026-11-20)

Ben, 2026-11-20: "i also see we're using 10gb of swap right now but
there is still over 32gb of memory free. i want the swap to only get
used when absolutely necessary, i.e. without it we'd run out of memory.
how can we make linux do that? figure it out and make it an option in
olo performance -> kernel on by default".

His computer (64 GB, swap in compressed memory as large, the
multi-generational LRU as Fedora starts it, `vm.swappiness` 1 as Olo
Doctor had set it) had written 102 GB to swap in three days and read
51 GB back, with tens of GB free or holding files' cache throughout;
in one minute that was watched, 1,458 MB went to swap with 21 GB free.

Why swappiness doesn't say what it seems to (mm/vmscan.c of this
kernel, read):

- **The multi-generational LRU at 1** takes "the files' cache first"
  of each generation, not of the whole: `isolate_folios` scans the
  files' oldest generation and falls back to programs' memory whenever
  that has nothing to scan (`if (!scanned) type = !type`), which under
  a build is often, with tens of GB of younger cache beside it.
- **The multi-generational LRU at 0** never swaps: programs' memory
  isn't evictable at all (`min_type(0)` is the files' type, and
  `for_each_evictable_type` runs over it alone), and nothing brings it
  back in when the cache is gone. The program that asked for memory is
  ended, with all of swap free.
- **The kernel's older reclaim at 0** (the multi-generational LRU put
  out: `/sys/kernel/mm/lru_gen/enabled`) scans programs' memory only
  when the files' cache and what's free are together under the zones'
  high marks (`file_is_tiny`, `prepare_scan_control`). That is what
  the kernel's documentation says of swappiness 0, and it is true of
  this reclaim only. At 1 it swaps an idle program out beside a build.

`tools/dev/kernel-check swap`: a guest with 1 GB, swap in compressed
memory as large. First a program takes 300 MB of its own and sits idle
for 75 s while a build goes on beside it (files written and read back,
100 to 170 GB of them through 1 GB of memory, and compilers that each
take 350 MB for a second), then it's used again. Then memory really
runs out: one takes 950 MB at 12 MB a second (it fits only with swap)
while another keeps reading 250 MB of a file it has mapped and 150 MB
of its own, ten times a second. One guest each:

| | beside the build: moved to swap | the idle program used again | memory running out: the program | moved to swap | the one in use, a round through its file: 99 in 100 within | worst |
| --- | ---: | ---: | --- | ---: | ---: | ---: |
| the multi-generational LRU, swappiness 60 (as Fedora comes) | 0 MB | 1 ms | got it all | 1,504 MB | 76 ms | 100 ms |
| the same, 1 (as Olo had it) | 0 MB | 3 ms | got it all | 1,664 MB | 55 ms | 71 ms |
| the same, 0 | 0 MB | 8 ms | **was ended after 56 s** | 0 MB | 133 ms | 837 ms |
| the older reclaim, 0 | 0 MB | 19 ms | got it all | 813 MB | 200 ms | 317 ms |
| **the older reclaim, 0, room kept free 200 (the setting that was)** | 0 MB | 1 ms | got it all | 869 MB | 132 ms | 178 ms |
| the older reclaim, 1 | 1,015 MB | 204 ms | got it all | 1,480 MB | 123 ms | 228 ms |
| the older reclaim, 1, room kept free 200 | 1,010 MB | 142 ms | got it all | 1,369 MB | 240 ms | 723 ms |
| the older reclaim, 60 (as kernels came before) | 290 MB | 413 ms | got it all | 1,542 MB | 294 ms | 391 ms |

So the setting was the older reclaim with swappiness 0 and the room
kept free at 200 (`vm.watermark_scale_factor`: 2% of memory between
the kernel's marks, where it comes with 0.1%, so that swap began while
programs in use still had some of their files in memory): in the
guest, nothing went to swap beside a build, swap was used when memory
ran out, and nothing was ended. It was known to be a trade (when
memory really runs out, the files of programs in use go before
anything is swapped: 132 ms a round where it took 55 to 76), and
offered because it was asked for.

**What happened on the computer itself.** Put on at 13:33 on Ben's
computer, at a load of 30 with agents' builds going, which also
brought the 10 GB in swap back into memory. Four minutes later 1.7 GB
were free and 5.9 GB could be freed, nothing was going to swap, and
at 13:37:56 NVIDIA's driver failed an allocation of ordinary memory
(`NV_ERR_NO_MEMORY`); at 13:39:32 the card's runtime suspend failed
(its firmware didn't halt: "GSP unload failed at suspend", "can't
suspend"), the kernel had the card in a power-management error from
then on, and everything on it was lost with it. Put off within the
hour; the kernel was swapping 2.1 GB a minute again at once, which is
what that computer's peaks need. Ben: "just remove that particular
swap setting it's obviously dangerous."

**Why the guest didn't show it.** The guest measured programs:
whether one that asks for memory gets it, and what one in use waits.
Both were fine: a program's allocation waits for as long as it takes,
and in the end the kernel swaps for it. What failed was an allocation
that is allowed to fail instead (`__GFP_RETRY_MAYFAIL`, which is how
NVIDIA's driver asks for ordinary memory, nv-vm.c
`nv_compute_gfp_mask`, and how drivers' larger allocations are
commonly made: the kernel tries reclaim a bounded number of times and
then says no rather than end a program). With the older reclaim at 0,
programs' memory is held back until the cache and what's free are
under the high marks, so at the moment a driver asks there is the
least to take and the swapping that would make room has only just
begun. A kernel that swaps a little all along, as the
multi-generational LRU at 1 does, has room made before it's asked
for. And "tens of GB free with swap in use" on that computer was a
reading between its peaks, not a sign that the swap wasn't needed:
102 GB were written to it in three days because its peaks want more
than its memory. A guest scenario for it would be a module that asks
for memory that way while the hog runs; none was made, since the
setting is gone.

**What stands of it:** the reading of mm/vmscan.c above (what each
swappiness means under each reclaim), the guest scenario
(`tools/dev/kernel-check swap`: it still measures who is swapped and
what waits, for whatever is tried next), and the rule it left
(docs/HACKING.md, "Swappiness and the size of swap"): nothing of
Olo's reduces what the kernel may reclaim or swap; what may be
steered is the order, which programs' memory goes first. And nothing
of this kind is put on on the person's computer to see: a guest
first, then the computer only when they say, at a quiet moment, with
the memory record running.

## Where it all is

- `crates/system/src/kernel_tuning.rs`: each setting read (what it is
  now, what the next start brings, its figures), as anyone.
- `apps/doctor/src/kernel.rs`: the steps that put each on or back, as
  root; `olo-doctor --kernel` (what there is, as JSON) and `olo-doctor
  --kernel preemption|short-turns|slow-disks|bbr3|graphics-memory
  on|off`.
- Olo Doctor's suggestions `kernel:preemption` and `kernel:slow-disks`,
  and what it recommends, `kernel:short-turns`
  (apps/doctor/src/checks.rs, `kernel_settings`).
- Preferences ▸ Performance ▸ Kernel (apps/preferences/src/performance.rs).
- `tools/dev/kernel-check` and `crates/system/examples/kbench.rs`: the
  measurements. `tools/dev/kernel-next`: the next kernel, checked.

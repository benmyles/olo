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
room kept free for allocations. And one thing isn't a kernel setting
at all but a way of using the kernel's groups, built after three
reviews and left for the person to turn on: **background work kept to
the memory that is to be had for nothing** (its own section below).

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

### Huge pages when asked for: made by the one who asks (`transparent_hugepage/defrag`)

Fedora gives a 2 MB page to a program that asks for them (`enabled`:
madvise), and where none is free the asking program makes one itself,
at its page fault: it compacts memory and reclaims, and waits meanwhile
(`defrag`: madvise). `defer` gives it single pages at once and wakes
the kernel's own threads to make 2 MB pages for later.

It came up on the day of looking at why the desktop lagged
(2026-11-20): in a trace of 20 s every program held "making room" was
asking for 2 MB pages, a linker 1,267 times. Of 4.6 GB free on that
computer, 132 MB was in pieces of 2 MB and over, and programs had
compacted memory themselves 281,399 times in four hours and found
nothing in two of three (`/proc/vmstat`: `compact_stall`,
`compact_fail`). Who asks: linkers (mold for its allocator's arena and
for its output, 1.6 GB of address space for a program of 257 MB; it
switches huge pages on again where they were off for it). Not rustc.

In the guest (`tools/dev/kernel-check huge-pages`: 2 GB, half of
1.5 GB left in single pages, "free"; and the same with a 1.1 GB file's
cache in what was free besides, "cached", as a computer is that has
run for days; a program asking for 2 MB pages on two threads beside
one using 150 MB ten times a second and a thread waking every
millisecond; the middle of three guests each way):

| | as it comes (madvise) | defer |
|---|---|---|
| free: MB a second | 20,993 | 15,824 |
| free: made to wait | 105 times | never |
| free: a 2 MB page, at worst | 21.9 ms | 4.7 ms |
| free: the kernel's own threads worked | 0 ms | 160 ms |
| cached: MB a second | 21,092 | 20,068 |
| cached: made to wait | 176 times | never |
| cached: a 2 MB page, at worst | 5.0 ms | 5.0 ms |
| cached: the kernel's own threads worked | 60 ms | 170 ms |

The two beside it waited the same either way (a round through 150 MB
0.1 to 0.2 ms at worst, the waker 2.1 to 2.4 ms).

On the computer itself, with nothing of it changed: the compositor's
dev build linked by mold over and over, by turns as it is and with
huge pages off for that one process (`PR_SET_THP_DISABLE` before it
starts, and a seccomp filter that has mold's own
`madvise(MADV_HUGEPAGE)` and `prctl(PR_SET_THP_DISABLE, 0)` answer 0
and do nothing), twenty each way:

- a link took 0.36 s in the middle with huge pages, 0.41 to 0.42 s
  without; the longest 0.99 s with, 0.57 s without;
- forty links as they are were held compacting 3,046 times, 8.7 s
  between their threads (the longest 150 ms), and reclaiming 784 times,
  2.4 s; 18,082 huge pages were given them and 443 refused;
- beside the link looped for 25 s (three rounds each): a thread waking
  every millisecond was late by 995 µs 99 times in 100 with links as
  they are, 830 µs with huge pages off for them, 599 µs with none; at
  worst 10.4, 4.5 and 2.2 ms. A program going through 300 MB of its
  own ten times a second wasn't held either way (0.3 ms at worst).

So what the linker waits, it gets back (it is a seventh quicker with
them), and what others wait beside it is a few milliseconds at the
tail for a thread of ordinary priority. `defer` gives up that seventh
for it: a trade, and not offered. Left as it comes.

What that day's lag had of this was not this setting's at all. The
threads that draw were held for 2 MB pieces too, without ever having
asked: the GPU driver's memory pool asks on their behalf, and `defrag`
doesn't govern it. "Graphics memory in 2 MB pieces", below.

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
- **Huge pages for everything** (`transparent_hugepage/enabled`:
  always): a gain for programs with heaps of gigabytes at the cost of
  memory for everything else; Fedora's "on request" lets those
  programs ask. (How a request is met was measured: above.)

## Background work kept to the memory that is to be had for nothing

Preferences ▸ Performance, "Give compilers and linkers only spare
memory": off as it comes, the person's to turn on, and there to turn
on only in a session Olo's login divided (docs/HACKING.md, "The
session divided" under the login screen and "The background group"
under the supervisor). How it came to be this is the order of this
section: the measurements of the rule, a first build that was held on
its review, and what was built instead, measured again with Olo's own
keeper as the one that keeps.

Ben, 2026-11-20, when the swap setting below was withdrawn: "how can
we optimize what gets swapper better? … so then the work that gets
background priority, like rustc etc would be more likely to get
swapped than interactive apps that will really impact me". Linux has
no link between a process's priority and reclaim; it has control
groups. Ways of using them were measured in guests
(`tools/dev/kernel-check groups` and `groups-cold`), two guests each
unless said:

**Where memory is truly short** (1 GB; a desktop with a 200 MB program
that sits idle and one in use, 150 MB of a file mapped and 100 MB of
its own read ten times a second; the system's services in use; a build
for a minute whose compilers take 350 MB each for a second and whose
files are written and read back):

| | the desktop's program in use: a round through its file, 99 in 100 within | worst | the system's files | the build got done | ended by the kernel |
| --- | ---: | ---: | ---: | --- | --- |
| one group for all, as a login session is today | 72, 71 ms (105, 110, 116 that morning) | 363, 223 ms | 40, 39 ms | 42, 43 compiles; 323, 363 rounds of files | nothing |
| the background's group kept to what is free | 25, 21 ms (27, 38, 36) | 158, 126 ms | 23, 24 ms | 36, 39; 162, 172 | nothing |
| …to what is free or cache the kernel has down as unused | 35, 49 ms | 104, 122 ms | 40, 50 ms | 39, 36; 159, 130 | nothing |
| **…or unused cache that no program has mapped** | **12, 23 ms** | 75, 83 ms | 18, 20 ms | 39, 39; 166, 179 | nothing |
| the desktop's group protected for all it holds (`memory.low`; one) | 4 ms | 243 ms | 31 ms | | **the desktop's idle program** |
| the same, for half of it (one) | 142 ms | 659 ms | 141 ms | | nothing |

**Where there is memory to spare, all of it in use as cache**, as a
computer's is after a day (3 GB, 1.5 GB of it a file read once; the
same desktop and build): the desktop's program waits under a
millisecond at the 99th percentile whatever the rule, and what differs
is the build:

| | the build's rounds of files | its compiles | of the unused cache, left unused at the end |
| --- | ---: | ---: | ---: |
| one group for all | 370, 355 | 42, 42 | 0.5 GB |
| the background's group kept to what is free | 193, 173 | 39, 39 | 1.3 GB |
| …to what is free or cache the kernel has down as unused | 265, 337 | 39, 41 | 0.4 GB |
| **…or unused cache that no program has mapped** | 216, 275 | 37, 40 | 0.4 GB |

- **Protection (`memory.low`) is not the way.** It is the kernel's
  own answer to the question, and with the desktop protected for all
  it held the kernel ended a program with all of swap free: the
  desktop's own idle one. Protected for half, it changed nothing for
  the better. (And Fedora's uresourced already writes the ancestors'
  protection, `MemoryLow=0` on the users' slices at every change of
  who is at the computer: Olo's would be a second writer.) Nothing of
  Olo's may make an allocation fail that succeeds today.
- **Keeping the background to what is to be had for nothing works,
  for what is in use.** Its group's `memory.high` set, four times a
  second while it has anything in it, to what it holds plus what is
  to be had for nothing less a reserve (12% of memory), never under
  what its programs themselves hold unless little is to be had at
  all. Past it the kernel has the group's own programs wait and takes
  the group's own cache and memory first, and the reclaim of
  everyone, which is what takes the files out from under a program in
  use, seldom runs. Where memory is short the desktop's program in
  use waited a third as long or less at the 99th percentile; the
  system's services half as long. The build pays: about a tenth fewer
  compiles, and half its files through. Nothing is ended and nothing
  refused: `memory.high` only slows who is over it.
- **What is to be had for nothing is more than what is free.** A
  computer that has run for a day keeps little free on purpose and
  holds most of its memory as cache (the person's: 0.6 to 8 GB free
  of 64 through an afternoon, 28 GB of cache, 15 GB of it what the
  kernel has down as not lately used, 2.6 GB mapped by programs). By
  free memory alone the background would have no cache at all there
  most of the time, and the second guest shows what that costs with
  nothing gained: half the build's files, 1.3 GB of unused cache left
  unused. Counting all the cache the kernel has down as unused
  (`Inactive(file)`, less the group's own) gave the build its files
  and took back half of what the rule had won where memory is short:
  a file a program has mapped and reads sits on that list between
  touches. Less what programs have mapped (`Mapped`), the short case
  is as good as by free memory alone and the build keeps most of its
  files. That is the rule (the branch's
  `olo_system::background::allowance`).
- **A program that sits idle is swapped all the same** (its 200 MB
  brought back in 100 to 160 ms when next used, under every rule):
  what isn't touched for minutes is the coldest memory there is, and
  when the build's own memory doesn't fit, swapping it into compressed
  memory takes memory too (zram's own isn't the group's). Keeping it
  would mean stopping the build, or forbidding the swap, which is
  the setting below.
What these runs don't say (the second review's; the harness was put
right for the runs further down): there are two guests a
case. What keeps the allowance in the guest is a shell loop writing
the file, which waits while the kernel takes the group's pages back;
systemd writes the same file without waiting, and then a group that is
asleep stays over its allowance, so who writes changes what was
measured. The guest's jobs are in their group before they start; on a
computer they are found and moved, and what they held until then stays
charged where they were. In the second table the desktop's probes end
(165 s) before the build's last allocation is whole. The build's
counts are of rounds begun: one that failed counts. And a guest can't
say what the reserve should be on 64 GB, nor what the rule does over a
day of the person's own work (of 229 minutes recorded here that day,
what was to be had by the rule was under the reserve in two).

### The first build, and why it was held

On the branch: `olo_system::background` and the supervisor's `group`.
A slice of the person's own systemd manager (`olobackground.slice`),
a job (the topmost process named in the Background list, with all it
started) handed to systemd by kernel handle as a scope in it, the
allowance written to the slice's `memory.high` four times a second by
a thread of the supervisor's, a check box in Preferences. It needed
no privilege and no change to login, and it passed its own live
tests. The review (the answer, its probes and their output:
/var/tmp/claude-blueferry/review5) found these, most of them shown
by running the build's own code or a guest, the rest read from
systemd's and the kernel's source:

1. **A new slice comes up under Fedora's `kill` policy.** What a
   transient unit is given at its making (`ManagedOOMMemoryPressure=
   auto`) loses to Fedora's drop-in for every slice
   (`slice.d/10-oomd-per-slice-defaults.conf`); only a property set
   on the unit afterwards (`SetUnitProperties`: a drop-in of its own)
   wins. The build set it afterwards only on a slice it found
   already there, so the test that looked passed and every fresh
   slice was on systemd-oomd's list at 80% for 20 seconds (`oomctl
   dump` showed four of the live test's, which also never removed
   them: removed since, with the others the tests left).
2. **A build's cache server went with its job**, and stayed: the
   names never to be taken (`UNGROUPED`) were checked at a job's top
   and not among what it had started, and what was taken was noted
   as taken before systemd had moved anything, so the taking-out
   never saw it.
3. **A whole job is more than background work.** `cargo`, `go`,
   `npm`, `bun`, `flatpak` start programs someone is waiting for (a
   window, a server an editor asks, a helper that asks for a
   password), and all of it is born in the group, on idle processors
   and an allowance. And out of the login session's scope a process
   is no longer of that session to logind (`sd_pid_get_session`
   answers "no data"): what asks by process (`loginctl`, a lock of
   the session, polkit's choice of who to ask) answers differently.
4. **A request is not a move.** A refusal, a process born between the
   look and the move, a job partly moved: none was seen or tried
   again; the count noted was of handles sent. And systemd moves by
   process number in the end, whatever it was handed.
5. **The allowance has other writers and no owner.** A second
   supervisor (a test session reaches the same manager) lifts or
   sets it; systemd writes its own value back at a reload; the
   keeper wrote only when its figure changed, so a foreign value
   stayed (shown: it survived the keeper's own end); a killed
   supervisor leaves the last allowance for good; turning it off
   said "lifted" without reading it back, and left the idle
   processor weight on what was still running.
6. **The allowance can slow what the desktop waits for.** A program
   waiting on a lock held by one kept to 16 MiB waited 120 ms where
   52 (measured in a guest); the write itself took 48 ms in the
   writer and swapped 80 MiB. The keeper is a thread of its own, but
   its making, its end and the moves ran on the supervisor's
   realtime thread, with waits of up to seconds.
7. **"Cache to be had for nothing" is the kernel's guess, and wrong
   both ways.** Under the multi-generational LRU a file a program
   read sixty-four times in a row was all "inactive"; and a group's
   `file` counts memory files (tmpfs, shared memory) that can only
   go to swap, so a job working in /tmp had no floor (3 GiB of it:
   an allowance of 16 MiB). The answer wanted is a credit that falls
   when the desktop is seen to wait on memory (its own group's
   pressure figures), not a sum of counters.
8. **A window of the group's in front lifted the allowance for
   every build**, and stayed on idle processors itself.

And one the review of the design had already said, which the build
left for the shorter road: **in the person's manager the group
competes for the processors as the manager, not as background work.**
The manager's group and the login session's are siblings of the same
weight; "idle" on a slice is idle among the manager's own slices
only, and nice orders nothing across groups. Measured on the
person's computer with sixteen hogs of the test's own (one run each):
a program in the login session working flat out got 4547 rounds a
second with the hogs beside it in the session at nice 10 and batch
(as today), 4804 with them in a scope of the manager's, 4646 in the
idle slice; woken a thousand times a second it was late by 0.6 ms at
the 99th percentile today and by 1.1 ms either other way (worst 23, 9
and 6 ms). The desktop is no better off for it, and by the tail a
little worse.

### What was built instead, and measured again

The design the first review signed off, with what the second found
(`olo_system::session_groups`, `olo_system::background`, the
supervisor's `group`; the third review, of this build, is in the
commit):

- **The groups are inside the login session's own scope**, made by
  Olo's login as root before the desktop starts: `leader/` (the
  login's own worker, out of the person's reach), `olo/` and in it
  `desk/` (everything, as it comes) and `bg/`. The scope is
  delegated by a drop-in Doctor writes; the person is given what
  moving between the two leaves takes and `bg`'s allowance, no more;
  anything unexpected and the login goes on as it always has. There
  a process keeps its session, systemd-oomd isn't watching, a move
  is one write, the allowance's file is nobody else's, the desktop's
  own waiting on memory is one group's figure, and idle on the
  processors is idle against the desktop.
- **Who goes in**: compilers and linkers by their own name, never
  what starts them, so nothing anyone waits on at a window is ever in
  it; what they start themselves is born there.
- **One keeper**, with a lock, the allowance read back at every
  look, a guard that lifts it if the supervisor ends by any road, off
  read back, and none of it on the supervisor's realtime thread.
- **The rule again**: the floor without memory files; and the unused
  cache counted only while the desktop's group isn't seen to wait on
  memory (`desk/memory.pressure`: a look in which it waited over 1%
  of the time halves the credit, a look without gives a fortieth
  back).

`tools/dev/kernel-check session-groups`: the login's steps as root
against the kernel in a guest, then as the person in `desk`: 37
checks (made only where the scope is delegated and the worker alone
in it, undone whole otherwise; the person can move their own between
`desk` and `bg` and set `bg`'s allowance, and can't touch the worker,
the scope, `desk`'s allowance, `bg`'s idle weight, or make a group;
taken apart whole where the session's first process can't be placed).

`tools/dev/kernel-check groups`, put right as the reviews asked (work
counted is work that succeeded; a run that doesn't finish, a probe
that ends badly or a figure missing fails it; the desktop's probes
run to the end), with **Olo's own keeper as the one that keeps**
(`kbench keep`, the supervisor's code, run in the desktop's group,
the background's idle on the processors as the login makes it). Two
guests each, the two scenarios run side by side on the same computer
(noisier than the tables above, and compared only with each other).

Where memory is truly short (1 GB):

| | the desktop's program in use: a round through its file, 99 in 100 within | worst | the system's files | the build got done |
| --- | ---: | ---: | ---: | --- |
| one group for all, as a login session is today | 96, 94 ms | 182, 207 ms | 64, 55 ms | 41, 41 compiles; 304, 338 rounds of files |
| **Olo's own keeper** | **7, 3 ms** | 38, 27 ms | 7, 7 ms | 40, 39 compiles; 171, 191 rounds |

(An earlier pair, before the keeper's last changes: 90 and 92 ms as
one group, 6 and 16 with the keeper; the rule kept by a shell loop,
as in the tables further up, 18 and 16.)

Where memory is all cache (3 GB, half of it a file read once), the
desktop's program waits under a tenth of a millisecond either way and
the build loses nothing: 42 and 43 compiles and 336 and 429 rounds of
files as one group, 40 and 43 and 335 and 421 with the keeper.

What these still don't say: the guest's compilers start in the
group, where on a computer they are found and moved up to two
seconds into their lives; a desktop program outside `desk` (a browser
in a scope of the person's manager) or one waiting on a compiler
isn't in the figure the credit reads; and nothing here is a person's
own day.

Costs, on release builds: a keeper's look 13.7 µs (four a second
while a build runs, none while the group is empty: it sleeps on the
group's own events); the supervisor's filter of its process table at
each of its looks 19.2 µs among 750 processes (nothing with the
choice off).

**Still to do**: a command that starts a job in the group on purpose
(`clone3` into the group); `tsession`'s desktops and agents' screens
in a third leaf of lower weight; the rule only noted, beside the
memory record, before it is trusted with a day's work. And **on the
person's computer, in stages, each on their word**: Doctor's two
fixes (the login brought up to date, the drop-in) and a new login;
then the choice on with the memory record running, matched real
builds off and on, and what a person feels measured (a key to its
frame against 8.3 ms, sound, the editor's waits), not only a file
walked; a way back is the choice off (the allowance lifted and read
back, everything moved back).

### The keeper's own write, seen on the desktop, and what replaced it (2026-11-21)

With the choice on, on the person's computer (62 GiB, a build
running; read-only: the keeper's thread from /proc twice a second,
the group's own files). The group held 8.6 to 9.3 GiB, 1.3 of it
files' cache; 0.6 to 0.9 GiB were free, 30 available. The desktop's
group was seen to wait, the credit went, and in one look the
allowance fell from 11.56 to 7.84 GiB: the rule's floor, what the
group's programs held at that look. For the next four and a half
seconds the keeper's thread ran at 86 to 92% of a processor, in the
kernel, and made no look; the group's cache went from 0.87 to 0.08
GiB in the first second; then its programs' own went from 7.84 to
7.03 GiB, to swap, with 30 GiB available. (Over a build: 46 and 59%
of a processor in two samples of ten seconds.)

Why: a write of `memory.high` under what the group holds takes pages
back in the writer until the group is under it
(`memory_high_write`); the compilers asked for more all the while;
and the figure taken back toward was the floor of a look by then
four seconds old. The second review's "the write itself took 48 ms
in the writer and swapped 80 MiB" (a 1 GiB guest) was this, small.

What the kernel has for it, each tried in a guest of this kernel
(7.3.4): `memory.high` opened with O_NONBLOCK sets the value and
returns (Linux 6.16: 55 µs, a group of 508 MB over an allowance of
100 still holding 508); `memory.reclaim` takes back a stated amount
in the writer's time, with `swappiness=0` (6.11) files' cache only
(200 MB asked for: 11 ms, files from 300 to 100 MB, the programs'
own untouched). And what it doesn't: a take-back, once asked for,
can't be called off (it sits in the kernel behind the disk's queue:
one of 64 MB took 42 s behind swap on a disk of 4 MB a second, one
of 4 MB over 20), and what it has handed to the disk is written
whatever happens after (12 MB more in the three seconds after the
allowance had risen, there). And `memory.swap.max`: how much of a
group may be in swap, kept by the kernel whoever takes the memory
back (a group of 160 MB that grew by 80 under an allowance of 176:
with its ceiling closed nothing was swapped, and the 80 MB took 1.3
s to get, held back by the kernel; open, 0.17 s and 65 MB of the
group's own in swap).

Built six times, the first five each faulted by a review (Astra, as
Ben asked of this design; every fault made to happen, in a guest or
against the code itself):

1. *The allowance without waiting, and what is over it taken back
   by the keeper in parts.* Faulted by: file pages locked in memory
   counted as cache to drop, so the part for the programs' own never
   came (and, in the rule itself, a floor under what the group could
   come down to: with 96 MB locked a program's next 16 MB sent 103
   MB of the group's own to swap); a session whose login hadn't
   handed over `memory.reclaim` gave nothing back for a group asleep;
   one take-back of 64 MB held the keeper 42 s; on Linux 6.11 to
   6.15 the take-back asked again for what the waiting write had
   taken.
2. *The take-backs on a second thread.* Faulted by: that thread
   acting for a decision that no longer stood. An allowance of 32 MB
   written after `Keeper::stop` had said "lifted", and 225 MB of a
   program by then in the desktop's group swapped; 4 MB of the
   programs' own swapped after the allowance had risen, and again
   because the group had grown between the keeper's two readings,
   with memory to spare; a thread that couldn't be started, and
   nothing said.
3. *The reviewer's own patch for all of that*: every ask numbered by
   the decision it was for, the group read again before each part,
   waiting writes kept for kernels before 6.16 and for sessions
   without `memory.reclaim`. Faulted, by the same reviewer, on Linux
   7.3 and on a 6.14 really booted: a program in the group that grew
   80 MiB under the allowance of the look before had 65 MiB of its
   own swapped by the kernel, in its own time, with twice the reserve
   available and no call of the keeper's in it (the allowance does
   that by itself, between two looks); a part asked for entered the
   kernel after the desktop had let go (4 MiB swapped; on 6.14 an
   allowance by then obsolete was written, and swap went from 57 to
   180 MB); on 6.14 a partial rise wasn't written for five seconds
   (the taker was inside its write); a look waited two seconds on a
   guard started in another's place; two looks that decided the same
   figure counted as one decision; and a part of the cache that was
   only waiting to be tried again let the programs' own be asked for.
4. *A swap ceiling* (`memory.swap.max`) set at every look, closed
   unless the look put the allowance under what the programs hold and
   then open by that much; no waiting write left, so nothing kept on
   a kernel before 6.16; a guard that ended replaced without the look
   waiting to hear from the new one. Faulted (the fourth review; its
   run was ended by its provider's filter, its notes kept): **a
   desktop that suddenly took 110 MB more than was free had 121 MB of
   its own swapped, and the background 4**, where with nobody keeping
   it was 61 and 75: under a closed ceiling the kernel can only take
   the desktop's, and what is available afterwards looks ample again,
   so the rule never saw the need. And: what a part still being taken
   had moved to swap was counted twice in the next ceiling (4 MiB too
   many let through); starting the guard's replacement still ran on
   the keeper's thread (a look of two seconds while the new program's
   start was held); a ceiling that couldn't be read was taken for
   none.
5. *The ceiling open when the computer is short*: open (not a
   figure) at every look that puts the allowance under what the
   programs hold or comes within half a gigabyte of it; looks every
   50 ms within one reserve of the line; what of the desktop's own
   went to swap lately counted as needing its room back; nothing
   written by waiting (so nothing kept before 6.16); the guard
   replaced on a thread of its own. The fifth review: "ship with the
   changes listed", with its patch for them. **What went to swap
   lately was counted by its rises alone**: swap going up and down by
   64 MiB four times a second around a gigabyte of old stood at 909
   MiB after half a minute, and took the allowance of its example
   from 1024 MiB to 208; a rise across an hour nobody looked counted
   as just gone. **At looks 50 ms apart a cache slow to give out
   starved the rest**: 80 ms to hear "no more" of the cache, every
   look beginning again at the cache, and the programs' own never
   asked for (three tries at the one, none at the other, in 2.5 s).
   **A session kept by its allowance alone had a growing build's own
   swapped with memory to spare** (65 MiB of 80; none where
   everything was handed over, none with no allowance at all). A
   guard's replacement still being started when the keeper was
   stopped stayed a zombie under the supervisor; a clearing that
   couldn't read the ceiling back came out as "cleared". The guest
   for the sudden need couldn't have a closed ceiling at all (640 MB
   is all within the band), and its check passed with nothing swapped
   on either side. And a table of sentences that said more than the
   code did ("never", "goes first", "by any road").
6. *As it is* (`olo_system::background`): that patch whole, and two
   things on what it called the weak point, the time to the next
   look. The sooner looks begin far enough off that, by the
   arithmetic, a need of ten gigabytes a second doesn't reach the
   band between two looks: within the band's half gigabyte and
   another 2.5 GiB of the line (on 8 GB under 4.9 GB available, where
   it was 2.9; on 64 GB under 18.4, where it was 23). And the cache's
   credit falls by the time the desktop was seen to wait, not by the
   look (at the sooner pace it went five times as fast as it was
   measured at: five such looks left an allowance of 512 MiB in the
   review's 8 GiB example, 928 now). A sixth review held both to the
   code: "ship with changes; keep `near` and the elapsed-time
   credit". The arithmetic is right for its rate and for looks on
   time, and promises nothing past that (a look 5 ms late, at that
   rate, finds 51 MiB less than the line); the same stall counts for
   less when looks are sooner (3 ms in a quarter of a second leaves
   half the credit at the ordinary pace, 0.89 of it at the sooner
   one); a guest of 6 GB showed the keeper go from one pace to the
   other (21 waits of 250 ms, then 235 of 50); the guests' checks
   said more than they showed (events counted in samples, not by a
   clock; "went first" from two totals; a ceiling taken for closed
   from the allowance alone), and so did three sentences. Its patch
   for those is in.

What it does, and how far each goes (the fifth and sixth reviews
held every sentence to the code):

- **What the rule forbids, the kernel refuses, while there is more
  than the band to spare.** The group's swap ceiling is closed at
  every look that leaves its programs what they hold with twice the
  reserve and half a gigabyte available: whoever then takes memory
  back (the group's own programs under their allowance, the taker, a
  part asked for a moment too late) takes cache, or is slowed, and
  the group is charged no more swap. (What was already handed to the
  disk is still written.)
- **When the computer is short, the background's is meant to go
  first, and mostly does.** The ceiling is open (not a figure) at a
  look that puts the allowance under what the programs hold or comes
  within the band; looks come every 50 ms before the line can be
  reached; and what of the desktop's went to swap lately (its
  group's swap seen to grow, a fall paying back, half as much every
  five seconds) counts as needing its room back. Open, the kernel
  takes from whom it will: an allowance under what the programs hold
  and the taker's asking lean it towards the background's, no more.
  What came of it is below (how much of each group's was in swap
  before and after; not who took it).
- **The band's price**: within it a build that grows past its
  allowance has its own swapped though memory is to spare (49 MiB of
  80 in the fifth review's guest: an allowance of 176 MiB, never
  less than 552 MiB available, the line at 347).
- **A look never asks the kernel for memory back, nor waits for a
  process.** (It has no bound on the clock all the same: the
  scheduler and the kernel's own counting can hold it; 25 ms once in
  these guests, on a busy host.) The allowance is only ever written
  without waiting, so on a kernel before 6.16 nothing is kept, and
  said why. A guard that ended is replaced on a thread of its own,
  the group without allowance and ceiling until the new one says it
  is there.
- **A session whose login didn't hand over `memory.reclaim` and
  `memory.swap.max`** is kept by the allowance alone, and only while
  less than the line is available; it says so once, with what is
  missing and what may bring it (the login's update and a new login).
- **What a look costs**: a fifth of a millisecond of the keeper's
  thread on a real group (the sixth review's guest: 16 looks in four
  seconds, 3.2 ms; 79 looks, 13.5 ms), so 0.08% of a processor at
  the ordinary pace and 0.34% at the sooner one, while anything is
  in the group, and nothing while nothing is. (The in-tree timing,
  18 µs, is the reading and the rule alone.)

The desktop suddenly needing more than there is (the fourth review's
guest: 242 MB of the background's own, 118 MB of the desktop's
asleep; the desktop takes what is free and 110 MB more), and what is
in swap five seconds on:

| in 640 MB | the desktop's | the background's |
| --- | ---: | ---: |
| nobody keeping | 61 MB | 75 MB |
| kept by the allowance alone | 63 | 76 |
| the fourth form: the ceiling closed | 121 | 4 |
| a ceiling reckoned at each look, looks at 250 ms | 111, 95 | 49, 53 |
| open when short, looks at 50 ms near the line | 47, 0, 0 | 96, 171, 175 |
| the fifth form (open within half a gigabyte of the line too): six runs here, three of the fifth review's | 0, 40, 0, 0, 0, 0; 0, 49, 0 | 158, 99, 164, 163, 138, 144 |
| the fifth review's patch on it: its four runs | 0, 0, 35, 51 | 99 with the 51 |

As it is, the same need four ways (`kernel-check keeper`; how much
more of each group's was in swap five seconds after the desktop had
what it asked for; all the runs there were, this form's):

| | runs | the desktop's | the background's |
| --- | ---: | --- | --- |
| 640 MB, the ceiling open from the start (all of it is within the band) | 12 | none in 11; 37 MB in one | 163 to 211 MB; 99 with the 37 |
| 1792 MB, the ceiling closed when the need begins | 6 | none | 221 to 227 MB |
| the same, the background's memory in use | 3 | none | 227 to 228 MB |
| 6 GB, the ceiling closed and the keeper at its ordinary pace | 2 | none | 220, 223 MB |

Mostly, not always: where the ceiling is open the kernel takes from
whom it will, and the keeper's part is a race (in the run that lost
37 MB the desktop had its 180 MB in 135 ms; the background's swap
was seen to grow 44 ms in, the desktop's 97 ms in). Where it was
closed when the need began, it was seen open 0.1 to 0.7 s into a
burst of 1.3 GB that took 1.3 to 2.9 s, and 5 to 9 s into one of
5.5 GB that took 10 to 13 (the keeper going from four looks a second
to twenty on the way), each time before any of the desktop's went.

`tools/dev/kernel-check keeper` holds it to all that in guests: a
group asleep holding a mapped file and memory of its own, memory of
its own only, a file locked in memory, in a session whose login
handed everything over (three seconds after the desktop took what
there was: the cache given back, the group at its allowance, its
own in swap, the ceiling open while the allowance is under what its
programs hold; the locked file stays and its own goes) and in one
from before (the keeper saying, whole and nothing else, that it
keeps by the allowance alone; the group holding what it held); a
program growing 80 MB under its allowance with memory to spare (3
GB, 1.8 of it a file the desktop has mapped: the ceiling closed,
nothing swapped, where 65 MB were); the desktop's sudden need four
ways (above: that the background's swap grew over the burst and by
no less than the desktop's, each event by the guest's clock, the
keeper's looks counted before the need and through it; the 6 GB one
only on a computer with 16 GB to spare, and said when it isn't run);
swap on a disk of 4 MB a second (the allowance rose 54 ms after the
desktop let go, no look took over a millisecond, nothing more of the
group's own went to swap after the rise). 37 checks.
`session-groups` (51 now): the person may set `bg`'s swap ceiling
and have its memory taken back, and is refused `desk`'s. On a 6.14
really booted (Fedora 42's first kernel, its packages unpacked in
scratch: `OLO_GUEST_KERNEL`) the keeper doesn't keep and says why,
nothing of it stands, and the login's 51 are as they should be. The
fourth review's own experiments again, against the fifth form: a
guard killed while its replacement's start is held two seconds (the
keeper's longest look 0 ms, where it was 1,985; the group without
allowance and ceiling meanwhile); the keeper and its guard both
killed (the allowance and a closed ceiling left standing until a
supervisor's next start clears them, `clear_if_unkept`: lifted,
nothing left in the group). No guest shows a need of ten gigabytes a
second, nor a computer of 8 or 64 GB: the band's half gigabyte, the
five seconds and the two paces are reckoned, not tuned.

The build scenario where memory is truly short (`groups`, the case
`olo`: 1 GB) again, the keeper as pushed and as changed **in turn**
(`OLO_KBENCH`), on a host busy with other builds throughout. Two
sittings: the fifth form beside two runs of the keeper as pushed,
then this one beside two more. One run each of the changed, then
stopped (Ben, 2026-11-21: "let's skip the rest of the before/after
measurement runs it's a little excessive"): two runs that agree, and
nothing of a tail. Where memory is all cache (`groups-cold`) neither
was measured; the third form's figures there, within the noise of
its parent's, are the last.

| | the keeper as pushed, four runs | the fifth form | as it is |
| --- | --- | --- | --- |
| the desktop's program in use: a round through its file, 99 in 100 within | 2.4, 9.9, 3.3, 4.6 ms | 0.24 ms | 0.43 ms |
| its worst round | 46, 64, 99, 55 ms | 64 ms | 87 ms |
| a round through its own memory, 99 in 100 within | 2.4, 1.8, 8.0, 2.4 ms | 0.16 ms | 0.24 ms |
| the system's files, 99 in 100 within | 3.8, 4.8, 1.7, 10.3 ms | 0.11 ms | 0.21 ms |
| the idle program's 200 MB brought back in | 140, 157, 221, 143 ms | 132 ms | 121 ms |
| the build got done: compiles | 36, 36, 30, 37 | 34 | 33 |
| and rounds of files | 158, 126, 99, 120 | 36 | 31 |
| the keeper's own time on the processors | 3.3, 3.8, 4.5, 3.6 s | 2.9 s | 2.5 s |

The desktop's programs in use waited a tenth as long at the 99th
percentile, the system's services less still; their worst rounds
are as they were. The build compiled as much and got a quarter of
its rounds of files through. Which of the changes did which isn't
measured. In 1 GB twice the reserve and the band are three quarters
of the computer, so the ceiling is open and the looks are the
sooner ones nearly throughout: the band's price where it is
dearest. On a computer of 64 GB the band is a thirtieth of the
line; nothing was measured there.

What they don't say: the keeper's time is its whole process's over
the whole scenario (the taker's too, and the minute after the build);
three runs say nothing of a tail; the guests' compilers start in the
group; and why the build's rounds of files differ isn't settled (the
third review counted the build's own programs entering the kernel's
take-back 322 times a round with the keeper as pushed and 383 with
the third form, the compiler's own share the same in both: more work
a round, not attributed further). **To do**: the same on a quiet
computer, with each group's own processor time and waiting, the
build's files brought back from disk a round, and the desktop's disk
while the background's memory is being written out; what would
settle the band and the two paces: by how much what is available
falls between two looks in real bursts on a computer of 8 GB and of
64, with the most of the desktop's in swap and how long it took to
come back (the fifth review's words; the keeper could keep the
steepest fall it saw); what Doctor and Preferences say on a kernel
before 6.16 and in a session from before the hand-over (the keeper's
note is in the log only); a keeper and its guard both killed with no
supervisor to start again (the allowance and the ceiling then stand
until the session ends).

## Graphics memory in 2 MB pieces

Found 2026-11-21 with `sudo tools/dev/memory-holds` (who is held while
the kernel makes room, by thread, by what asked and from where in the
kernel), on the computer Olo is written on: Linux 7.3.4, Intel graphics
on the xe driver, memory in small pieces as above.

The GPU drivers' memory pool (TTM, `drivers/gpu/drm/ttm/ttm_pool.c`)
fills each buffer with the largest pieces it can get. Since Linux 6.19
a driver can tell it which size is worth having, and since 7.2 xe says
2 MB. Linux 7.3 made the pool wait for nothing but that size and
single pages ("drm/ttm: Issue direct reclaim at beneficial_order",
a56a73ec85c8), and then try *hard* for it ("drm/ttm/pool: Try harder
for beneficial orders", dd8dde5745ea: `__GFP_RETRY_MAYFAIL` in place of
`__GFP_NORETRY`, in its own words "at the cost of longer allocation
setup time. That tradeoff is acceptable for beneficial-order
allocations: higher-order backing pages can improve TLB hit rates").
Trying hard is compacting and reclaiming, on the thread that made the
buffer. Before 7.3 the pool made one light try, waiting, at every size
from the driver's down to two pages: the day the desktop was found
lagging under builds (2026-11-20) this computer still ran 7.2.15, and
that day's first trace already had Chrome held compacting for 2 MB
pieces 37 times in 20 s. It has run 7.3.4 since that evening, and what
follows is of 7.3.

Ten minutes of the usual load, a build among it (each hold attributed
to `ttm_pool_alloc_page` being on the thread's stack):

| thread | held | in all | longest | of 4 ms and over |
|---|---|---|---|---|
| Chrome's main threads (its GPU process) | 591 times | 4.2 s | 327 ms | 119 |
| one of Chrome's pool threads | 144 times | 1.6 s | 502 ms | 31 |
| kit apps' drawing threads ("display") | 37 times | 1.0 s | 390 ms | 13 |
| a kit app's window thread | 64 times | 0.6 s | 458 ms | 5 |
| the compositor's loop | 11 times | 13 ms | 7.3 ms | 1 |

every one from `xe_ttm_tt_populate`, and none of those programs asked
for a huge page. A frame at 120 Hz is 8.3 ms. (The same ten minutes had
a video encoder's threads and a virtual machine's held far longer for
huge pages they did ask for: theirs to wait for, above.) With a linker
looping beside it for 157 s the compositor's loop was held 34 times,
8.5 ms the longest.

Nothing the kernel offers turns it off (`/sys/module/xe/parameters`,
`/sys/module/ttm/parameters`: nothing for it; `defrag` is for a
program's own pages).

### What was built: `olo_gpu_pages`

With Astra from the design on, as Ben said of anything in the kernel:
a design review, then four reviews of what was built and of what was
built around it; all in guests on this kernel, each with must-fix
findings made to happen there, all of them taken. The last ends: "yes
to one owner-authorized trial" on this one build; "the public offer
stays on hold".

**What it does.** For one PCI device, bound to xe, the pool's 2 MB try
is made without `__GFP_DIRECT_RECLAIM` (and `__GFP_KSWAPD_RECLAIM`
kept): a 2 MB piece that is free is taken, the kernel's own threads may
be woken to make more, and otherwise the pool goes on with smaller
pieces, as it does by itself when a size is refused. Every other size,
single pages and every other device's pool are as the kernel has them.
Background reclaim stays because it is what makes the next large piece:
in the fragmented guest, with it 28 wakes of the compactor and 9 large
pieces got; with both kinds of reclaim off, none of either.

**How.** A probe at `ttm_pool_alloc_page()`'s entry (a kprobe on the
function's ftrace site) that changes its gfp argument, the kernel's own
function then running. Not a live patch of the function, which would
carry a copy of its body, its private struct and its accounting, to be
kept true to every kernel build. The first review preferred the live
patch and the second, asked to break the choice, did not hold to it
("D1 is not a must-fix. Exact-build qualification is.").

**What the reviews changed**, each shown in a guest before it was
taken:

- *Only the pool's hard try, for one driver.* The first draft took
  direct reclaim off every size above a page for every driver.
- *A probe can be in and change nothing* (probes disarmed for the
  whole kernel: `insmod` succeeds, the flags are the kernel's own). So
  the module checks itself as it loads, watching the page allocator's
  tracepoint for what reaches it, and doesn't load unless the change
  arrived.
- *It must not touch anything real before it knows the kernel.* The
  first build registered its probe switched on and then checked: in a
  7.2.15 guest it refused to load twelve times, and 5,539 of another
  task's 100,000 buffers had their try changed meanwhile. Now, until
  the check has passed, the probe compares the pool's address with the
  module's own test pool before it reads anything, and changes only
  that pool's try, on the loading task (0 of 100,000).
- *The check must go the way real buffers will.* The first build's
  check had a way round the test of the driver's name: a module with
  that test broken passed its check, announced success, and changed
  nothing. Now the test pool's owner is the device itself, so the check
  passes only through the same test. The device and its driver's module
  are held while the module is loaded, and the probe compares pointers
  (the name was read from a driver that could be going).
- *Where a build keeps the function's arguments is not something a
  module can check as it loads* (a build that passed them elsewhere
  would have the probe read a pool that isn't one before any check
  could refuse). So the module is loaded only into kernel builds on a
  list, `kernel/gpu-pages/QUALIFIED`: the kernel's build ID, its ttm
  module's, and the build of Olo's live patch that was in the kernel
  beside it (a live patch changes the kernel under the same notes: the
  module loads with none, or with exactly that one). A new kernel is
  not on it: the module stays out, said as such, until it has been
  qualified and an Olo with its line installed.
- *No tool certifies a build.* `tools/dev/gpu-pages-check --qualify`
  runs the guests on the very build (its IDs read inside each) and
  reads what it can of the function (its shape from the kernel's type
  information; its machine code taking the order, the flags and the
  pool from the registers the probe reads; the branches to it). The
  third review fed that reading shapes the probe would misread (two
  argument registers exchanged, an address taken and never read, a
  clone of the function, a jump into its middle) and it passed all
  four; it also crashed at its last step, having never been run through
  its own entry point. So it says a build's words and that they are
  pending a person's review, which the list's own header sets out
  (caller and callee at the probe's site, the function's range and
  clones, every relocation in code and data, the pool's layout, the
  falling back on refusal), and who read what goes in the commit that
  adds a line.
- *Kernels before 7.3 are another policy.* Their pool waits at 1 MB too
  (`0x392cc2` at both sizes in a 7.2.15 guest): the module refuses them
  rather than leave a blocking try one size down.
- The check tool's own assertions were weaker than they read (it
  passed with rows deleted from its input): they now check each trial's
  exit, each size's row, the page totals and the fallback.
- *The service switched on a module it never loaded.* With a module in
  the kernel already (a trial's, switched off), on a build that wasn't
  listed and with no device, `olo-gpu-pages` wrote its switch on. It
  now gives each load a name of its own, loads it switched off,
  switches on only what carries that name, for the device, in a kernel
  still as it was checked, built for that very build; and removes
  only its own, switched off first. It and the trial take one lock.
- *The trial tool could leave its module in and on.* Its first form
  went on to load after a warning in the phase before, called a run
  whose tracing never attached a success, removed whatever module was
  there, and after a kill left the module changing tries for good.
  Now: the kernel's log is followed record by record from the first
  phase (a record lost ends it); tracing must attach whole; the load
  carries the trial's name and only that is removed; and a trial's
  load is on only while the trial holds a switch of the module's open
  (/dev/olo_gpu_pages_trial_switch), so whatever ends the trial puts
  the kernel's own behaviour back. The tool itself is run in guests
  (`gpu-pages-check --trial-tool`): to its end (344 tries changed, no
  holds in a guest, so it says it says nothing either way); and
  stopped while its module is on by a kernel warning (off 2 ms later,
  out of the kernel), a SIGTERM (7 ms) and a SIGKILL (2 ms, still in
  as the trial's own, out with `rmmod`).
- *Held back means held back for everyone.* A switch in the
  environment had Olo Doctor offer it on a real computer, and a module
  put in by hand made the Kernel tab's switch pressable. Now nothing
  switches it on while it isn't offered, whatever is in the kernel;
  what is shown (in the kernel, switched off) is not what may be done.
- *The fourth review, of how the third's were taken*: the module's
  code unchanged by it and the service's branches the same; two faults
  left in the trial tool. A kernel warning that doesn't call itself
  one (the graphics driver's own "VM worker error", at warning level)
  didn't stop it: the module was still on a quarter of a second later,
  and the run ended saying nothing of it (any record at warning level
  or worse now ends the trial). And a phase in which no frame was late
  ended a healthy trial (`journalctl -g` ends 1 where nothing
  matches, as it does where it can't read: the lines are now picked
  out here). Its own test gained what it didn't exercise, in the same
  guest: tracing that fails, tracing with a probe missing, a warning
  with nothing loaded yet (each must stop it before it loads
  anything), and another's load getting in first (left as it was);
  and it runs the real journalctl. The qualifier now holds the live
  patches to the rule the service and the trial do.
- *What the list's fourth word is, and isn't.* The same source,
  kernel and compiler gave Olo's live patch another build ID when it
  was built in another folder (the folder's name is in its debugging
  data): the ID listed is of the one `olo-gpu-memory` builds in
  /var/lib/olo-gpu-memory/RELEASE. Whether another computer's comes
  out the same there is not known. Before it is offered beyond this
  computer: that shown, or each computer's build qualified by itself.

**In guests** (`tools/dev/gpu-pages-check`; a guest has no GPU, so a
module of ours fills a pool with xe's flags, owned by QEMU's host
bridge bound to a stand-in driver named xe): in memory with one page
kept of every free 2 MB piece, 64 buffers of 2 MB:

| | as the kernel has it | with the module |
|---|---|---|
| the thread went compacting | 113 times | never |
| the thread went reclaiming | 56 times | never |
| tries for 2 MB given | 12 of 64 | 8 of 64 |
| the kernel's compactor woken | 0 times | 32 times |
| every buffer filled | yes | yes |

Beside Olo's live patch in the same module of the kernel, either
loaded first; `on` written 0 puts the kernel's own behaviour back
without unloading; unloaded during 100,000 fills, the rest ran with the
kernel's flags; a suspend and resume of the guest's devices changed
nothing. A fill of one page costs 80 ns without the module and 95 to
98 with it loaded (the reviewer's guest, one processor of the host's
kept to it; 82 and 96 to 107 here): 15 ns, the probe.

**What it gives up, and is not yet known.** With 384 buffers held at
once in memory fragmented with movable pages (the reviewer's guests,
one pair): 181 pieces of 2 MB got where the kernel's own way got 271,
100,942 single pages where it took 50,062, and the longest whole fill
98.6 ms against 68.1 (why isn't known: that run recorded no reclaim
for single pages at all, and the fixture times whole fills). Fewer
large pieces may be more page-table work for the GPU, which no guest
has. Its count, `changed`, is of tries made without waiting allowed,
whether or not they would have waited: it is not waits spared. No
guest can say what any of it is worth against a window that stops for
half a second.

**So it is built, checked and not offered.** `olo-gpu-pages` installs
and loads it; Olo Doctor and Preferences ▸ Performance ▸ Kernel say
what there is where it is in or installed; neither offers it, and
nothing switches it on (`kernel_tuning::GRAPHICS_PAGES_OFFERED`).

*The trial*, on a computer with Intel's graphics and with its owner's
word: `sudo tools/dev/gpu-pages-try --on-this-computer` (the one build
on the list, its live patches as checked, nothing else at work on the
module). It ends 0 only if its phases with the module off reproduced a
hold of 4 ms and its phases on changed tries; anything else says
nothing either way. Its figures go here.

*And before it is offered*, by the third review's word, more than that
trial: the same matched runs repeated, with what the person sees
measured beside them (frames and input to the display, their tails;
the GPU's work done; the kernel's own threads' work), once more with
no tracing at all (the tracer costs a cheap call 350 to 600 ns); the
service unloading before the driver is removed, and choosing again
after the device is replaced (the module holds the driver's module,
which blocks `modprobe -r xe` and nothing else; a new device at the
same address is another object, and the old load would cover nothing);
a real xe suspend, resume and reset with it in; and in guests the
cases not yet covered (pools that use coherent DMA memory, another
size named, a fill that fails part way, a restore).

For the driver's people, to carry in the kernel itself: a pool flag
selected by xe, three lines (docs/reports/ttm-pool-holds.md, a draft
for Ben to send).

Seen beside it, and not yet weighed: xe makes a new display page table
for every page flip (`intel_plane_pin_fb` → `__xe_pin_fb_vma_dpt` →
`xe_bo_create_pin_map`: 180 a second on the compositor's loop here,
3 µs each in the middle and 6.5 ms at worst in 45 s with no one held
making room; small, 64 KB for a 32 MB framebuffer, a size the pool
doesn't wait for), where i915 kept one for each framebuffer. In the
same draft.

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

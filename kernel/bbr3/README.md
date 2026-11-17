# BBRv3 for TCP, as a module (`tcp_bbr3`)

How fast TCP sends is its congestion control's to decide. Linux's
default, CUBIC, sends faster until a buffer along the way overflows and
a packet is dropped: an upload keeps the slowest link's queue full, and
everything else waits behind it. Linux's own `bbr` is BBRv1 (2016),
which goes by a model of the path instead, but loses many packets where
buffers are small and takes more than its share beside other
connections. **BBRv3** (Google, 2023) fixes both. It isn't in Linux:
Google's is a kernel with patches to TCP itself.

This is Google's BBRv3 made into a module for the kernel as it comes
(written for Linux 7.2), registered as the congestion control **`bbr3`**
beside the kernel's own `bbr`. Olo Doctor offers it ("TCP could send
with BBRv3"); `olo-bbr3` (apps/bbr3, systemd's `olo-bbr3.service`)
builds it for the running kernel, loads it and makes it what new
connections use, at each start. It governs what this computer sends
(uploads, calls, what it serves to others); what it receives is paced
by whoever sends it.

## What's here

- `upstream/tcp_bbr.c`: `net/ipv4/tcp_bbr.c` of
  [github.com/google/bbr](https://github.com/google/bbr), branch `v3`,
  commit 90210de4b779d40496dee0b89081780eeddf2a60 (2025-03-18, on Linux
  6.13.7), untouched. Dual BSD/GPL, as it says.
- `tcp_bbr3.c`: the module. That file with Olo's changes: `diff
  upstream/tcp_bbr.c tcp_bbr3.c` is all of them. The algorithm is as
  Google wrote it; what changed is where it gets what it needs, and the
  module's entry points, which follow the line "Olo: the module's side
  of it".
- `bbr3_olo.h`: the log of snapshots (below).
- `bbr3_selfcheck.h`: for a build that checks itself, never the one
  installed (below).
- `Kbuild`.

## What Google's patches give BBRv3, and what the module does instead

Google's BBRv3 is `tcp_bbr.c` and some seventeen patches to TCP. TCP is
built into the kernel, so having them means running another kernel
(XanMod's and CachyOS's do). The module runs on the kernel there is,
and makes what BBRv3 needs of those patches from what any congestion
control is given:

| Google's patches | The module |
| --- | --- |
| Each packet records, as it's sent, the packets in flight just after it and the packets lost so far (`tx.in_flight`, `tx.lost`, in its control block), and a rate sample carries its packet's (`rate_sample.tx_in_flight`, `.lost`). It is how BBRv3 knows what share of a flight was lost, and at how many packets in flight the loss set in. | Looked up. TCP calls a congestion control as each burst of sending begins (`min_tso_segs`), after each ACK (`cong_control`) and at a timeout (`set_state`), always with nothing sent since the last thing that changed what's in flight; the module takes a snapshot at each (the next new byte, packets in flight, lost and delivered so far) and keeps it if new data was sent from it. A packet of new data is then found by where it lies in the sequence space: its figures are its snapshot's, plus the packets sent from the snapshot to its own end. Each retransmission is noted by itself as it's sent (a probe on `__tcp_retransmit_skb`, as it returns), and found by its end and the delivered count it went at (`tx.delivered`, which the kernel does record). A snapshot is let go when everything sent from it is acknowledged; the log grows with the bursts in flight (16 entries, 384 bytes, at first; 2,048 at most, after which the oldest go and what was sent from them isn't known, which BBR takes as no information). A FIN riding on data takes a sequence number and no packet, and is counted so. One thing differs on purpose: where part of a packet is acknowledged selectively and joined to the packet before it, the patches take the joined segments off the rest's count (`tcp_shifted_skb`); here the rest keeps the count it was sent with, which is what was in flight when it went. |
| A call when a packet is marked lost (`skb_marked_lost`, in `tcp_mark_skb_lost`). | A probe on `tcp_mark_skb_lost`'s entry (ftrace's fprobe), taking the same two branches that count a loss. |
| An event when a tail loss probe repaired a loss (`CA_EVENT_TLP_RECOVERY`), the application-limited state of what it repaired (`tlp_orig_data_app_limited`), and `rate_sample.is_acking_tlp_retrans_seq`. | Told from how TCP leaves the probe: `in_ack_event` notes the probe an ACK finds open, and `set_state(CWR)` with that probe closed and the ACK above it is the recovery; the repaired data's snapshot says whether it was application-limited. |
| A bigger room for the congestion control's state in the socket (144 bytes; this kernel's is 104). | The state is allocated, and the room holds its pointer: freed under RCU (`ss` reads it without the socket's lock), and tied to its socket (a copied socket's is none). Until there's memory for it, a connection runs Reno's window. |
| `tcp_rate_check_app_limited` called as each ACK arrives and when a timer fires. | Called as each ACK arrives (`in_ack_event`); not at timers. |
| The burst's size chosen by the congestion control (`tso_segs`). | The kernel's own `tcp_tso_autosize`, which has had BBR's allowance by round-trip time since 5.18; with the system's floor it computes what Google's `bbr_tso_segs` does. |
| ECN for routes marked as shallow-marking (`RTAX_FEATURE_ECN_LOW`), CE events without requiring ECN, ECT kept on retransmissions. | Never used (`bbr_can_use_ecn` says no). No route on the Internet is so marked, so Google's BBRv3 doesn't use ECN there either. |
| The receiver's "fast ACK mode" (ACKs sent sooner while receiving). | Not there (it's how ACKs are sent, not how data is). |
| More in `tcp_bbr_info` (`ss -i`, `TCP_CC_INFO`): the phase, the version, the bounds. | The kernel's five figures, as BBRv1 gives them: the bandwidth, the least round trip, the two gains. |

The probes read their functions' arguments by position, so the build
checks that this kernel declares the two functions as the module takes
them (`static_assert` on their types): a kernel that changes either
doesn't get a module built, rather than one that reads the wrong thing.

## How it was shown to be right

Nothing of this is tried in the person's own kernel. `tools/dev/kvm-guest`
starts this computer's kernel in a KVM guest, with a root made of this
computer's programs; `tools/dev/bbr3-check` makes links in it (network
namespaces, `tbf` and `netem`) and runs flows over them (`tcp-flow`,
crates/net/examples).

**The figures looked up are the true ones.** A build with
`BBR3_SELFCHECK` also records what Google's kernel records, where it
records it (a probe on `__tcp_transmit_skb`: the packets in flight with
each packet sent, and `tp->lost`), and compares every figure the module
looks up with it. (It found two things on the way: the FIN above, 3
figures in 560,000 off by one; and a fault of its own, a kretprobe that
isn't run for a call arriving while another probe's handler runs, which
left a stale true figure once in 100,000 and blamed the module. It is an
fprobe now, as the module's are.) `tools/dev/bbr3-check`, 25 links (plain, shallow and
deep queues, loss of 1% and 3%, 2 and 500 Mbit/s, a 300 ms round trip,
fq, two flows, beside CUBIC, application-limited, bursts, small writes,
a blackout, a rate change, tail losses, reordering, accepted
connections, loopback at over 90 Gbit/s, 1,200 short connections with `ss`
reading every socket meanwhile, no memory for the state):

- 1,315,509 figures compared, each with its true one: 1,300,903 rate
  samples of new data, 14,434 of retransmissions, 172 lost packets. All
  the same, in flight and lost alike. (31 more weren't known, on the
  links where connections began with no memory for their state: sent
  before there was any.)
- every loss TCP counted (`tp->lost`) was told to the module, in every
  one of 1,229 connections (16,360 losses);
- the tail-probe recoveries it told were the kernel's own count
  (`TCPLossProbeRecovery`: 4 of 4);
- the module unloaded at the end, and the kernel logged nothing.

**It behaves as Google's BBRv3 does.** A kernel built for the guest from
the same source as this computer's (7.2.15) with XanMod's rebase of
Google's patches has the real one; `bbr3-check --compare --reference`
runs both over the same links, each on its own guest. Three runs of
each, with this kernel's CUBIC and BBRv1 beside them (two runs); each
cell is the mean, and what the runs spread:

| Link (rate, round trip, queue) | | Mbit/s | sent again | round trip, mean |
| --- | --- | --- | --- | --- |
| 50 Mbit/s, 40 ms, 170 packets | CUBIC | 47.1 ±0.1 | 0.04% | 68.6 ms |
| | BBRv1 | 46.1 ±0.0 | 0.61% | 45.4 ms |
| | BBRv3, Google's | 45.8 ±0.0 | 0.03% | 43.5 ms |
| | **bbr3, Olo's** | 45.8 ±0.0 | 0.03% | 43.1 ms |
| a shallow queue (20 packets) | CUBIC | 41.5 ±0.0 | 0.13% | 41.2 ms |
| | BBRv1 | 45.3 ±0.0 | 3.83% | 41.2 ms |
| | BBRv3, Google's | 44.2 ±1.3 | 0.71% | 40.5 ms |
| | **bbr3, Olo's** | 44.4 ±1.3 | 0.68% | 40.5 ms |
| a deep queue (1,000 packets) | CUBIC | 47.1 ±0.0 | 0.13% | 212.7 ms |
| | BBRv1 | 46.2 ±0.0 | 0.00% | 47.0 ms |
| | BBRv3, Google's | 46.0 ±0.3 | 0.00% | 43.4 ms |
| | **bbr3, Olo's** | 45.9 ±0.2 | 0.00% | 43.4 ms |
| 1% of packets lost | CUBIC | 6.3 ±0.7 | 0.85% | 41.6 ms |
| | BBRv1 | 45.6 ±0.1 | 1.21% | 43.3 ms |
| | BBRv3, Google's | 37.3 ±2.2 | 1.04% | 41.6 ms |
| | **bbr3, Olo's** | 39.1 ±0.9 | 1.10% | 42.5 ms |
| 3% lost (over BBRv3's 2%) | CUBIC | 2.4 ±0.4 | 2.89% | 44.2 ms |
| | BBRv1 | 44.6 ±0.0 | 3.56% | 43.2 ms |
| | BBRv3, Google's | 24.1 ±2.4 | 3.07% | 40.8 ms |
| | **bbr3, Olo's** | 23.0 ±2.3 | 3.00% | 40.9 ms |
| 2 Mbit/s, 100 ms, 20 packets | CUBIC | 1.90 | 0.58% | 219 ms |
| | BBRv1 | 1.87 | 0.02% | 153 ms |
| | BBRv3, Google's | 1.86 | 0.00% | 129 ms |
| | **bbr3, Olo's** | 1.86 | 0.00% | 127 ms |
| 500 Mbit/s, 10 ms, 400 packets | CUBIC | 461 ±1 | 0.10% | 12.2 ms |
| | BBRv1 | 470 ±0 | 0.42% | 14.1 ms |
| | BBRv3, Google's | 455 ±3 | 0.22% | 12.3 ms |
| | **bbr3, Olo's** | 457 ±6 | 0.26% | 12.1 ms |
| 20 Mbit/s, 300 ms, 500 packets | CUBIC | 17.4 ±0.0 | 0.18% | 608 ms |
| | BBRv1 | 16.8 ±0.0 | 2.78% | 353 ms |
| | BBRv3, Google's | 16.5 ±0.2 | 0.05% | 392 ms |
| | **bbr3, Olo's** | 16.4 ±0.3 | 0.07% | 393 ms |
| beside a CUBIC flow (its share of 50 Mbit/s; the CUBIC flow's) | CUBIC | 22.4 ±1.2; 24.9 | 0.61% | 88.8 ms |
| | BBRv1 | 39.8 ±0.5; 7.2 | 0.54% | 72.3 ms |
| | BBRv3, Google's | 26.8 ±1.7; 20.4 | 0.29% | 75.6 ms |
| | **bbr3, Olo's** | 25.0 ±1.9; 22.3 | 0.36% | 83.2 ms |
| beside CUBIC, deep queue | BBRv1 | 6.3 ±1.5; 41.2 | 0.02% | 234 ms |
| | BBRv3, Google's | 14.0 ±1.2; 33.4 | 0.52% | 244 ms |
| | **bbr3, Olo's** | 13.9 ±1.5; 33.3 | 0.37% | 253 ms |
| beside CUBIC, shallow queue (40) | BBRv1 | 40.5 ±0.2; 6.0 | 1.70% | 43.7 ms |
| | BBRv3, Google's | 26.2 ±0.4; 20.8 | 0.38% | 48.5 ms |
| | **bbr3, Olo's** | 26.7 ±0.4; 20.4 | 0.37% | 49.0 ms |

The module and Google's kernel agree within the runs' spread on every
link (15 in all; the rest: fq, application-limited, a rate change, two
flows). What they do that CUBIC and BBRv1 don't is plain in it: the
queue kept short where CUBIC fills it (a round trip of 43 ms, not 69,
and not 213 where the queue is deep), a fair share beside CUBIC where
BBRv1 takes four fifths, a twentieth of BBRv1's retransmissions, and
most of the link kept under a loss that stops CUBIC.

**What it costs** (`bbr3-check --timing`: the module's own work by the
processor's cycle counter, in the guest):

| | an ACK (of it BBR's own) | an ACK begins | a burst begins | of a core, in all |
| --- | --- | --- | --- | --- |
| 50 Mbit/s, 40 ms: 2,300 ACKs and 3,500 bursts a second | 333 ns (226) | 46 ns | 27 ns | 0.10% |
| 500 Mbit/s, 10 ms: 19,200 and 21,500 | 147 ns (98) | 20 ns | 19 ns | 0.36% |
| loopback, 98 Gbit/s: 68,700 and 465,700 | 86 ns (44) | 15 ns | 18 ns | 1.5% |

A lost packet told takes 160 to 380 ns, a retransmission noted 17 to
75 ns (the probes' own entry not counted), each as often as packets are
lost. Per connection, the state is 232 bytes (152 of them BBR's own)
and its log 384 at first (16 snapshots of 24 bytes).

Nothing runs while a connection is idle (BBR has no timer of its own),
and the two probes are on functions TCP calls only for a lost packet
and for a retransmission.

## A new kernel

The module is built for each kernel from this source, and depends on
TCP's insides more than a congestion control in the kernel's tree does.
What it takes of this kernel, to be looked at again when a kernel
changes any of it (and `tools/dev/bbr3-check` run on the new kernel
before the module is trusted with it; a new kernel series usually means
a new Fedora kernel within two months):

- `struct tcp_congestion_ops`, and when each is called: `min_tso_segs`
  before anything is sent by `tcp_write_xmit` and
  `tcp_xmit_retransmit_queue`; `cong_control` at the end of every ACK
  that changed what's delivered, before what the ACK lets go is sent;
  `in_ack_event` before the ACK's losses are marked; `set_state` before
  a timeout's retransmission.
- `struct rate_sample`: `prior_mstamp`, `prior_delivered`,
  `last_end_seq` (the sample's packet's end), `is_retrans`.
- `tcp_mark_skb_lost(sk, skb)`: every loss goes through it, and it
  counts one in the two branches `bbr3_lost_entry` takes.
- `__tcp_retransmit_skb(sk, skb, segs)`: every retransmission goes
  through it, returns 0 when sent, and leaves `skb` the packet as sent.
- `tcp_process_tlp_ack`: how it leaves `tlp_high_seq` and enters CWR.
- `tcp_skb_cb.tx.delivered`, `tcp_sock`'s `lost`, `delivered`,
  `snd_nxt`, `snd_una`, `app_limited`, `tlp_high_seq`, `tlp_retrans`.

If the build fails, or a probe can't be put in, `olo-bbr3` says why and
connections go on with the congestion control there was.

**The kernel an update brings is checked before it comes.**
`tools/dev/kernel-next` asks Fedora's repositories for the newest
kernel, unpacks its packages in scratch (nothing installed) and runs,
in a guest on that kernel, this module's self-check
(`tools/dev/bbr3-check --on RELEASE --root DIR` is that part alone)
beside everything else of Olo's that touches the kernel. The series the
full self-check has passed on are listed in [CHECKED](CHECKED): 7.2
(7.2.15, where it was made) and 7.3 (7.3.3-300.fc45, Fedora's update of
2026-11: built from this source unchanged, 1,265,738 figures the same as
the true ones over the 25 links, the module unloaded, 2026-11-16). At a
new kernel's first start `olo-bbr3` builds the module for it whatever
its series; where that series isn't in the list yet, Olo Doctor's "TCP
sends with BBRv3" and Preferences ▸ Performance ▸ Kernel say so, until
it has been run there and added.

## The real one to compare with

A kernel with Google's patches, for the guest only (five minutes;
nothing is installed):

    curl -LO https://cdn.kernel.org/pub/linux/kernel/v7.x/linux-7.2.15.tar.xz   # this kernel's version
    tar xf linux-7.2.15.tar.xz && cd linux-7.2.15
    # XanMod's rebase of Google's patches, one file:
    curl -L https://gitlab.com/xanmod/linux-patches/-/raw/master/linux-7.2.y-xanmod/net/tcp/0001-tcp_bbr-v3-update-TCP-bbr-congestion-control-module-.patch | patch -p1
    make defconfig && make kvm_guest.config
    ./scripts/config -e NET_NS -e VETH -e NET_SCHED -e NET_SCH_NETEM -e NET_SCH_TBF -e NET_SCH_FQ \
        -e TCP_CONG_ADVANCED -e TCP_CONG_BBR -e INET_DIAG -e BLK_DEV_INITRD -e DEVTMPFS -e TMPFS \
        -e SERIAL_8250 -e SERIAL_8250_CONSOLE -e MAGIC_SYSRQ -e RD_GZIP -d DEBUG_INFO --set-val HZ 1000 -e HZ_1000
    make olddefconfig && make -j12 bzImage
    tools/dev/bbr3-check --compare --reference arch/x86/boot/bzImage

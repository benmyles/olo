/* SPDX-License-Identifier: GPL-2.0 */
/*
 * A build that checks itself (BBR3_SELFCHECK=1; tools/dev/bbr3-check
 * makes it and runs it in a KVM guest). Never the build that's installed.
 *
 * What Google's kernel records in each packet as it's sent (the packets
 * in flight just after, tcp_set_tx_in_flight(), and tp->lost) is taken
 * here too, where that kernel takes it: a probe on __tcp_transmit_skb()
 * keeps the true figures of every data packet a bbr3 connection sends,
 * by socket and end sequence. Each figure the module then looks up in
 * its log of snapshots (bbr3_olo.h) is compared with them, and
 * /sys/module/tcp_bbr3/parameters/selfcheck says how they compared.
 *
 * fail_alloc=N has every Nth allocation of a connection's state fail,
 * to try what the module does without it. timing=1 times the module's
 * own work instead of checking it (the checks are then off, so that
 * what's timed is what the installed build does), by the processor's
 * cycle counter, and `selfcheck` says each part's count and mean.
 */
#include <linux/hash.h>
#include <linux/kprobes.h>
#include <asm/msr.h>
#include <asm/tsc.h>

struct bbr3_truth {
	const struct sock *sk;
	u32 end_seq;
	u32 in_flight;
	u32 lost;
};

#define BBR3_TRUTH_BITS 16
static struct bbr3_truth bbr3_truths[1 << BBR3_TRUTH_BITS];
static DEFINE_RAW_SPINLOCK(bbr3_truth_lock);

/* By what was looked up: a rate sample's packet or a lost one's, new data
 * or a retransmission.
 */
struct bbr3_tally {
	u64 n;		/* lookups */
	u64 unknown;	/* the log had no answer */
	u64 no_truth;	/* no true figure to compare with (a packet cut after
			 * it was sent has another end, or its slot was reused)
			 */
	u64 exact;	/* in flight: the same */
	u64 off1;	/* within 1 packet */
	u64 off3;	/* within 3 */
	u64 more;
	u64 max;	/* the furthest out */
	u64 lost_same;	/* lost so far: the same */
	u64 lost_wrong;
	u64 lost_max;
};
static struct bbr3_tally bbr3_tallies[2][2];

/* Whole connections: every loss TCP counted (tp->lost) is one the
 * module's probe on tcp_mark_skb_lost() was told of; and the losses
 * repaired by a tail loss probe that it told BBR of (the kernel's own
 * count is TCPLossProbeRecovery in /proc/net/netstat).
 */
static u64 bbr3_conns, bbr3_conns_lost_same, bbr3_lost_told, bbr3_lost_true;
static u64 bbr3_tlp_told;

static int bbr3_fail_alloc_every;
module_param_named(fail_alloc, bbr3_fail_alloc_every, int, 0644);
static atomic_t bbr3_allocs;

static int bbr3_timing;
module_param_named(timing, bbr3_timing, int, 0644);
static u64 bbr3_time_n[BBR3_T_COUNT], bbr3_time_cycles[BBR3_T_COUNT];

static inline u64 bbr3_timed(void)
{
	return READ_ONCE(bbr3_timing) ? rdtsc() : 0;
}

/* (Not exact under more than one flow at a time: no lock taken, to time
 * the work and not the lock.)
 */
static inline void bbr3_time(int what, u64 since)
{
	if (since) {
		bbr3_time_cycles[what] += rdtsc() - since;
		bbr3_time_n[what]++;
	}
}

static inline bool bbr3_fail_alloc(void)
{
	int every = READ_ONCE(bbr3_fail_alloc_every);

	return every > 0 && atomic_inc_return(&bbr3_allocs) % every == 0;
}

static inline struct bbr3_truth *bbr3_truth_slot(const struct sock *sk,
						 u32 end_seq)
{
	return &bbr3_truths[hash_32(hash_ptr(sk, 32) ^ end_seq,
				    BBR3_TRUTH_BITS)];
}

static void bbr3_check(const struct sock *sk, u32 end_seq, bool retrans,
		       bool lost_skb, bool known, const struct bbr3_sent *sent)
{
	struct bbr3_tally *t = &bbr3_tallies[lost_skb][retrans];
	struct bbr3_truth truth, *slot = bbr3_truth_slot(sk, end_seq);
	unsigned long flags;
	u64 d;

	if (READ_ONCE(bbr3_timing))
		return;
	raw_spin_lock_irqsave(&bbr3_truth_lock, flags);
	truth = *slot;
	t->n++;
	if (!known) {
		t->unknown++;
	} else if (truth.sk != sk || truth.end_seq != end_seq) {
		t->no_truth++;
	} else {
		d = abs((s64)sent->in_flight - (s64)truth.in_flight);
		if (!d)
			t->exact++;
		else if (d <= 1)
			t->off1++;
		else if (d <= 3)
			t->off3++;
		else
			t->more++;
		t->max = max(t->max, d);
		d = abs((s64)sent->lost - (s64)truth.lost);
		if (!d)
			t->lost_same++;
		else
			t->lost_wrong++;
		t->lost_max = max(t->lost_max, d);
	}
	raw_spin_unlock_irqrestore(&bbr3_truth_lock, flags);
}

static void bbr3_check_began(const struct sock *sk)
{
	bbr3_sk(sk)->check_lost = tcp_sk(sk)->lost;
}

static void bbr3_check_lost(const struct sock *sk, u32 pcount)
{
	bbr3_sk(sk)->check_lost += pcount;
}

static void bbr3_check_tlp(void)
{
	unsigned long flags;

	raw_spin_lock_irqsave(&bbr3_truth_lock, flags);
	bbr3_tlp_told++;
	raw_spin_unlock_irqrestore(&bbr3_truth_lock, flags);
}

static void bbr3_check_ended(const struct sock *sk)
{
	u32 told = bbr3_sk(sk)->check_lost, lost = tcp_sk(sk)->lost;
	unsigned long flags;

	raw_spin_lock_irqsave(&bbr3_truth_lock, flags);
	bbr3_conns++;
	bbr3_conns_lost_same += told == lost;
	bbr3_lost_told += told;
	bbr3_lost_true += lost;
	raw_spin_unlock_irqrestore(&bbr3_truth_lock, flags);
}

/* What __tcp_transmit_skb() is about to send, as Google's kernel records
 * it at that function's end (nothing it counts changes in between); kept
 * if the sending went through, as there.
 */
struct bbr3_tx_call {
	struct bbr3_truth truth;
	u32 seq, mss;
};

static int bbr3_tx_entry(struct kretprobe_instance *ri, struct pt_regs *regs)
{
	struct sock *sk = (struct sock *)regs_get_kernel_argument(regs, 0);
	const struct sk_buff *skb =
		(const struct sk_buff *)regs_get_kernel_argument(regs, 1);
	struct bbr3_tx_call *call = (struct bbr3_tx_call *)ri->data;
	const struct tcp_sock *tp = tcp_sk(sk);

	if (inet_csk(sk)->icsk_ca_ops != &tcp_bbr3_cong_ops ||
	    TCP_SKB_CB(skb)->end_seq == TCP_SKB_CB(skb)->seq ||
	    READ_ONCE(bbr3_timing))
		return 1;	/* not ours, or no data: no return probe */
	call->truth.sk = sk;
	call->truth.end_seq = TCP_SKB_CB(skb)->end_seq;
	call->truth.in_flight = tcp_packets_in_flight(tp) + tcp_skb_pcount(skb);
	call->truth.lost = tp->lost;
	call->seq = TCP_SKB_CB(skb)->seq;
	call->mss = tcp_skb_mss(skb);
	return 0;
}

static int bbr3_tx_return(struct kretprobe_instance *ri, struct pt_regs *regs)
{
	struct bbr3_tx_call *call = (struct bbr3_tx_call *)ri->data;
	unsigned long flags;

	if (regs_return_value(regs))
		return 0;	/* not sent */
	raw_spin_lock_irqsave(&bbr3_truth_lock, flags);
	*bbr3_truth_slot(call->truth.sk, call->truth.end_seq) = call->truth;
	/* And of each part it may be cut into later, at a segment's end:
	 * Google's tcp_fragment() gives the first part the in-flight count
	 * "as if 1st were sent by itself".
	 */
	while (call->mss && call->truth.in_flight > 1 &&
	       after(call->truth.end_seq - call->mss, call->seq)) {
		call->truth.end_seq -= call->mss;
		call->truth.in_flight--;
		*bbr3_truth_slot(call->truth.sk, call->truth.end_seq) =
			call->truth;
	}
	raw_spin_unlock_irqrestore(&bbr3_truth_lock, flags);
	return 0;
}

static struct kretprobe bbr3_tx_probe = {
	.kp.symbol_name	= "__tcp_transmit_skb",
	.entry_handler	= bbr3_tx_entry,
	.handler	= bbr3_tx_return,
	.data_size	= sizeof(struct bbr3_tx_call),
	.maxactive	= 256,
};

static int bbr3_selfcheck_show(char *buf, const struct kernel_param *kp)
{
	static const char * const what[2][2] = {
		{ "sample new", "sample retransmitted" },
		{ "lost new", "lost retransmitted" },
	};
	unsigned long flags;
	int i, j, len = 0;

	raw_spin_lock_irqsave(&bbr3_truth_lock, flags);
	for (i = 0; i < 2; i++) {
		for (j = 0; j < 2; j++) {
			const struct bbr3_tally *t = &bbr3_tallies[i][j];

			len += sysfs_emit_at(buf, len,
				"%s: lookups %llu unknown %llu no_truth %llu in_flight exact %llu within1 %llu within3 %llu more %llu max %llu lost same %llu wrong %llu max %llu\n",
				what[i][j], t->n, t->unknown, t->no_truth,
				t->exact, t->off1, t->off3, t->more, t->max,
				t->lost_same, t->lost_wrong, t->lost_max);
		}
	}
	len += sysfs_emit_at(buf, len,
		"connections %llu with_every_loss_told %llu lost_told %llu lost_true %llu tlp_recoveries_told %llu\n",
		bbr3_conns, bbr3_conns_lost_same, bbr3_lost_told,
		bbr3_lost_true, bbr3_tlp_told);
	raw_spin_unlock_irqrestore(&bbr3_truth_lock, flags);
	len += sysfs_emit_at(buf, len, "missed sends %d\n",
			     bbr3_tx_probe.nmissed);
	for (i = 0; i < BBR3_T_COUNT; i++) {
		static const char * const part[BBR3_T_COUNT] = {
			"ack", "ack_bbr_main", "ack_begins", "burst", "lost",
			"retransmission",
		};
		u64 n = bbr3_time_n[i];

		if (n)
			len += sysfs_emit_at(buf, len,
				"time %s: %llu calls, %llu ns each\n", part[i], n,
				div64_u64(div64_u64(bbr3_time_cycles[i] * 1000000ULL,
						    tsc_khz), n));
	}
	return len;
}

/* Anything written: the counts begin again. */
static int bbr3_selfcheck_reset(const char *val, const struct kernel_param *kp)
{
	unsigned long flags;

	raw_spin_lock_irqsave(&bbr3_truth_lock, flags);
	memset(bbr3_tallies, 0, sizeof(bbr3_tallies));
	bbr3_conns = bbr3_conns_lost_same = bbr3_lost_told = 0;
	bbr3_lost_true = bbr3_tlp_told = 0;
	memset(bbr3_time_n, 0, sizeof(bbr3_time_n));
	memset(bbr3_time_cycles, 0, sizeof(bbr3_time_cycles));
	raw_spin_unlock_irqrestore(&bbr3_truth_lock, flags);
	return 0;
}

static const struct kernel_param_ops bbr3_selfcheck_ops = {
	.get = bbr3_selfcheck_show,
	.set = bbr3_selfcheck_reset,
};
module_param_cb(selfcheck, &bbr3_selfcheck_ops, NULL, 0644);

static int bbr3_selfcheck_start(void)
{
	int ret = register_kretprobe(&bbr3_tx_probe);

	if (ret < 0)
		pr_err("tcp_bbr3: self-check: __tcp_transmit_skb can't be probed (%d)\n", ret);
	return ret;
}

static void bbr3_selfcheck_stop(void)
{
	unregister_kretprobe(&bbr3_tx_probe);
}

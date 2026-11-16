/* SPDX-License-Identifier: GPL-2.0 */
/*
 * What BBRv3 needs of TCP that this kernel doesn't give a congestion
 * control, made here (Olo's; kernel/bbr3/README.md).
 *
 * Google's BBRv3 comes with patches to TCP itself: each packet records,
 * as it's sent, how many packets were in flight and how many had been
 * lost so far (tx.in_flight, tx.lost in its control block), and a rate
 * sample carries those of the packet it was taken from, with the losses
 * since. TCP is built into the kernel, so those patches mean another
 * kernel. This module runs on the kernel as it is: the same two figures
 * are looked up, for any packet, in a log of snapshots the module takes
 * each time TCP calls it with nothing sent since (before each burst of
 * sending: min_tso_segs; after each ACK: cong_control; at a timeout:
 * set_state), so that how things stood at a snapshot is how they stood
 * when the next packet went. New data is found by where it lies in the
 * sequence space (a packet's in-flight count is its snapshot's, plus the
 * packets sent from the snapshot to its own end); a retransmission by
 * the delivered count it was sent at (tx.delivered, which the kernel
 * does keep) and its time.
 */
#ifndef _OLO_BBR3_H
#define _OLO_BBR3_H

/* The kernel's rate sample as BBRv3 reads it, with what Google's
 * patches add to it (tcp_bbr3.c: bbr3_sample()).
 */
struct bbr_rate_sample {
	/* The kernel's own (struct rate_sample), as it gave them: */
	u32  prior_delivered;	/* tp->delivered when the sample's packet was sent */
	s32  delivered;		/* number of packets delivered over interval */
	s32  delivered_ce;	/* packets delivered w/ CE mark over interval */
	long interval_us;	/* time for tp->delivered to incr "delivered" */
	long rtt_us;		/* RTT of last (S)ACKed packet (or -1) */
	int  losses;		/* number of packets marked lost upon ACK */
	u32  acked_sacked;	/* number of packets newly (S)ACKed upon ACK */
	u32  prior_in_flight;	/* in flight before this ACK */
	bool is_app_limited;	/* is sample from packet with bubble in pipe? */
	bool is_ack_delayed;	/* is this (likely) a delayed ACK? */
	/* Made here: */
	u32  tx_in_flight;	/* packets in flight when the sample's packet was sent */
	s32  lost;		/* number of packets lost over interval */
	bool is_ece;		/* did this ACK have ECN marked? (never: no ECN here) */
	bool is_acking_tlp_retrans_seq;  /* ACKed a TLP retransmit sequence? */
};

/* How things stood when something was about to be sent. */
struct bbr3_snap {
	u32 snd_nxt;		/* the next new byte to go */
	u32 delivered;		/* tp->delivered: what a packet sent from here records */
	u32 lost;		/* tp->lost */
	u32 stamp;		/* tp->tcp_mstamp, its low 32 bits (usec) */
	u32 in_flight:31,	/* tcp_packets_in_flight() */
	    app_limited:1;	/* tp->app_limited was set: packets sent from here are */
};

#define BBR3_LOG_MIN	16
#define BBR3_LOG_MAX	2048	/* 40 KB: a flight of more bursts than this loses its oldest */

struct bbr3_tx {
	struct bbr3_snap *log;	/* a ring, oldest first; none until one is needed */
	u32 cap;		/* its size, a power of two (0: none) */
	u32 head;		/* the oldest's place in it */
	u32 len;
	struct bbr3_snap now;	/* the latest snapshot: the log's next once something is sent after it */
	u32 now_retrans;	/* tp->total_retrans at `now` */
	u32 tlp_high_seq;	/* tp->tlp_high_seq as the ACK being processed found it */
	u32 tlp_stamp;		/* that ACK's time (tp->tcp_mstamp, low 32 bits) */
	u8  have_now:1,
	    tlp_retrans:1,	/* that TLP probe was a retransmission */
	    tlp_app_limited:1;	/* the data it repaired was sent app-limited */
};

static inline struct bbr3_snap *bbr3_at(const struct bbr3_tx *tx, u32 i)
{
	return &tx->log[(tx->head + i) & (tx->cap - 1)];
}

/* The i-th snapshot there is, oldest first: the log's, then `now`. */
static inline const struct bbr3_snap *bbr3_snap_at(const struct bbr3_tx *tx,
						   u32 i)
{
	return i < tx->len ? bbr3_at(tx, i) : &tx->now;
}

static inline u32 bbr3_snaps(const struct bbr3_tx *tx)
{
	return tx->len + tx->have_now;
}

static void bbr3_log_push(struct bbr3_tx *tx, const struct bbr3_snap *snap)
{
	if (tx->len == tx->cap) {
		u32 cap = tx->cap ? tx->cap * 2 : BBR3_LOG_MIN;
		struct bbr3_snap *log = NULL;
		u32 i;

		if (cap <= BBR3_LOG_MAX)
			log = kmalloc_array(cap, sizeof(*log),
					    GFP_ATOMIC | __GFP_NOWARN);
		if (log) {
			for (i = 0; i < tx->len; i++)
				log[i] = *bbr3_at(tx, i);
			kfree(tx->log);
			tx->log = log;
			tx->cap = cap;
			tx->head = 0;
		} else if (!tx->cap) {
			return;	/* no log at all: nothing is known, which BBR takes */
		} else {
			/* As long as it can be: the oldest goes (what was
			 * sent from it is then not known).
			 */
			tx->head = (tx->head + 1) & (tx->cap - 1);
			tx->len--;
		}
	}
	*bbr3_at(tx, tx->len++) = *snap;
}

/* Take a snapshot. Called wherever TCP calls the module before it sends:
 * if anything was sent since the last one, that one is kept, for what
 * was sent from it.
 */
static void bbr3_tx_note(const struct sock *sk, struct bbr3_tx *tx)
{
	const struct tcp_sock *tp = tcp_sk(sk);

	if (tx->have_now && (tp->snd_nxt != tx->now.snd_nxt ||
			     tp->total_retrans != tx->now_retrans))
		bbr3_log_push(tx, &tx->now);
	tx->now.snd_nxt = tp->snd_nxt;
	tx->now.delivered = tp->delivered;
	tx->now.lost = tp->lost;
	tx->now.stamp = (u32)tp->tcp_mstamp;
	tx->now.in_flight = tcp_packets_in_flight(tp);
	tx->now.app_limited = !!tp->app_limited;
	tx->now_retrans = tp->total_retrans;
	tx->have_now = 1;
}

/* Let go of the snapshots nothing outstanding was sent from: one is done
 * with when the next was taken at or below snd_una. (After an ACK's
 * sample has been looked up, never before: the sample's packet is one
 * the ACK has just taken off the queue.)
 */
static void bbr3_tx_trim(const struct sock *sk, struct bbr3_tx *tx)
{
	u32 una = tcp_sk(sk)->snd_una;

	while (tx->len && !after(bbr3_snap_at(tx, 1)->snd_nxt, una)) {
		if (tx->len == 1 && !tx->have_now)
			break;
		tx->head = (tx->head + 1) & (tx->cap - 1);
		tx->len--;
	}
}

/* The snapshot new data ending at end_seq was sent from: the last one
 * taken below its end. NULL if there is none (older than the log).
 */
static const struct bbr3_snap *bbr3_tx_of_new(const struct bbr3_tx *tx,
					      u32 end_seq)
{
	u32 lo = 0, hi = bbr3_snaps(tx);

	/* The first that is not below end_seq; the one before it. */
	while (lo < hi) {
		u32 mid = lo + (hi - lo) / 2;

		if (before(bbr3_snap_at(tx, mid)->snd_nxt, end_seq))
			lo = mid + 1;
		else
			hi = mid;
	}
	return lo ? bbr3_snap_at(tx, lo - 1) : NULL;
}

/* The snapshot a packet was (re)sent from, by what it recorded then: the
 * delivered count (tx.delivered) and its time. The last snapshot with
 * that count or less; of several with the same (bursts between two
 * ACKs), the last taken by the packet's time.
 */
static const struct bbr3_snap *bbr3_tx_of_sent(const struct bbr3_tx *tx,
					       u32 delivered, u32 stamp)
{
	u32 lo = 0, hi = bbr3_snaps(tx);
	const struct bbr3_snap *snap;

	while (lo < hi) {
		u32 mid = lo + (hi - lo) / 2;

		if (!after(bbr3_snap_at(tx, mid)->delivered, delivered))
			lo = mid + 1;
		else
			hi = mid;
	}
	if (!lo)
		return NULL;
	snap = bbr3_snap_at(tx, --lo);
	while (lo && (s32)(snap->stamp - stamp) > 0 &&
	       bbr3_snap_at(tx, lo - 1)->delivered == snap->delivered)
		snap = bbr3_snap_at(tx, --lo);
	return snap;
}

/* What a packet had recorded in Google's kernel: the packets in flight
 * just after it was sent, and the packets lost by then.
 */
struct bbr3_sent {
	u32 in_flight;
	u32 lost;
	bool app_limited;
};

/* Of a packet ending at end_seq, of `pcount` segments of `mss` bytes,
 * retransmitted or not, sent at tx.delivered `delivered` and time
 * `stamp`. False if it isn't known.
 */
static bool bbr3_tx_lookup(const struct bbr3_tx *tx, u32 end_seq, u32 pcount,
			   u32 mss, bool retrans, u32 delivered, u32 stamp,
			   struct bbr3_sent *sent)
{
	const struct bbr3_snap *snap = NULL;

	if (!retrans) {
		snap = bbr3_tx_of_new(tx, end_seq);
		/* (Sent from it: then at its delivered count. If not, this
		 * isn't its snapshot: the log lost it.)
		 */
		if (snap && snap->delivered == delivered) {
			sent->in_flight = snap->in_flight +
				DIV_ROUND_UP(end_seq - snap->snd_nxt,
					     max(mss, 1U));
			sent->lost = snap->lost;
			sent->app_limited = snap->app_limited;
			return true;
		}
	}
	snap = bbr3_tx_of_sent(tx, delivered, stamp);
	if (!snap || snap->delivered != delivered)
		return false;
	sent->in_flight = snap->in_flight + pcount;
	sent->lost = snap->lost;
	sent->app_limited = snap->app_limited;
	return true;
}

static void bbr3_tx_free(struct bbr3_tx *tx)
{
	kfree(tx->log);
	tx->log = NULL;
	tx->cap = tx->head = tx->len = 0;
	tx->have_now = 0;
}

#endif /* _OLO_BBR3_H */

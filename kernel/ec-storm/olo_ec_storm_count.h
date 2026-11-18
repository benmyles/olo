/* SPDX-License-Identifier: GPL-2.0 */
/*
 * olo_ec_storm's counting: of one event arriving over and over, one runs
 * its method and the rest are counted for it to run again for, up to
 * OWED. In a header of its own so that check.c runs these very lines
 * from many threads at once (the kernel's workers), outside any kernel.
 *
 * Whoever includes it has atomic_t and atomic_long_t with the kernel's
 * names for what is done to them, and olo_ec_storm_run(guard).
 */
#ifndef OLO_EC_STORM_COUNT_H
#define OLO_EC_STORM_COUNT_H

/*
 * Runs of an event's method owed beside the one running: enough that no
 * key pressed is lost (its method takes a millisecond or two; a key
 * repeats thirty times a second at most), few enough that a flood's
 * leftovers are over in seconds.
 */
#define OLO_EC_STORM_OWED 8

struct olo_ec_storm_count {
	/* Events not yet handled: the one running, and those it owes. */
	atomic_t waiting;
	/* The method's runs, and the events only counted. */
	atomic_long_t ran;
	atomic_long_t counted;
};

/* An event came: `g`'s method is run for it, here or by the one running. */
static inline void olo_ec_storm_event(struct olo_ec_storm_count *c, struct guard *g)
{
	/* One of this event's is running: it runs for this one too. */
	if (atomic_inc_return(&c->waiting) > 1) {
		/* (Past what's owed already: the same news again.) */
		if (atomic_read(&c->waiting) > 1 + OLO_EC_STORM_OWED)
			atomic_dec(&c->waiting);
		atomic_long_inc(&c->counted);
		return;
	}
	do {
		olo_ec_storm_run(g);
		atomic_long_inc(&c->ran);
	} while (atomic_dec_return(&c->waiting) > 0);
}

#endif

// SPDX-License-Identifier: GPL-2.0
/*
 * olo_ec_storm's counting, run outside the kernel: the very lines of
 * olo_ec_storm_count.h, from many threads at once, as the kernel's
 * workers run them.
 *
 *   cc -O2 -pthread -o /var/tmp/ec-storm-check kernel/ec-storm/check.c && /var/tmp/ec-storm-check
 *
 * What must hold, and is checked: the method never runs twice at once
 * for one event; every event is followed by a run that began after it
 * came (so what it has to say is read), unless more than OWED were
 * already waiting; a flood of events costs far fewer runs than events;
 * events that come one at a time each get their own run; and when all
 * is over nothing is left waiting.
 */
#include <pthread.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include <unistd.h>

typedef struct { atomic_int v; } atomic_t;
typedef struct { atomic_long v; } atomic_long_t;
static inline int atomic_inc_return(atomic_t *a) { return atomic_fetch_add(&a->v, 1) + 1; }
static inline int atomic_dec_return(atomic_t *a) { return atomic_fetch_sub(&a->v, 1) - 1; }
static inline void atomic_dec(atomic_t *a) { atomic_fetch_sub(&a->v, 1); }
static inline int atomic_read(atomic_t *a) { return atomic_load(&a->v); }
static inline void atomic_long_inc(atomic_long_t *a) { atomic_fetch_add(&a->v, 1); }
static inline long atomic_long_read(atomic_long_t *a) { return atomic_load(&a->v); }

struct guard;
static void olo_ec_storm_run(struct guard *g);
#include "olo_ec_storm_count.h"

struct guard {
	struct olo_ec_storm_count count;
	/* How long the method takes (microseconds). */
	int takes;
	atomic_int running;
	atomic_int overlapped;
	/* Counts the runs begun: an event reads it as it comes. */
	atomic_long begun;
	/* The greatest "runs begun" any event saw as it came. */
	atomic_long last_seen;
};

static void olo_ec_storm_run(struct guard *g)
{
	if (atomic_fetch_add(&g->running, 1) != 0)
		atomic_store(&g->overlapped, 1);
	atomic_fetch_add(&g->begun, 1);
	if (g->takes)
		usleep(g->takes);
	atomic_fetch_sub(&g->running, 1);
}

struct flood {
	struct guard *g;
	int events;
	int every;
};

static void *flood(void *arg)
{
	struct flood *f = arg;

	for (int i = 0; i < f->events; i++) {
		long seen = atomic_load(&f->g->begun), was = atomic_load(&f->g->last_seen);

		while (seen > was && !atomic_compare_exchange_weak(&f->g->last_seen, &was, seen))
			;
		olo_ec_storm_event(&f->g->count, f->g);
		if (f->every)
			usleep(f->every);
	}
	return NULL;
}

static int failed;
#define CHECK(what, ...) do { if (!(what)) { failed = 1; printf("FAILED: " __VA_ARGS__); printf("\n"); } } while (0)

int main(void)
{
	/* A flood: sixteen workers, an event every 100 microseconds each, a method of 2 ms. */
	struct guard g = { .takes = 2000 };
	pthread_t t[16];
	struct flood f = { &g, 2000, 100 };

	for (int i = 0; i < 16; i++)
		pthread_create(&t[i], NULL, flood, &f);
	for (int i = 0; i < 16; i++)
		pthread_join(t[i], NULL);
	long events = 16 * 2000, ran = atomic_long_read(&g.count.ran), counted = atomic_long_read(&g.count.counted);
	printf("a flood: %ld events, the method run %ld times, %ld only counted\n", events, ran, counted);
	CHECK(!atomic_load(&g.overlapped), "the method ran twice at once");
	CHECK(atomic_read(&g.count.waiting) == 0, "%d left waiting", atomic_read(&g.count.waiting));
	CHECK(ran + counted >= events && ran < events / 4, "runs %ld, counted %ld, of %ld", ran, counted, events);
	/* The last event to come is followed by a run begun after it came. */
	CHECK(atomic_load(&g.begun) > atomic_load(&g.last_seen), "no run after the last event (begun %ld, last seen %ld)", atomic_load(&g.begun), atomic_load(&g.last_seen));

	/* One at a time (a key pressed, then another): each its own run, none only counted. */
	struct guard keys = { .takes = 0 };
	struct flood k = { &keys, 5000, 0 };
	flood(&k);
	printf("one at a time: %d events, the method run %ld times, %ld only counted\n", k.events, atomic_long_read(&keys.count.ran), atomic_long_read(&keys.count.counted));
	CHECK(atomic_long_read(&keys.count.ran) == k.events && atomic_long_read(&keys.count.counted) == 0, "a key's press was lost or counted");

	/* A burst while one runs (a key pressed five times in the 10 ms its method takes): all five run. */
	struct guard burst = { .takes = 10000 };
	struct flood b = { &burst, 1, 0 };
	pthread_t first, rest[4];
	pthread_create(&first, NULL, flood, &b);
	usleep(2000);
	for (int i = 0; i < 4; i++)
		pthread_create(&rest[i], NULL, flood, &b);
	pthread_join(first, NULL);
	for (int i = 0; i < 4; i++)
		pthread_join(rest[i], NULL);
	printf("a burst of five: the method run %ld times\n", atomic_long_read(&burst.count.ran));
	CHECK(atomic_long_read(&burst.count.ran) == 5 && atomic_read(&burst.count.waiting) == 0, "a burst within what's owed lost a run");

	/* More than is owed while one runs: the one running, and what's owed, no more. */
	struct guard many = { .takes = 20000 };
	struct flood m = { &many, 1, 0 };
	pthread_t one, more[40];
	pthread_create(&one, NULL, flood, &m);
	usleep(3000);
	for (int i = 0; i < 40; i++)
		pthread_create(&more[i], NULL, flood, &m);
	pthread_join(one, NULL);
	for (int i = 0; i < 40; i++)
		pthread_join(more[i], NULL);
	long r = atomic_long_read(&many.count.ran);
	printf("forty-one at once: the method run %ld times\n", r);
	CHECK(r >= 2 && r <= 1 + OLO_EC_STORM_OWED + 1 && atomic_read(&many.count.waiting) == 0, "runs %ld for forty-one at once", r);

	printf(failed ? "FAILED\n" : "all held\n");
	return failed;
}

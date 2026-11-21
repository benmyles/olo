// SPDX-License-Identifier: GPL-2.0
/*
 * For a guest only (tools/dev/gpu-pages-check): free memory left in small
 * pieces, for certain. One page of every free 2 MB piece is kept and the
 * rest given back, so no 2 MB piece is free and each can be made again
 * only by moving nothing (the kept page is the kernel's own, not movable):
 * what a computer that has run for days has of memory that can't be
 * moved. Unloading gives the pages back.
 *
 * (Half of memory held in every other page by a program, as `kbench
 * fragment` leaves it, still left 85 pieces of 4 MB free in one guest's
 * low zone: the pool was given every 2 MB piece it asked for without
 * waiting once.)
 */

#include <linux/gfp.h>
#include <linux/mm.h>
#include <linux/module.h>
#include <linux/sched.h>

#define ORDER	9

static struct page *kept[4096];
static unsigned int count;

static int __init fragment_init(void)
{
	while (count < ARRAY_SIZE(kept)) {
		struct page *p = alloc_pages((GFP_HIGHUSER & ~__GFP_RECLAIM) | __GFP_NOMEMALLOC | __GFP_NOWARN, ORDER);
		int i;

		if (!p)
			break;
		split_page(p, ORDER);
		kept[count++] = p;
		for (i = 1; i < 1 << ORDER; i++)
			__free_page(p + i);
		cond_resched();
	}
	pr_info("olo_gpu_pages_fragment: one page kept of each of %u free 2 MB pieces\n", count);
	return 0;
}

static void __exit fragment_exit(void)
{
	while (count)
		__free_page(kept[--count]);
}

module_init(fragment_init);
module_exit(fragment_exit);
MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("Olo, for a test guest: free memory left in small pieces");

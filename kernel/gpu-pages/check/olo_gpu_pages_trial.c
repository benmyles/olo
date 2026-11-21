// SPDX-License-Identifier: GPL-2.0
/*
 * For a guest only (tools/dev/gpu-pages-check): GPU buffers filled from a
 * pool like xe's, with no GPU. A pool with xe's flags (2 MB named as the
 * size worth having, pages from the page allocator), owned by a made-up
 * PCI device whose driver has the name given (`driver=xe`, or another to
 * see that another driver's pool is left alone), filled `fills` times
 * with `pages` pages each, `keep` of them held at once. While it fills,
 * the kernel's own tracepoints are watched on this thread: each size
 * asked of the page allocator with its flags, and each time the thread
 * went compacting or reclaiming for it.
 *
 * It says, in the kernel's log: how long a fill took (the middle, 99 in
 * 100, the longest); then for each size, how often it was asked for, how
 * often with waiting allowed, how often refused, how many pieces of it
 * the buffers got, how often the thread compacted and reclaimed for it,
 * and the flags last used. Loading it is the trial; it holds nothing
 * after.
 */

#include <linux/ktime.h>
#include <linux/module.h>
#include <linux/pci.h>
#include <linux/slab.h>
#include <linux/sort.h>
#include <linux/tracepoint.h>
#include <drm/ttm/ttm_allocation.h>
#include <drm/ttm/ttm_bo.h>
#include <drm/ttm/ttm_pool.h>
#include <drm/ttm/ttm_tt.h>
#include <trace/events/kmem.h>

static unsigned int pages = 512, fills = 64, keep = 64;
static bool observe = true;
module_param(observe, bool, 0444);
static char *bdf;
module_param(bdf, charp, 0444);
static unsigned long large_zone[MAX_NR_ZONES];
static char *driver = "xe";
module_param(pages, uint, 0444);
module_param(fills, uint, 0444);
module_param(keep, uint, 0444);
module_param(driver, charp, 0444);

/* Only the two things a pool's owner is asked: its bus and its driver's name. */
static struct device_driver made_up_driver;
static struct device made_up_device = { .bus = &pci_bus_type, .driver = &made_up_driver };

static struct ttm_pool pool;
static struct task_struct *watched;

static struct size {
	unsigned long asked, waiting, refused, pieces, compacted, reclaimed;
	gfp_t flags;
} sizes[NR_PAGE_ORDERS];

static void saw_allocation(void *data, struct page *page, unsigned int order, gfp_t gfp, int migratetype)
{
	if (current != READ_ONCE(watched) || order >= NR_PAGE_ORDERS)
		return;
	if (!in_task())
		return;
	if (order == 9 && page)
		large_zone[page_zonenum(page)]++;
	sizes[order].asked++;
	sizes[order].waiting += !!(gfp & __GFP_DIRECT_RECLAIM);
	sizes[order].refused += !page;
	sizes[order].flags = gfp;
}

static void saw_compaction(void *data, int order, gfp_t gfp, int priority)
{
	if (current == READ_ONCE(watched) && in_task() && (unsigned int)order < NR_PAGE_ORDERS)
		sizes[order].compacted++;
}

static void saw_reclaim(void *data, gfp_t gfp, int order, struct mem_cgroup *memcg)
{
	if (current == READ_ONCE(watched) && in_task() && (unsigned int)order < NR_PAGE_ORDERS)
		sizes[order].reclaimed++;
}

static struct {
	const char *name;
	void *saw;
	struct tracepoint *tp;
	bool watching;
} watches[] = {
	{ "mm_page_alloc", saw_allocation },
	{ "mm_compaction_try_to_compact_pages", saw_compaction },
	{ "mm_vmscan_direct_reclaim_begin", saw_reclaim },
};

static void find(struct tracepoint *tp, void *unused)
{
	int i;

	for (i = 0; i < ARRAY_SIZE(watches); i++)
		if (!strcmp(tp->name, watches[i].name))
			watches[i].tp = tp;
}

static int by_time(const void *a, const void *b)
{
	u64 x = *(const u64 *)a, y = *(const u64 *)b;

	return (x > y) - (x < y);
}

static int __init trial_init(void)
{
	struct ttm_operation_ctx ctx = { };
	struct ttm_tt tt = { .num_pages = pages, .caching = ttm_cached };
	struct device *pci_target = NULL;
	struct page **all;
	u64 *took;
	unsigned int i, j, n, held = 0;
	int ret = 0;

	/*
	 * (The first callback's shape against the kernel's own declaration.
	 * The other two tracepoints' headers want the memory manager's
	 * private types and can't be read from a module: theirs are as
	 * Linux 7.3 has them, mm_compaction_try_to_compact_pages(order,
	 * gfp_mask, prio) and mm_vmscan_direct_reclaim_begin(gfp_flags,
	 * order, memcg). A guest's to get wrong.)
	 */
	check_trace_callback_type_mm_page_alloc(saw_allocation);
	if (!pages || !fills || !keep || keep > fills || pages > 32768 || fills > 100000)
		return -EINVAL;
	all = kvcalloc(array_size(pages, keep), sizeof(*all), GFP_KERNEL);
	took = kvcalloc(fills, sizeof(*took), GFP_KERNEL);
	if (!all || !took) {
		ret = -ENOMEM;
		goto out;
	}
	made_up_driver.name = driver;
	if (bdf) {
		pci_target = bus_find_device_by_name(&pci_bus_type, NULL, bdf);
		if (!pci_target) {
			ret = -ENODEV;
			goto out;
		}
	}
	ttm_pool_init(&pool, pci_target ?: &made_up_device, NUMA_NO_NODE, TTM_ALLOCATION_POOL_BENEFICIAL_ORDER(9));
	for_each_kernel_tracepoint(find, NULL);
	for (i = 0; observe && i < ARRAY_SIZE(watches); i++) {
		ret = watches[i].tp ? tracepoint_probe_register(watches[i].tp, watches[i].saw, NULL) : -ENOENT;
		if (ret)
			goto done;
		watches[i].watching = true;
	}
	for (n = 0; n < fills; n++) {
		u64 began;

		tt.pages = all + held * pages;
		WRITE_ONCE(watched, current);
		began = ktime_get_ns();
		ret = ttm_pool_alloc(&pool, &tt, &ctx);
		took[n] = ktime_get_ns() - began;
		WRITE_ONCE(watched, NULL);
		if (ret)
			break;
		held++;
		/* What it was given: each piece's first page says its size, and its pages follow one another. */
		for (i = 0; i < pages;) {
			unsigned long order = tt.pages[i]->private;

			if (order >= NR_PAGE_ORDERS || (1UL << order) > pages - i) {
				ret = -EINVAL;
				break;
			}
			for (j = 1; j < 1UL << order; j++)
				if (tt.pages[i + j] != tt.pages[i] + j)
					ret = -EINVAL;
			sizes[order].pieces++;
			i += 1UL << order;
		}
		if (ret)
			break;
		if (held == keep)
			while (held) {
				tt.pages = all + --held * pages;
				ttm_pool_free(&pool, &tt);
			}
		cond_resched();
	}
	while (held) {
		tt.pages = all + --held * pages;
		ttm_pool_free(&pool, &tt);
	}
	if (ret) {
		pr_info("olo_gpu_pages_trial: failed=%d after=%u\n", ret, n);
	} else {
		sort(took, fills, sizeof(*took), by_time, NULL);
		pr_info("olo_gpu_pages_trial: driver=%s pages=%u fills=%u keep=%u middle_ns=%llu p99_ns=%llu longest_ns=%llu\n", driver, pages, fills, keep,
			took[fills / 2], took[DIV_ROUND_UP(fills * 99, 100) - 1], took[fills - 1]);
	}
	for (i = 0; i < ARRAY_SIZE(sizes); i++) {
		struct size *s = sizes + i;

		if (s->asked || s->pieces || s->compacted || s->reclaimed)
			pr_info("olo_gpu_pages_trial: order=%u asked=%lu waiting=%lu refused=%lu pieces=%lu compacted=%lu reclaimed=%lu flags=%#x\n", i, s->asked,
				s->waiting, s->refused, s->pieces, s->compacted, s->reclaimed, s->flags);
	}
done:
	for (i = 0; i < ARRAY_SIZE(watches); i++)
		if (watches[i].watching)
			tracepoint_probe_unregister(watches[i].tp, watches[i].saw, NULL);
	tracepoint_synchronize_unregister();
	ttm_pool_fini(&pool);
	for (i = 0; i < MAX_NR_ZONES; i++)
		if (large_zone[i])
			pr_info("large zone index=%u count=%lu\n", i, large_zone[i]);
	put_device(pci_target);
out:
	kvfree(took);
	kvfree(all);
	return ret;
}

static void __exit trial_exit(void)
{
}

module_init(trial_init);
module_exit(trial_exit);
MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("Olo, for a test guest: GPU buffers filled from a pool like xe's");

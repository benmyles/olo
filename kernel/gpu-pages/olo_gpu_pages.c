// SPDX-License-Identifier: GPL-2.0
/*
 * olo_gpu_pages: no program is held for a 2 MB piece of graphics memory.
 *
 * TTM, the memory manager under Intel's xe graphics driver, fills each
 * GPU buffer with the largest pieces it can get. For the size the driver
 * names as worth having (xe: 2 MB) Linux 7.3 has it try hard
 * (ttm_pool_alloc_page(): __GFP_RETRY_MAYFAIL, with direct reclaim): where
 * no 2 MB piece is free it compacts memory and reclaims, on the thread
 * that made the buffer. Those threads draw. On a computer whose free
 * memory was in small pieces, apps' drawing threads and a browser's GPU
 * process were held a third to half of a second at a time, a compositor's
 * loop up to 8 ms (kernel/TUNING.md, "Graphics memory in 2 MB pieces").
 *
 * With this, that one try is made without waiting: a free 2 MB piece is
 * taken, the kernel's own threads may be woken to make more
 * (__GFP_KSWAPD_RECLAIM stays), and otherwise the pool goes on with
 * smaller pieces, as it does by itself whenever a size is refused. Every
 * other size, single pages, and every other device's pool are as the
 * kernel has them.
 *
 * Nothing of the kernel's is replaced. A probe at the entry of
 * ttm_pool_alloc_page() (a kprobe, on the function's ftrace site) takes
 * __GFP_DIRECT_RECLAIM off its gfp argument in exactly that case, and the
 * kernel's own function then runs.
 *
 * Whose pool: one PCI device's, named as it loads (`bdf=0000:00:02.0`),
 * which must be bound to xe then. The device and its driver's module are
 * held for as long as this is loaded, and the probe compares pointers.
 *
 * It trusts nothing until it has checked itself (two reviews with Astra,
 * 2026-11-21: a probe can be in and change nothing; one that changes
 * things before it knows the kernel is the one it was written for has
 * changed them on a kernel it then refuses). Until the check has passed,
 * the probe looks at one thing only, a pool of this module's own (with
 * the device as its owner, so the check goes the way real buffers will),
 * on the loading task, comparing its address before reading anything of
 * it. That pool is filled twice while the page allocator's tracepoint is
 * watched, each call the probe sees paired with what reached the
 * allocator: as the kernel has it, the 2 MB try must be the hard one
 * (direct and background reclaim, __GFP_RETRY_MAYFAIL, no __GFP_NORETRY)
 * and the 1 MB try must not reclaim at all; with the change, the 2 MB
 * try must differ by that one flag and the 1 MB try not at all. Anything
 * else, or anything unclear, and it says why and doesn't load: the
 * kernel is as it comes. Only then are real buffers' tries changed.
 *
 * That check is of what the function does with its arguments, not of
 * where this kernel's build keeps them: that is checked before loading,
 * by the build (apps/gpu-pages loads it only on a kernel and TTM build
 * listed in kernel/gpu-pages/QUALIFIED, each looked at and tried with
 * tools/dev/gpu-pages-check). x86-64.
 *
 * /sys/module/olo_gpu_pages/parameters: `on` (root's to write: what is
 * asked for; 0 leaves every try as the kernel has it, without
 * unloading), `changed` (real tries whose gfp argument the probe has
 * changed since it loaded, whether or not they would have waited) and
 * `missed` (the kernel's own count of calls the probe didn't see, which
 * is not all of them: probes switched off for the whole kernel are not
 * counted); `bdf`, and whose load it is: `service_id` (olo-gpu-pages',
 * which switches and removes only its own) or `trial_id` (a trial's:
 * below).
 */

#define pr_fmt(fmt) KBUILD_MODNAME ": " fmt

#include <linux/device.h>
#include <linux/gfp.h>
#include <linux/kernel.h>
#include <linux/kprobes.h>
#include <linux/mm.h>
#include <linux/miscdevice.h>
#include <linux/module.h>
#include <linux/pci.h>
#include <linux/percpu.h>
#include <linux/sched.h>
#include <linux/sizes.h>
#include <linux/slab.h>
#include <linux/tracepoint.h>
#include <drm/ttm/ttm_allocation.h>
#include <drm/ttm/ttm_bo.h>
#include <drm/ttm/ttm_pool.h>
#include <drm/ttm/ttm_tt.h>
#include <trace/events/kmem.h>

#ifndef CONFIG_X86_64
#error "olo_gpu_pages reads ttm_pool_alloc_page()'s arguments from x86-64's registers"
#endif

/* The size xe names: 2 MB, in pages as a power of two. */
#define OLO_ORDER	get_order(SZ_2M)

/*
 * Whose load this is: the service gives each load a name of its own and
 * keeps it, so it switches and removes only what it loaded itself.
 */
static char *service_id = "";
module_param(service_id, charp, 0444);
MODULE_PARM_DESC(service_id, "The name olo-gpu-pages gave this load (it switches and removes only its own)");

/* The one PCI device whose pool this is for. */
static char *bdf;
module_param(bdf, charp, 0444);
MODULE_PARM_DESC(bdf, "The PCI address of the device, bound to xe, whose buffers this is for (0000:00:02.0)");

static bool on = true;
static bool qualified;
static struct device *target;
static const struct device_driver *target_driver;
/*
 * A trial's load (tools/dev/gpu-pages-try), named by the trial so it
 * removes only its own: loaded switched off, and on only while the trial
 * holds /dev/olo_gpu_pages_trial_switch open. Whatever ends the trial,
 * a kill it can't catch too, closes that and puts the kernel's own
 * behaviour back; writing `on` does nothing for such a load.
 */
static char *trial_id = "";
module_param(trial_id, charp, 0444);
MODULE_PARM_DESC(trial_id, "The name a trial gave this load: on only while the trial holds its switch open");

static int set_on(const char *value, const struct kernel_param *kp)
{
	bool enable;

	if (*trial_id && (!value || kstrtobool(value, &enable) || enable ||
			  smp_load_acquire(&qualified)))
		return -EPERM;
	return param_set_bool(value, kp);
}

static const struct kernel_param_ops on_ops = {
	.flags = KERNEL_PARAM_OPS_FL_NOARG,
	.set = set_on,
	.get = param_get_bool,
};
module_param_cb(on, &on_ops, &on, 0644);

static int trial_open(struct inode *inode, struct file *file)
{
	if (!smp_load_acquire(&qualified))
		return -EAGAIN;
	return cmpxchg(&on, false, true) ? -EBUSY : 0;
}

static int trial_close(struct inode *inode, struct file *file)
{
	WRITE_ONCE(on, false);
	return 0;
}

static const struct file_operations trial_ops = {
	.owner = THIS_MODULE,
	.open = trial_open,
	.release = trial_close,
};

static struct miscdevice trial_switch = {
	.minor = MISC_DYNAMIC_MINOR,
	.name = "olo_gpu_pages_trial_switch",
	.fops = &trial_ops,
	.mode = 0600,
};
MODULE_PARM_DESC(on, "What is asked for: the 2 MB try for the device's buffers made without waiting (0: as the kernel has it)");

static DEFINE_PER_CPU(unsigned long, olo_changed);

/* The check: the loading task's alone, in task context, on its own pool. */
static struct task_struct *checking;
static struct ttm_pool *check_pool;
static bool test_change;
static struct {
	unsigned int pending, edits, large_count, small_count;
	gfp_t large, smaller;
	bool ambiguous;
} saw;

static int get_target(void)
{
	int ret = -ENODEV;

	if (!bdf)
		return -EINVAL;
	target = bus_find_device_by_name(&pci_bus_type, NULL, bdf);
	if (!target)
		return ret;

	device_lock(target);
	if (device_is_bound(target) && target->driver &&
	    !strcmp(target->driver->name, "xe") &&
	    try_module_get(target->driver->owner)) {
		target_driver = target->driver;
		ret = 0;
	}
	device_unlock(target);
	if (ret)
		put_device(target);
	return ret;
}

static void put_target(void)
{
	module_put(target_driver->owner);
	put_device(target);
}

/*
 * At ttm_pool_alloc_page(pool, gfp_flags, order)'s entry: %rdi, %rsi,
 * %rdx. Only the try the pool would make hard: the size its driver named,
 * 2 MB, from the page allocator itself (not coherent DMA memory), for xe.
 */
static int olo_before(struct kprobe *probe, struct pt_regs *regs)
{
	const struct ttm_pool *pool = (const struct ttm_pool *)regs->di;
	unsigned int order = (unsigned int)regs->dx;
	bool testing = !smp_load_acquire(&qualified);

	if (testing) {
		/*
		 * Before the check has passed, %rdi is a number to compare:
		 * nothing but this module's own pool is read. (An interrupt on
		 * the loading task isn't it.)
		 */
		if (current != READ_ONCE(checking) || !in_task() ||
		    pool != READ_ONCE(check_pool))
			return 0;
		if (saw.pending)
			saw.ambiguous = true;
		saw.pending = 0;
		if (order == OLO_ORDER || order == OLO_ORDER - 1) {
			if ((gfp_t)regs->si != GFP_HIGHUSER)
				saw.ambiguous = true;
			saw.pending = order;
		}
		if (!test_change)
			return 0;
	} else if (!READ_ONCE(on)) {
		return 0;
	}
	if (order != OLO_ORDER || !(regs->si & __GFP_DIRECT_RECLAIM))
		return 0;
	if ((pool->alloc_flags & (0xff | TTM_ALLOCATION_POOL_USE_DMA_ALLOC)) !=
	    TTM_ALLOCATION_POOL_BENEFICIAL_ORDER(OLO_ORDER))
		return 0;
	if (pool->dev != target || READ_ONCE(target->driver) != target_driver)
		return 0;
	regs->si &= ~(unsigned long)__GFP_DIRECT_RECLAIM;
	if (testing)
		saw.edits++;
	else
		__this_cpu_inc(olo_changed);
	return 0;
}
NOKPROBE_SYMBOL(olo_before);

static struct kprobe olo_probe = {
	.symbol_name = "ttm:ttm_pool_alloc_page",
	.pre_handler = olo_before,
};

static void olo_allocation(void *data, struct page *page, unsigned int order,
			   gfp_t gfp, int migratetype)
{
	if (current != READ_ONCE(checking) || !in_task() ||
	    (order != OLO_ORDER && order != OLO_ORDER - 1))
		return;
	if (saw.pending != order) {
		saw.ambiguous = true;
		return;
	}
	saw.pending = 0;
	if (order == OLO_ORDER) {
		saw.large = gfp;
		saw.large_count++;
	} else {
		if (saw.small_count && saw.smaller != gfp)
			saw.ambiguous = true;
		saw.smaller = gfp;
		saw.small_count++;
	}
}

static struct tracepoint *olo_page_alloc;

static void olo_find(struct tracepoint *tp, void *unused)
{
	if (!strcmp(tp->name, "mm_page_alloc"))
		olo_page_alloc = tp;
}

/*
 * 3 MB of a buffer filled from `pool` and given back: one piece of 2 MB
 * and one of 1 MB are asked of the page allocator whatever it has free
 * (a refused size goes on with the next one down).
 */
#define OLO_CHECK_PAGES	(3 << (OLO_ORDER - 1))

static int olo_fill(struct ttm_pool *pool, struct page **pages, bool change)
{
	struct ttm_operation_ctx ctx = { };
	struct ttm_tt tt = { .pages = pages, .num_pages = OLO_CHECK_PAGES, .caching = ttm_cached };
	int ret;

	memset(pages, 0, OLO_CHECK_PAGES * sizeof(*pages));
	memset(&saw, 0, sizeof(saw));
	test_change = change;
	WRITE_ONCE(check_pool, pool);
	WRITE_ONCE(checking, current);
	ret = ttm_pool_alloc(pool, &tt, &ctx);
	WRITE_ONCE(checking, NULL);
	WRITE_ONCE(check_pool, NULL);
	if (!ret)
		ttm_pool_free(pool, &tt);
	if (!ret && (saw.ambiguous || saw.pending || saw.large_count != 1 ||
		     !saw.small_count || saw.small_count > 3 || saw.edits != change)) {
		pr_err("its check of itself wasn't clear (2 MB tries seen: %u, 1 MB: %u, changed: %u, unanswered: %u, out of turn: %d): not loading\n",
		       saw.large_count, saw.small_count, saw.edits, saw.pending, saw.ambiguous);
		ret = -EOPNOTSUPP;
	}
	return ret;
}

static int olo_check(void)
{
	struct ttm_pool *pool = kzalloc(sizeof(*pool), GFP_KERNEL);
	struct page **pages = kvcalloc(OLO_CHECK_PAGES, sizeof(*pages), GFP_KERNEL);
	gfp_t stock = 0, smaller = 0, ours = 0;
	bool same_smaller = false;
	int ret = -ENOMEM;

	/* (The callback's shape against the kernel's own declaration: no build where it differs.) */
	check_trace_callback_type_mm_page_alloc(olo_allocation);
	if (!pool || !pages)
		goto out;
	for_each_kernel_tracepoint(olo_find, NULL);
	ret = olo_page_alloc ? tracepoint_probe_register(olo_page_alloc, olo_allocation, NULL) : -ENOENT;
	if (ret) {
		pr_err("the page allocator's tracepoint can't be watched (%d): not loading\n", ret);
		goto out;
	}
	ttm_pool_init(pool, target, NUMA_NO_NODE, TTM_ALLOCATION_POOL_BENEFICIAL_ORDER(OLO_ORDER));
	ret = olo_fill(pool, pages, false);
	if (!ret) {
		stock = saw.large;
		smaller = saw.smaller;
		if ((stock & __GFP_RECLAIM) != __GFP_RECLAIM ||
		    !(stock & __GFP_RETRY_MAYFAIL) || (stock & __GFP_NORETRY) ||
		    (smaller & __GFP_RECLAIM)) {
			pr_info("this kernel's pool isn't the one this was written for (as it comes, the 2 MB try's flags are %#x and the 1 MB try's %#x): not loading\n", stock, smaller);
			ret = -EOPNOTSUPP;
		}
	}
	if (!ret) {
		ret = olo_fill(pool, pages, true);
		ours = saw.large;
		same_smaller = smaller == saw.smaller;
	}
	ttm_pool_fini(pool);
	tracepoint_probe_unregister(olo_page_alloc, olo_allocation, NULL);
	tracepoint_synchronize_unregister();
	if (ret) {
		pr_err("its check of itself failed (%d): not loading\n", ret);
	} else if (!same_smaller || ours != (stock & ~__GFP_DIRECT_RECLAIM)) {
		pr_err("the probe is in, but its change didn't reach the page allocator as meant (the 2 MB try's flags %#x became %#x, the 1 MB try's %#x became %#x): not loading\n",
		       stock, ours, smaller, saw.smaller);
		ret = -EIO;
	} else {
		pr_info("checked on %s: the 2 MB try's flags %#x become %#x and the 1 MB try's stay %#x; switched %s\n",
			bdf, stock, ours, smaller, READ_ONCE(on) ? "on" : "off");
	}
out:
	kvfree(pages);
	kfree(pool);
	return ret;
}

static int olo_changed_get(char *buffer, const struct kernel_param *kp)
{
	unsigned long sum = 0;
	int cpu;

	for_each_possible_cpu(cpu)
		sum += READ_ONCE(per_cpu(olo_changed, cpu));
	return sysfs_emit(buffer, "%lu\n", sum);
}

static const struct kernel_param_ops olo_changed_ops = { .get = olo_changed_get };
module_param_cb(changed, &olo_changed_ops, NULL, 0444);
MODULE_PARM_DESC(changed, "Real 2 MB tries whose gfp argument the probe has changed since this loaded");

static int olo_missed_get(char *buffer, const struct kernel_param *kp)
{
	return sysfs_emit(buffer, "%lu\n", READ_ONCE(olo_probe.nmissed));
}

static const struct kernel_param_ops olo_missed_ops = { .get = olo_missed_get };
module_param_cb(missed, &olo_missed_ops, NULL, 0444);
MODULE_PARM_DESC(missed, "The kernel's count of calls the probe didn't see (not all of them: probes switched off for the whole kernel aren't counted)");

static int __init olo_gpu_pages_init(void)
{
	int ret;

	if (*trial_id && on)
		return -EINVAL;
	ret = get_target();

	if (ret) {
		pr_err("no PCI device %s bound to xe (%d): not loading\n", bdf ?: "named (bdf=)", ret);
		return ret;
	}
	ret = register_kprobe(&olo_probe);

	if (ret) {
		pr_err("no probe at ttm_pool_alloc_page (%d): not loading\n", ret);
		put_target();
		return ret;
	}
	ret = olo_check();
	if (!ret && *trial_id)
		ret = misc_register(&trial_switch);
	if (ret) {
		unregister_kprobe(&olo_probe);
		put_target();
	} else {
		smp_store_release(&qualified, true);
	}
	return ret;
}

static void __exit olo_gpu_pages_exit(void)
{
	if (*trial_id)
		misc_deregister(&trial_switch);
	unregister_kprobe(&olo_probe);
	put_target();
}

module_init(olo_gpu_pages_init);
module_exit(olo_gpu_pages_exit);
MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("Olo: no program is held for a 2 MB piece of graphics memory (xe's 2 MB try made without direct reclaim)");

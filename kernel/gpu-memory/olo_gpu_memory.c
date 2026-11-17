// SPDX-License-Identifier: GPL-2.0
/*
 * olo_gpu_memory: graphics memory stays in memory.
 *
 * TTM, the memory manager under Intel's xe graphics driver (and others),
 * backs GPU buffers up to swap when the kernel is short of memory, were
 * it only short of file cache, and only while there is swap space free:
 * ttm_backup_bytes_avail() is its one gate (ttm_pool_backup() refuses
 * without it, and xe's shrinker counts nothing to shrink). A buffer
 * backed up comes back, page by page, inside its program's next GPU
 * submission, under its window buffers' locks, which the compositor's
 * every submission takes too: the whole desktop stands still meanwhile,
 * for up to seconds (docs/HACKING.md, "Windows' buffers stay in
 * memory").
 *
 * This live patch answers the gate's question with "none": no GPU
 * buffer is backed up, whatever swap the computer's ordinary memory
 * has. What a program has said it no longer needs (purgeable) is still
 * freed. One function replaced, in the running kernel; unloading the
 * module (after `echo 0 > /sys/kernel/livepatch/olo_gpu_memory/enabled`)
 * puts the kernel's own back.
 *
 * Built for each kernel by olo-gpu-memory (apps/gpu-memory), which
 * loads it at every start.
 */

#define pr_fmt(fmt) KBUILD_MODNAME ": " fmt

#include <linux/build_bug.h>
#include <linux/kernel.h>
#include <linux/livepatch.h>
#include <linux/module.h>
#include <drm/ttm/ttm_backup.h>

static u64 olo_ttm_backup_bytes_avail(void)
{
	return 0;
}

/*
 * A kernel that comes with an update is built for like any other. If its
 * ttm_backup_bytes_avail() is no longer there, or no longer takes
 * nothing and gives a u64, this doesn't build: olo-gpu-memory says so
 * and the kernel is left as it comes, where a function of another shape
 * put in its place would be called wrongly.
 */
static_assert(__builtin_types_compatible_p(typeof(&ttm_backup_bytes_avail), typeof(&olo_ttm_backup_bytes_avail)));

static struct klp_func funcs[] = {
	{
		.old_name = "ttm_backup_bytes_avail",
		.new_func = olo_ttm_backup_bytes_avail,
	},
	{ }
};

static struct klp_object objs[] = {
	{
		.name = "ttm",
		.funcs = funcs,
	},
	{ }
};

static struct klp_patch patch = {
	.mod = THIS_MODULE,
	.objs = objs,
};

static int __init olo_gpu_memory_init(void)
{
	return klp_enable_patch(&patch);
}

static void __exit olo_gpu_memory_exit(void)
{
}

module_init(olo_gpu_memory_init);
module_exit(olo_gpu_memory_exit);
MODULE_LICENSE("GPL");
MODULE_INFO(livepatch, "Y");
MODULE_DESCRIPTION("Olo: GPU buffers are never backed up to swap");

// SPDX-License-Identifier: GPL-2.0
/*
 * olo_ec_storm: an embedded controller that floods the kernel with events
 * doesn't take the keyboard's with it.
 *
 * The ACPI EC driver fetches every event the controller has and queues
 * the firmware's handler (_Qxx) for each, without limit. A handler may be
 * slow by the firmware's own design (a ThinkPad's adapter handlers sleep
 * half a second, then take a firmware mutex and sleep a tenth more inside
 * it: ten a second at most, however many workers), so a controller that
 * reports the adapter coming and going tens of times a second builds a
 * backlog without bound (195,778 on a P1 Gen 9, 2026-11-17): every other
 * event (the brightness keys, the USB-C ports, the lid) waits behind it,
 * and the backlog is replayed for hours after the controller stops.
 *
 * An event of the controller's carries nothing but its number: its
 * handler reads what is so now. So of the same event queued over and
 * over, all but the last few say nothing new. This puts a handler of its
 * own in front of each of the firmware's (the driver's own way for a
 * module to handle a query: acpi_ec_add_query_handler, whose handlers
 * are found before the firmware's). While none of an event's is running,
 * it runs the firmware's method as ever. While one is, the event is only
 * counted, up to OWED: the one running runs the method again for each
 * owed, then stops. A flood of one event then costs a counter's change a
 * time and one worker running its method back to back; the queue stays
 * short, and the other events' handlers run as they come.
 *
 * No structure of the driver's is read: the controller is found as the
 * ACPI device it is (PNP0C09), its methods by walking its namespace, as
 * the driver finds them. The driver's two functions and the pointer to
 * the controller it serves are exported but declared in its own header
 * (drivers/acpi/internal.h), so they are declared again here, as that
 * header has them in the kernel series this was checked on
 * (kernel/ec-storm/CHECKED; olo-ec-storm loads this on no other).
 *
 * Built for each kernel by olo-ec-storm (apps/ec-storm), which loads it
 * at every start. Unloading it gives every event back to the firmware's
 * handlers alone.
 */

#define pr_fmt(fmt) KBUILD_MODNAME ": " fmt

#include <linux/acpi.h>
#include <linux/atomic.h>
#include <linux/delay.h>
#include <linux/kernel.h>
#include <linux/module.h>
#include <linux/slab.h>

/* drivers/acpi/internal.h's own, exported to modules. */
struct acpi_ec;
extern struct acpi_ec *first_ec;
typedef int (*acpi_ec_query_func)(void *data);
int acpi_ec_add_query_handler(struct acpi_ec *ec, u8 query_bit, acpi_handle handle, acpi_ec_query_func func, void *data);
void acpi_ec_remove_query_handler(struct acpi_ec *ec, u8 query_bit);

struct guard;
static void olo_ec_storm_run(struct guard *g);
#include "olo_ec_storm_count.h"

struct guard {
	struct olo_ec_storm_count count;
	/* The firmware's method for this event (NULL: it has none). */
	acpi_handle method;
	bool guarded;
};

static struct guard guards[256];
static unsigned int guarding;

static int stats_get(char *buf, const struct kernel_param *kp)
{
	long ran = 0, counted = 0;
	int i, most = -1;

	for (i = 0; i < 256; i++) {
		ran += atomic_long_read(&guards[i].count.ran);
		counted += atomic_long_read(&guards[i].count.counted);
		if (atomic_long_read(&guards[i].count.counted) && (most < 0 || atomic_long_read(&guards[i].count.counted) > atomic_long_read(&guards[most].count.counted)))
			most = i;
	}
	if (most < 0)
		return sysfs_emit(buf, "guarding %u ran %ld counted %ld\n", guarding, ran, counted);
	return sysfs_emit(buf, "guarding %u ran %ld counted %ld most 0x%02x %ld\n", guarding, ran, counted, most, atomic_long_read(&guards[most].count.counted));
}
static const struct kernel_param_ops stats_ops = { .get = stats_get };
/*
 * "guarding N ran R counted C [most 0xQQ M]": the events guarded, the
 * methods run, the events only counted (one of theirs was running), and
 * the event counted most.
 */
module_param_cb(stats, &stats_ops, NULL, 0444);

/* The firmware's method for an event, run. */
static void olo_ec_storm_run(struct guard *g)
{
	acpi_evaluate_object(g->method, NULL, NULL, NULL);
}

/* An event of the controller's, in the driver's worker for it. */
static int olo_ec_storm_query(void *data)
{
	struct guard *g = data;

	olo_ec_storm_event(&g->count, g);
	return 0;
}

/* A method of the controller's: one named _Qxx is an event's. */
static acpi_status olo_ec_storm_method(acpi_handle handle, u32 level, void *context, void **ret)
{
	char name[5];
	struct acpi_buffer buffer = { sizeof(name), name };
	unsigned int value;

	if (ACPI_FAILURE(acpi_get_name(handle, ACPI_SINGLE_NAME, &buffer)))
		return AE_OK;
	if (sscanf(name, "_Q%x", &value) == 1 && value > 0 && value < 256)
		guards[value].method = handle;
	return AE_OK;
}

static void olo_ec_storm_remove(void)
{
	int i;

	/* (Each waits for the events queued to it to be done.) */
	for (i = 0; i < 256; i++)
		if (guards[i].guarded) {
			acpi_ec_remove_query_handler(first_ec, i);
			guards[i].guarded = false;
		}
}

static int __init olo_ec_storm_init(void)
{
	struct acpi_device *adev, *other;
	int i, ret;

	if (!first_ec)
		return -ENODEV;
	/*
	 * The one controller: with two, which of them the driver's pointer
	 * names can't be told from here, and neither is touched.
	 */
	adev = acpi_dev_get_first_match_dev("PNP0C09", NULL, -1);
	if (!adev)
		return -ENODEV;
	other = acpi_dev_get_next_match_dev(adev, "PNP0C09", NULL, -1);
	if (other) {
		acpi_dev_put(other);
		acpi_dev_put(adev);
		pr_info("more than one embedded controller: none guarded\n");
		return -ENODEV;
	}
	acpi_walk_namespace(ACPI_TYPE_METHOD, adev->handle, 1, olo_ec_storm_method, NULL, NULL, NULL);
	acpi_dev_put(adev);
	for (i = 0; i < 256; i++) {
		if (!guards[i].method)
			continue;
		ret = acpi_ec_add_query_handler(first_ec, i, NULL, olo_ec_storm_query, &guards[i]);
		if (ret) {
			pr_err("event 0x%02x: %d\n", i, ret);
			olo_ec_storm_remove();
			return ret;
		}
		guards[i].guarded = true;
		guarding++;
	}
	if (!guarding)
		return -ENODEV;
	pr_info("guarding %u of the embedded controller's events\n", guarding);
	return 0;
}

static void __exit olo_ec_storm_exit(void)
{
	long ran = 0, counted = 0;
	int i;

	olo_ec_storm_remove();
	/*
	 * An event fetched as its handler was taken out was queued a moment
	 * after the driver's wait for the queue began: once more, when that
	 * moment is long over, so nothing of this module's is still to run.
	 */
	msleep(100);
	acpi_ec_remove_query_handler(first_ec, 0);
	for (i = 0; i < 256; i++) {
		ran += atomic_long_read(&guards[i].count.ran);
		counted += atomic_long_read(&guards[i].count.counted);
	}
	pr_info("gone: %ld methods run, %ld events only counted\n", ran, counted);
}

module_init(olo_ec_storm_init);
module_exit(olo_ec_storm_exit);
MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("Olo: a flood of one EC event doesn't hold up the others");

// SPDX-License-Identifier: GPL-2.0
/*
 * For a guest only (tools/dev/gpu-pages-check): a PCI driver named "xe"
 * for a device of QEMU's that no driver has, its host bridge, so that a
 * guest has a PCI device really bound to a driver of that name (the
 * kernel's own binding: its lock, its unbind and bind files), which is
 * what olo_gpu_pages asks of the device it is told. It does nothing with
 * the device.
 */

#include <linux/module.h>
#include <linux/pci.h>

static const struct pci_device_id bridges[] = {
	{ PCI_DEVICE(PCI_VENDOR_ID_INTEL, 0x1237) },	/* i440FX */
	{ PCI_DEVICE(PCI_VENDOR_ID_INTEL, 0x29c0) },	/* Q35 */
	{ }
};

static int probe(struct pci_dev *dev, const struct pci_device_id *id)
{
	return 0;
}

static struct pci_driver stand_in = {
	.name = "xe",
	.id_table = bridges,
	.probe = probe,
};
module_pci_driver(stand_in);
MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("Olo, for a test guest: a PCI driver named xe for QEMU's host bridge");

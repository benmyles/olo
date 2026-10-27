<!-- Special Topics - Device Drivers - Introduction -->

## Introduction

Writing device drivers requires additional knowledge of the inner workings of the BeOS. To write a driver you must follow the rules laid out in this chapter very carefully. These rules are not the same as those for writing a normal application—if your driver tries to do things it's not allowed to do, it could bring down the system.

This introduction covers how drivers interact with the kernel.

* * *

### The Kernel and the Driver Author

The BeOS kernel comprises the basic functionality of the operating system: It knows how to start the boot process and to manage memory and threads, and it contains the PCI bus manager, the ISA bus manager, the device file system (devfs, which manages `/dev`), the root file system (rootfs, which manages `/`), and a few other things.

But this isn't enough to satisfy the needs of most applications, so the kernel uses add-ons to provide additional functionality. During the boot process, add-ons are loaded to handle "real" file systems, devices, busses, and the like.

Although Be's kernel add-ons provide support for a wide range of hardware—from disk devices to joysticks—this support isn't all-inclusive. Hardware developers may need to create their own drivers for their products.

#### Types of Kernel Add-on

There are three types of kernel add-on:

  1. Device drivers are add-ons that communicate directly with devices.

  2. Modules are kernel space add-ons that export an API for use by drivers (or by other modules).

  3. File systems are add-ons that support specific file systems, such as BFS, DOSFS, HFS, and so forth.

Device drivers and file systems, while extending the functionality of the kernel, are still accessible from user space: Applications can open and address them using file descriptors. Modules, on the other hand, are kernel-only units. Applications have no access to them; they're provided strictly for use by the kernel and other kernel add-ons.

##### Device Drivers

A device driver is an add-on that recognizes a specific device (or class of devices) and provides a means for the rest of the system to communicate with it. Usually this communication involves some form of device-specific protocol. For example, if the system wants to use an Ethernet card or graphics card, it needs to load a device driver add-on that knows how to communicate with that card. Similarly, code that knows how to talk to a class of devices (SCSI disks, ATA devices, ATAPI disks, or USB input devices, etc.) must be implemented as a device driver add-on.

##### Modules

Modules provide a uniform API for use by other modules and drivers. A module is like a library in that it acts as a repository for common code that's shared among several drivers.

For example: Let's say you have a device driver that talks to a SCSI device connected to a SCSI bus. A computer can have multiple SCSI busses. Because all SCSI devices use the same command set independent of the particular controller used to send the commands, the command set can be (and is) implemented as a module. The SCSI module knows how to handle all SCSI cards the BeOS supports; the API that the SCSI module defines is adopted by and augmented by the modules for specific SCSI device types (hard disks, scanners, CD drives, etc). The SCSI device modules are managed by a SCSI bus manager module, which knows how to cope with multiple busses and presents them in encapulated form to the drivers. The drivers then only need to deal with the bus manager's API, which makes the life of a driver author much simpler.

Be provides bus managers for SCSI, USB, IDE, and PCMCIA.

##### File Systems

File system add-ons provide support for disk and network file systems, such as BFS, HFS, FAT, ISO 9660, CIFS, and so forth. By creating new file system add-ons, developers can provide access to disks that are formatted using other file system.

#### Interactions with the Kernel

The kernel provides a number of services that drivers and modules can use. These include:

  * Enabling and disabling interrupts.

  * Setting up memory for DMA transactions.

  * Access to other devices and modules.

The kernel also provides, at the user level, a Posix-like API for accessing devices. An application can open a device through `open()`, and use `read()`, `write()`, and `ioctl()` to access the device.

The Posix functions are converted into system calls into the kernel, which then passes them, via devfs, to the appropriate device driver.

* * *

### devfs

The kernel manages device drivers through devfs, the device file system that's mounted at `/dev` during the boot process. In order to be accessed, a driver must "publish" itself by adding an entry in the `/dev` hierarchy. The basic Posix I/O functions (`open()`, `read()`, `write()`, `readv()`, `writev()`, `ioctl()`, and `close()`) can then be used.

Devfs makes the drivers available as needed in `/dev`; this usually happens the first time a program iterates through the directory entries for a subdirectory in `/dev`. The kernel knows where in the /dev hierarchy to publish drivers based on their location in `/boot/beos/system/add-ons/kernel/drivers/dev`. For example, the ATAPI driver publishes drivers in `/dev/disk/ide/atapi`, the driver is located in `/boot/beos/system/add-ons/kernel/drivers/dev/disk/ide/atapi`. Whew.

You can see this device hierarchy by using the "ls" command from a Terminal window. `ls /dev` will show you the root of the device hierarchy, `ls /dev/disk` will show you disk device busses, `ls /dev/disk/ide` will show you the IDE devices, and so forth.

In reality, drivers tend to publish themselves in multiple locations in the `/dev` hierarchy, so instead of putting duplicate copies of the driver in the `…/drivers/dev` tree, the driver binaries are put at `/boot/beos/system/add-ons/kernel/drivers/bin`, and symlinks are created in the `…/drivers/dev` tree at the appropriate place. (The same is also done for drivers in `/boot/home/config/add-ons/kernel/drivers/…`)

* * *

### Driver Implementation Principles

Much of the stability of the BeOS is achieved by constructing a nearly impenetrable wall between the kernel and user applications. Drivers are chinks in that wall. If a driver misbehaves or fails, there's a strong possibility that it will cause unexpected behavior or kill the entire system. It's absolutely critical that drivers not only be very carefully tested before being released to the public, but that they follow the rules to the letter.

#### Kernel Space vs. User Space

One way you can reduce the risk of your driver causing a general system failure is by putting as much code as possible in user space. Create a driver that loads into kernel space just enough code to handle the low-level interactions that absolutely have to be done in kernel space, then load code into user space to handle the rest of the work. If the add-on fails, the system will keep running—only your driver will fail.

Another plus to placing as much of your code as possible into user space is that it's much easier to debug code running in user space. Conventional debugging techniques that don't work for kernel code can be applied, and there's much less chance of taking down the system in the process.

#### Code Synchronization

Normally, spinlocks are a bad thing. A spinlock is a tight loop that watches for a condition to occur, looping endlessly until that condition is met (this is called "busy waiting"). This wastes valuable processor time, and is normally discouraged.

In general, you're encouraged to use semaphores instead of spinlocks; however, you can't acquire a semaphore while handling an interrupt. So if you need to synchronize code while handling an interrupt, you must use a spinlock. Put simply:

  * Use spinlocks to protect critical sections in interrupt-handling code.

  * Use semaphores in any other situation that calls for code synchronization.

Anywhere you use a spinlock to protect a critical section, you should disable interrupts. Of course, in an interrupt handler, you know that interrupts are already disabled, so you don't need to explicitly disable interrupts yourself. Interrupt handlers include I/O interrupts installed using `install_io_interrupt()` and timer interrupts installed by calling `add_timer()`.

##### Functions Available During Spinlocks

While your spinlock is running, you can perform the following actions. If it's not on this list, you can't do it.

  * You can examine and alter hardware registers by using the appropriate bus manager hooks.

  * You can examine and alter any locked-down memory.

  * You can call the following kernel functions: `system_time()`, `atomic_add()`, `atomic_or()`, `atomic_and()`.

  * You can call the following bus manager functions: `read_io_*()` and `write_io*()`.

If you do anything else inside your spinlock, you're breaking the rules, so don't do it.

##### Using Spinlocks

You need to be sure that your calls to `acquire_spinlock()` and `release_spinlock()` are balanced. In addition, if you nest spinlocks, they must be released in logical order—that is, in the opposite order in which they're acquired.

The kernel keeps track of which spinlocks are being held and which are being waited upon. The kernel assumes that spinlocks are initialized to 0, and then acquired and released in logical order.

By keeping track of spinlocks, the kernel can detect and break deadlocks on multiprocessor systems.

#### Disabling Interrupts

The only time you should ever disable interrupts in a device driver is just before entering a spinlock-protected critical section. There is absolutely no other reason to do it, so don't.

After disabling interrupts, you should reenable them as quickly as possible. You must never, under any circumstances, leave interrupts disabled for more than 50 microseconds. This means that your interrupt handler code (which runs with interrupts implicitly disabled) must execute in 50 microseconds or less.

##### Functions Available While Interrupts Are Disabled

If you have interrupts disabled and aren't in a spinlock, you can do the following things in addition to those listed above in "Functions Available During Spinlocks":

  * You can call `release_sem_etc()` with the `B_DO_NOT_RESCHEDULE` flag set.

  * You can call `get_sem_count()`, `add_timer()`, `cancel_timer()`, and `dprintf()`.

If you feel that you need to call a function not explicitly listed as permitted here, please contact Be Developer Support at devsupport@be.com and explain your needs; we'd be happy to discuss the situation with you.

##### Don't Block

It's crucial that your interrupt handler never block, whether directly (by acquiring a semaphore, for example) or indirectly (by calling a function that might block).

Blocking can happen in a surprisingly large number of BeOS functions. It's obvious that `acquire_sem()` can block, but you might not be aware that functions such as `malloc()` or `read_port()` can block. Even touching unlocked memory areas can block because of virtual memory hits.

The point is this: If the BeOS function you want to call isn't explicitly listed in this section as one you can use, don't call it.

##### Don't Preempt

Your interrupt handler or spinlock section can't be preempted. Preemption could occur if you call `release_sem()` or `release_sem_etc()` without specifying the `B_DO_NOT_RESCHEDULE` flag. Normally, `release_sem()` lets the scheduler preempt your thread to allow other threads to acquire the semaphore as fast as possible. By specifying `B_DO_NOT_RESCHEDULE`, you tell the scheduler to allow your thread to continue running after it releases the semaphore.

If your interrupt handler wants to ensure that any preemption is handled immediately, it should specify `B_DO_NOT_RESCHEDULE` when calling `release_sem()`, then return `B_INVOKE_SCHEDULER`. This causes the scheduler to immediately handle preemption after your interrupt handler returns, instead of resuming the interrupted task. This is especially useful if your code called `release_sem_etc()` to release a semaphore that will allow other code to run elsewhere (such as in your driver's corresponding user-space code).

Warning

![Warning](./images/admonitions/Stop_32.png)

Again, when you call `release_sem_etc()`, be sure to specify the `B_DO_NOT_RESCHEDULE` flag to avoid any chance of preemption.

In summary, the order in which you should do things is this:

  * Disable interrupts.

  * Acquire the spinlock.

  * Perform your tasks.

  * Release the spinlock.

  * Restore the original interrupt state.

##### File I/O

Sometimes a driver needs to be able to access disk files. Perhaps the driver has a preference file it needs to read. There are two ways to do this. You can use Posix I/O calls, or you can use the driver settings API provided by BeOS. The latter is preferred.

###### Using Posix Calls

Under BeOS, device drivers can access disk files using the standard low-level Posix I/O functions: `open()`, `close()`, `read()`, `write()`, and so forth. There aren't any special chores to attend to beforehand. Just `open()` the file and do your thing.

Two Posix extensions that might be helpful when you're writing code to perform file I/O from a device driver: `readv()` and `writev()`.
    
    
    int readv(int fd, const struct iovec*vector, size_t count);
    
    int writev(int fd, const struct iovec*vector, size_t count);
    
    struct iovec {
       __ptr_t iov_base;
       size_t iov_len;
    };

These functions provide a means to read and write contiguous portions of a file from multiple buffers. vector is a pointer to an array containing count vector records, each of which contains a pointer to a buffer, and the size of the buffer. `readv()` fills these buffers with data from the file, and `writev()` writes them to the file, in order.

When successful, `readv()` returns the number of bytes read.

For example, if your code needs to write two separate 1k buffers into a file, one after the other, you might do something like this:
    
    
    struct iovec v[2];
    v[0].iov_base = &buffer1;
    v[0].iov_len = 1024;
    v[1].iov_base = &buffer2;
    v[1].iov_len = 1024;
    if (writev(fd, &v, 2) != B_OK) {
       /* error */
    }

Performing vectored I/O like this is often faster than doing multiple calls to `read()` and `write()`.

###### The Driver Settings API

If your driver is loaded before the file system for the disk on which your settings file resides, your driver might not be able to load its settings using Posix calls. The driver settings API lets you work around this circumstance. See "[Driver Settings API](DeviceDrivers_DriverSettingsAPI.md "Driver Settings API")" for details.

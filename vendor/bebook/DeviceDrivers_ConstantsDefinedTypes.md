<!-- Special Topics - Device Drivers - Constants And Defined Types -->

## Constants And Defined Types

Declared in: `drivers/Drivers.h`

This section covers constants and types defined for use by kernel drivers and modules.

* * *

### Constants

#### Current Driver API Version
    
    
    #define B_CUR_DRIVER_API_VERSION _N_

The `B_CUR_DRIVER_API_VERSION` constant indicates what version of the driver API your driver will be built to.

See also: "[Symbols Drivers Export](DeviceDrivers_WritingDrivers.md#DeviceDrivers_SymbolsDriversExport "Symbols Drivers Export")"

####  Driver Control Opcodes

Constant| Description  
---|---  
`B_GET_DEVICE_SIZE`| Returns a size_t indicating the device size in bytes.  
`B_SET_DEVICE_SIZE`| Sets the device size to the value pointed to by data.  
`B_SET_NONBLOCKING_IO`| Sets the device to use nonblocking I/O.  
`B_SET_BLOCKING_IO`| Sets the device to use blocking I/O.  
`B_GET_READ_STATUS`| Returns `true` if the device can read without blocking, otherwise `false`.  
`B_GET_WRITE_STATUS`| Returns `true` if the device can write without blocking, otherwise `false`.  
`B_GET_GEOMETRY`| Fills out the specified [device_geometry](DeviceDrivers_ConstantsDefinedTypes.md#device_geometry "device_geometry") structure to describe the device.  
`B_GET_DRIVER_FOR_DEVICE`| Returns the path of the driver executable handling the device.  
`B_GET_PARTITION_INFO`| Returns a [partition_info](DeviceDrivers_ConstantsDefinedTypes.md#partition_info "partition_info") structure for the device.  
`B_SET_PARTITION`| Creates a user-defined partition. data points to a [partition_info](DeviceDrivers_ConstantsDefinedTypes.md#partition_info "partition_info") structure.  
`B_FORMAT_DEVICE`| Formats the device. data should point to a boolean value: If `true`, the device is formatted low-level. If it's `false`, the formatting is <<??>>  
`B_EJECT_DEVICE`| Ejects the device.  
`B_GET_ICON`| Fills out the specified [device_icon](DeviceDrivers_ConstantsDefinedTypes.md#device_icon "device_icon") structure to describe the device's icon.  
`B_GET_BIOS_GEOMETRY`| Fills out a [device_geometry](DeviceDrivers_ConstantsDefinedTypes.md#device_geometry "device_geometry") structure to describe the device as the BIOS sees it.  
`B_GET_MEDIA_STATUS`| Gets the status of the media in the device by placing a status_t at the location pointed to by data.`B_GET_MEDIA_STATUS` returns one of these values in data:

`B_NO_ERROR`.
    The media is ready.
`B_DEV_NO_MEDIA`.
    No media in the device
`B_DEV_NOT_READY`.
    The device isn't ready.
`B_DEV_MEDIA_CHANGED`.
    The media changed since the time that the device was opened, or since the time of the last `B_GET_MEDIA_STATUS` request.
`B_DEV_MEDIA_CHANGE_REQUEST`.
    The user wants to remove the media.
`B_DEV_DOOR_OPEN`.
    The drive "door" is open.  
`B_LOAD_MEDIA`| Loads the media.  
`B_GET_BIOS_DRIVE_ID`| Returns the BIOS ID for the device.  
`B_SET_UNINTERRUPTABLE_IO`| Prevents control+C from interrupting I/O.  
`B_SET_INTERRUPTABLE_IO`| Allows control+C to interrupt I/O.  
`B_FLUSH_DRIVE_CACHE`| Flushes the drive's cache.  
`B_GET_NEXT_OPEN_DEVICE`| Iterates through open devices; data points to an open_device_iterator.  
`B_ADD_FIXED_DRIVER`| For internal use only.  
`B_REMOVE_FIXED_DRIVER`| For internal use only.  
`B_AUDIO_DRIVER_BASE`| Base for codes in `audio_driver.h`.  
`B_MIDI_DRIVER_BASE`| Base for codes in `midi_driver.h`.  
`B_JOYSTICK_DRIVER_BASE`| Base for codes in `joystick.h`.  
`B_GRAPHIC_DRIVER_BASE`| Base for codes in `graphic_driver.h`.  
`B_DEVICE_OP_CODES_END`| End of Be-defined control IDs.  
  
* * *

### Defined Types

#### device_geometry
    
    
    typedef struct {
        uint32 bytes_per_sector;
        uint32 sectors_per_track;
        uint32 cylinder_count;
        uint32 head_count;
        uchar device_type;
        bool removable;
        bool read_only;
        bool write_once;
    } device_geometry

The device_geometry structure is returned by the `B_GET_GEOMETRY` driver control function. Its fields are:

Field| Description  
---|---  
`bytes_per_sector`| Indicates how many bytes each sector of the disk contains.  
`sectors_per_track`| Indicates how many sectors each disk track contains.  
`cylinder_count`| Indicates the number of cylinders the disk contains.  
`head_count`| Indicates how many heads the disk has.  
`device_type`| Specifies the type of device; there's a list of device type definitions below.  
`removable`| Is `true` if the device's media can be removed from the drive, and is `false` otherwise.  
`read_only`| Is `true` if the media is read-only (such as CD-ROM), or `false` if the media can be both read from and written.  
`write_once`| Is `true` if the media can only be written to once (such as CD-recordable), or `false` if there's no limit to the number of times the media can be written to.  
  
If you need to compute the total size of the device in bytes, you can obtain this figure using the following simple formula:
    
    
    disk_size = geometry.cylinder_count * geometry.sectors_per_track *
    geometry.head_count * geometry.bytes_per_sector;

The device type returned in `device_type` is one of:

Constant| Description  
---|---  
`B_DISK`| Hard disk, floppy disk, etc.  
`B_TAPE`| Tape drive.  
`B_PRINTER`| Printer.  
`B_CPU`| CPU device.  
`B_WORM`| Write-once, read-many device (such as CD-recordable).  
`B_CD`| CD-ROM.  
`B_SCANNER`| Scanner.  
`B_OPTICAL`| Optical device  
`B_JUKEBOX`| Jukebox device.  
`B_NETWORK`| Network device.  
  
#### device_hooks
    
    
    typedef struct {
        device_open_hook open;
        device_close_hook close;
        device_free_hook free;
        device_control_hook control;
        device_read_hook read;
        device_write_hook write;
        device_select_hook select;
        device_deselect_hook deselect;
        device_readv_hook readv;
        device_writev_hook writev;
    } device_hooks

This structure is used by device drivers to export their function hooks to the kernel.

#### device_icon
    
    
    typedef struct {
        int32 icon_size;
        void*icon_data;
    } device_icon

When you want to obtain an icon for a specific device, call `ioctl()` on the open device, specifying the `B_GET_ICON` opcode. Pass in data a pointer to a device_icon structure in which `icon_size` indicates the size of icon you want and `icon_data` points to a buffer large enough to receive the icon's data.

`icon_size` can be either `B_MINI_ICON`, in which case the buffer pointed to by `icon_data` should be large enough to receive a 16x16 8-bit bitmap (256-byte), or `B_LARGE_ICON`, in which case the buffer should be large enough to receive a 32x32 8-bit bitmap (1024-byte). The most obvious way to set up this buffer would be to create a [`BBitmap`](BBitmap.md "BBitmap") of the appropriate size and color depth and use its buffer, like this:
    
    
    BBitmap bits(BRect(0, 0, B_MINI_ICON-1, B_MINI_ICON-1, 0, B_CMAP8));
    device_icon iconrec;
    
    iconrec.icon_size = B_MINI_ICON;
    iconrec.icon_data = bits.Bits();
    status_t err = ioctl(dev_fd, B_GET_ICON, &iconrec);
    
    if (err == B_OK) {
       /* enjoy the icon */
       ...
       view->DrawBitmap(bits);
    } else {
       /* I don't like icons anyway */
    }

#### driver_path
    
    
    typedef char driver_path[256];

Used by the `B_GET_DRIVER_FOR_DEVICE` control function to return the pathname of the specified device.

#### open_device_iterator
    
    
    typedef struct {
        uint32 cookie;
        char device[256];
    } open_device_iterator

Used by the `B_GET_NEXT_OPEN_DEVICE` control function. The first time you call this function, your open_device_iterator should have cookie initialized to 0. Then just keep calling it over and over; each time you'll get the name of the next open device. When an error is returned, you're done.

#### partition_info
    
    
    typedef struct {
        off_t offset;
        off_t size;
        int32 logical_block_size;
        int32 session;
        int32 partition;
        char device[256];
    } partition_info

The partition_info structure describes a disk partition, and is used by the `B_GET_PARTITION_INFO` and `B_SET_PARTITION` control commands.

The fields are:

Field| Description  
---|---  
`offset`| Is the offset, in bytes, from the beginning of the disk to the beginning of the partition.  
`size`| Is the size, in bytes, of the partition.  
`logical_block_size`| Is the block size with which the file system was written to the partition.  
`session` and `partition`| Are the session and partition ID numbers for the partition.  
`device`| Is the pathname of the physical device on which the partition is located.

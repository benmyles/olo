<!-- Classes And Methods - The Storage Kit - Defined Types -->

## Defined Types

* * *

### attr_info

Defined In: `kernel/fs_attr.h`
    
    
    typedef struct attr_info
       uint32 type;
       off_t size;
    } attr_info;

The attr_info structure contains the following members

Field| Description  
---|---  
`type`| Is a constant (`B_STRING_TYPE`, `B_INT32_TYPE`, etc) that describes the type of data that the attribute holds.  
`size`| Is the size of the attribute's data, in bytes.  
  
* * *

### stat

Declared in: `posix/sys/stat.h`

The stat structure looks like this:
    
    
    struct stat {
       dev_t         st_dev;
       ino_t         st_ino;
       mode_t        st_mode;
       nlink_t       st_nlink;
       uid_t         st_uid;
       gid_t         st_gid;
       off_t         st_size;
       dev_t         st_rdev;
       size_t        st_blksize;
       time_t        st_atime;
       time_t        st_mtime;
       time_t        st_ctime;
    } stat;

And the fields are…

Field| Description  
---|---  
`st_dev`| Identifies the node's device.  
`st_ino`| Is the node's "inode" number.  
  
By combining `st_dev` and `st_ino` you can roll your own node_ref:
    
    
    node_ref nref;
    stat st;
    
    if (file.GetStat(&st) == B_OK) {
       nref.dev = st.st_dev;
       nref.node = st.st_ino;
    }

Meanwhile…

Field| Description  
---|---  
`st_mode`| Describes the node's flavor: plain file, directory or symbolic link. To test the field, pass it to the `S_ISREG()`, `S_ISDIR()`, and `S_ISLNK()` boolean macros:
    
    
    if (S_ISREG(st.st_mode))
       /* it's a "regular" file */
       else if (S_ISDIR(st.st_mode))
          /* it's a directory */
          else if (S_ISLNK(st.st_mode))
             /* it's a symbolic link */  
  
`st_nlink`| Is the number of "hard links" that point to this node.  
`st_uid` and `st_gid`| Are the user (owner) and group ids that were described in the [`GetOwner()`](BStatable.md#BStatable_GetOwner "GetOwner\(\), SetOwner\(\), GetGroup\(\), SetGroup\(\), GetPermissions\(\), SetPermissions\(\)") function.  
`st_rdev`| Is, well, no one really knows. It's provided for System V compatibility (hold your applause), but it's unused.  
`st_blksize`| Is the "preferred block size" that's used during copying. The cp command line program allocates buffers of this size when it's copying the file's data.  
`st_atime`, `st_mtime`, and `st_ctime`| Are the access, modification, and creation times in seconds since January 1, 1970. Access time (`st_atime`) is currently unused.  
  
* * *

### fs_info

The fs_info structure is defined as:
    
    
    typedef struct fs_info {
        dev_t     dev;
        ino_t     root;
        uint32    flags;
        off_t     block_size;
        off_t     io_size;
        off_t     total_blocks;
        off_t     free_blocks;
        off_t     total_nodes;
        off_t     free_nodes;
        char      device_name[128];
        char      volume_name[B_FILE_NAME_LENGTH];
        char      fsh_name[B_OS_NAME_LENGTH];
    };

The structure's fields are:

  * `dev`. The device number of the device.

  * `root`. The inode of the root of the device.

  * `flags`. Flags describing the device's capabilities.

  * `block_size`. The fundamental block size of the device.

  * `io_size`. Optimal I/O size of the device.

  * `total_blocks`. The total number of blocks on the device.

  * `free_blocks`. The number of free (unused) blocks on the device.

  * `total_nodes`. The total number of nodes on the device.

  * `free_nodes`. The number of free (unused) nodes on the device.

  * `device_name`. Name of the device holding the file system.

  * `volume_name`. Name of the volume contained by the device.

  * `fsh_name`. Name of the file system handler for the device.

The `flags` can be any combination of the following values, which specify the attributes of the file system on the device:

  * `B_FS_IS_READONLY`. The file system on the device is read-only.

  * `B_FS_IS_REMOVABLE`. The device contains removable media.

  * `B_FS_IS_PERSISTENT`. Data written to the device remains even while the device is off.

  * `B_FS_IS_SHARED`. The file system is being shared on a network.

  * `B_FS_HAS_MIME`. The file system supports the MIME typing system used by the BeOS.

  * `B_FS_HAS_ATTR`. The file system supports node attributes.

  * `B_FS_HAS_QUERY`. The file system supports the BeOS query mechanism.

The information in the fs_info structure is guaranteed to be internally consistent, but the structure as a whole should be considered to be out-of-date as soon as you receive it. It provides a picture of a device as it exists just before the info-retrieving function returns. In particular, the number of free blocks and of free nodes can easily change immediately after you receive this information.

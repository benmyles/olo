<!-- Classes And Methods - The Storage Kit - BStatable -->

## BStatable

| Derived From:| None  
---|---  
Mix-in Classes:| None  
Declared In:| `storage/Statable.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BStatable_Overview.md "BStatable")

  * [Member Functions](BStatable.md#BStatable_Member_Functions)

  
  
* * *

### Member Functions

####  GetCreationTime(), SetCreationTime(), GetModificationTime(), SetModificationTime(), GetAccessTime(), SetAccessTime()

`status_t GetCreationTime(time_t* ctime) const;``status_t SetCreationTime(time_t ctime);``status_t GetModificationTime(time_t* mtime) const;``status_t SetModificationTime(time_t mtime);``status_t GetAccessTime(time_t* atime) const;``status_t SetAccessTime(time_t atime);`

Warning

![Warning](./images/admonitions/Stop_32.png)

Access time is currently unused.

These function let you get and set the time at which the item was created, last modified, and last accessed (opened). The measure of time is given as seconds since (the beginning of ) January 1, 1970.

Note

![Note](./images/admonitions/Info_32.png)

The time quanta that `stat` uses is seconds; the rest of the BeOS measures time in microseconds (bigtime_t).

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_NOT_ALLOWED`.| You tried to set a time field for a file on a read-only volume.  
`B_NO_MEMORY`.| Couldn't get the necessary resources to complete the transaction.  
`B_BAD_VALUE`.|  The node doesn't exist (abstract entry).  
  
#### GetNodeRef()

`status_t GetNodeRef(node_ref* nref) const;`

Copies the item's node_ref structure into the `nref` argument, which must be allocated.

Typically, you use an node's node_ref as a key to the Node Monitor by passing the node_ref structure to the [`watch_node()`](The_Node_Monitor.md#watch_node "watch_node\(\)") function. The Node Monitor watches the node for specific changes; see "[The Node Monitor](The_Node_Monitor.md "The Node Monitor")" section of this chapter for details.

As a convenience, you can use a node_ref structure to initialize a [`BDirectory`](BDirectory.md "BDirectory") object (through the constructor or [`BDirectory::SetTo()`](BDirectory.md#BDirectory_SetTo "SetTo\(\), Unset\(\)") function).

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_NO_MEMORY`.|  Couldn't get the necessary resources to complete the transaction.  
`B_BAD_VALUE`.| The node doesn't exist (abstract entry).  
  
####  GetOwner(), SetOwner(), GetGroup(), SetGroup(), GetPermissions(), SetPermissions()

`status_t GetOwner(uid_t* owner) const;``status_t SetOwner(uid_t owner);``status_t GetGroup(gid_t* group) const;``status_t SetGroup(gid_t group);``status_t GetPermissions(mode_t* perms) const;``status_t SetPermissions(mode_t perms);`

These functions set and get the owner, group, and read/write/execute permissions for the node:

  * The `owner` identifier encodes the identity of the user that "owns" the file.

  * The `group` identifier encodes the "group" that is permitted group access to the file (as declared by the permissions).

  * The `permissions` value records nine "permission facts": Whether the file can be read, written, and executed by the node's owner, by users in the node's group, and by everybody else (read/write/execute * owner/group/others = 9 items).

The uid_t, gid_t, and mode_t types used here are standard POSIX types. All three are 32-bit unsigned integers and are defined in `posix/sys/types.h`.

The owner and group encodings must match values found in the system's user and group files (which are as currently unimplemented).

The permissions value is a combination of the following bitfield constants (defined in `posix/sys/stat.h`):

  * `S_IRUSR` owner's read bit.

  * `S_IWUSR` owner's write bit.

  * `S_IXUSR` owner's execute bit.

  * `S_IRGRP` group's read bit.

  * `S_IWGRP` group's write bit.

  * `S_IXGRP` group's execute bit.

  * `S_IROTH` others' read bit.

  * `S_IWOTH` others' write bit.

  * `S_IXOTH` others' execute bit.

For example:
    
    
    /* Is a file readable by everybody? */
    mode_t perms;
    if (node.GetPermissions(&perms) < B_OK)
       /* handle the error... */
    
    if (perms & S_ISROTH)
       // Yes it is
    else
       // No it isn't

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_NOT_ALLOWED`.| You tried to set permissions on a read-only volume.  
`B_BAD_VALUE`.| The node doesn't exist (abstract entry).  
  
#### GetSize()

`status_t GetSize(off_t* size) const;`

Gets the size of the node's data portion (in bytes). Only the "used" portions of the node's file blocks are counted; the amount of storage the node actually requires (i.e. the number of blocks the node consumes) may be larger than the size given here.

The size measurement doesn't include the node's attributes.

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_NO_MEMORY`.| Couldn't get the necessary resources to complete the transaction.  
`B_BAD_VALUE`.| The node doesn't exist (abstract entry).  
  
#### GetStat()

`virtual status_t GetStat(struct stat* st) const;`

`GetStat()` returns the [stat](TheStorageKit_DefinedTypes.md#stat "stat") structure for the node. The structure is copied into the `st` argument, which must be allocated. The `BStatable` object does not cache the stat structure; every time you call `GetStat()`, fresh stat information is retrieved.

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_NO_MEMORY`.| Couldn't get the necessary resources to complete the transaction.  
`B_BAD_VALUE`.| The node doesn't exist (abstract entry).  
  
####  IsFile(), IsDirectory(), IsSymLink()

`bool IsFile() const;``bool IsDirectory() const;``bool IsSymLink() const;`

These functions return true if the node is a plain file, a directory, or a symbolic link, respectively.

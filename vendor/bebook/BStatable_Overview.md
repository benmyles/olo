<!-- System Overview - The Storage Kit - BStatable -->

## BStatable

`BStatable` is a pure abstract class that provides functionality for its two derived class, [`BEntry`](BEntry.md "BEntry") and [`BNode`](BNode.md "BNode"). The `BStatable` functions let you get and set "statistical" information about a node in the file system. You can…

  * Determine whether the node is a file, directory, or symbolic link.

  * Get and set an node's owner, group, and permissions.

  * Get and set the node's creation, modification, and access times.

  * Get the size of the node's data (not counting attributes).

  * Get a [`BVolume`](BVolume.md "BVolume") object for the node's volume.

  * Get the node_ref of the node (and pass it to [`watch_node()`](The_Node_Monitor.md#watch_node "watch_node\(\)"), most likely).

* * *

### Nodes and Entries

Technically, `BStatable` information pertains to nodes, not entries. The fact that [`BEntry`](BEntry.md "BEntry") implements the `BStatable` functions is a (slightly confusing) convenience: When you invoke a `BStatable` function on a [`BEntry`](BEntry.md "BEntry") object, what you're really doing is asking for information about the node that corresponds to the object.

* * *

### Abstract Entries

As explained in [`BEntry`](BEntry.md "BEntry"), it's possible to create "abstract" [`BEntry`](BEntry.md "BEntry") objects; in other words, objects that don't correspond to actual files (nodes) on the disk. You can't get (or set) `BStatable` information for abstract entries. The `BStatable` functions return `B_BAD_VALUE` if the invoked-upon entry is abstract.

* * *

### Relationship to stat()

The `BStatable` functions are covers for the POSIX [`stat()`](TheStorageKit_DefinedTypes.md#stat "stat") call. `stat()` retrieves a file-specific stat structure, which records the statistics listed above (and then some). Although `BStatable` was designed to hide stat details, you can get the [`stat()`](TheStorageKit_DefinedTypes.md#stat "stat") structure through the [`GetStat()`](BStatable.md#BStatable_GetStat "GetStat\(\)") function.

`stat()` is notorious for being expensive. Furthermore, the stat structure is stale as soon as it gets back from the `stat()` call. If you're concerned with efficiency, be aware that every `BStatable` function (the "setters" as well as the "getters") performs a `stat()`. For example, calling [`GetOwner()`](BStatable.md#BStatable_GetOwner "GetOwner\(\), SetOwner\(\), GetGroup\(\), SetGroup\(\), GetPermissions\(\), SetPermissions\(\)") and then [`GetGroup()`](BStatable.md#BStatable_GetGroup) results in two `stat()` calls. If you want to look at lot of fields (within the same stat structure) all at once, you might consider using `BStatable`'s [`GetStat()`](BStatable.md#BStatable_GetStat "GetStat\(\)") function.

As for integrity, `BStatable` info-getting functions are obviously in the same boat as the `stat()` call itself: The retrieved data isn't guaranteed to be in sync with the actual state of the `stat()`'d item.

The [`BDirectory`](BDirectory.md "BDirectory") class also defines a stat-retrieving function that, in some cases, can be more efficient than the [`GetStat()`](BStatable.md#BStatable_GetStat "GetStat\(\)") function defined here:

  * The [`BDirectory::GetStatFor()`](BDirectory.md#BDirectory_GetStatFor "GetStatFor\(\)") function retrieves the stat structure for the node of a named entry within a directory. If you're interested in getting stat information for a series of nodes within the same directory, you should use this function. You have to call it iteratively (once for each named entry), but the accumulation of the iterated calls will be faster than the [`GetStat()`](BStatable.md#BStatable_GetStat "GetStat\(\)") calls made on the analogous [`BEntry`](BEntry.md "BEntry") objects.

* * *

### Accessing Unreadable and Unwritable Entries

`BStatable` isn't thwarted by file permissions: If you can construct a valid [`BEntry`](BEntry.md "BEntry") or [`BNode`](BNode.md "BNode") to an item, then you can invoke any of the info-getting `BStatable` functions on that object:

  * The `BStatable` functions aren't denied even if the node that you're looking at is read-protected. However, you can only invoke the info-setting functions if the node allows writing.

  * Similarly, you can get stat info for a locked node, but you won't be able to write the info (through functions such as [`SetOwner()`](BStatable.md#BStatable_SetOwner)) unless your object holds the lock. See [`BNode`](BNode.md "BNode") for more on locking.

* * *

### Other Details

You rarely set stat information. In practice, you rarely use `BStatable`'s info-setting functions. Setting information such as when a file was created, who owns it, or how big it is, is the responsibility of the system and the privilege of the user. For example, when you [`Write()`](BFile.md#BFile_Write) to a [`BFile`](BFile.md "BFile") object, the system automatically updates the size and modification date for the file.

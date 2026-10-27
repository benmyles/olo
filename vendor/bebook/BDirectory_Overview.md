<!-- System Overview - The Storage Kit - BDirectory -->

## BDirectory

A [`BDirectory`](BDirectory.md "BDirectory") object gives you access to the contents of a directory. A [`BDirectory`](BDirectory.md "BDirectory")'s primary features are:

  * It can iteratively retrieve the entries in the directory. The entries are returned as [`BEntry`](BEntry.md "BEntry") objects, entry_refs, or dirent structures ([`GetNextEntry()`](BDirectory.md#BDirectory_GetNextEntry "GetNextEntry\(\), GetNextRef\(\), GetNextDirents\(\), CountEntries\(\),Rewind\(\)"), [`GetNextRef()`](BDirectory.md#BDirectory_GetNextRef), [`GetNextDirents()`](BDirectory.md#BDirectory_GetNextDirents)).

  * It can find a specific entry. You can ask if the entry exists ([`Contains()`](BDirectory.md#BDirectory_Contains "Contains\(\)")), and you can retrieve the entry as a [`BEntry`](BEntry.md "BEntry") ([`FindEntry()`](BDirectory.md#BDirectory_FindEntry "FindEntry\(\)")).

  * It can create new entries. Through the aptly named [`CreateFile()`](BDirectory.md#BDirectory_CreateFile "CreateFile\(\), CreateDirectory\(\), CreateSymLink\(\)"), [`CreateDirectory()`](BDirectory.md#BDirectory_CreateDirectory) and [`CreateSymLink()`](BDirectory.md#BDirectory_CreateSymLink) functions.

Unlike the other [`BNode`](BNode.md "BNode") classes, a [`BDirectory`](BDirectory.md "BDirectory") knows its own entry ([`GetEntry()`](BDirectory.md#BDirectory_GetEntry "GetEntry\(\)")), and can be initialized with a node_ref structure.

* * *

### Retrieving Entries

The [`BDirectory`](BDirectory.md "BDirectory") functions that let you iterate over a directory's entries are inherited from [`BEntryList`](BEntryList.md "BEntryList"):
    
    
    status_t GetNextEntry(BEntry *entry, bool traverse = true);
    status_t GetNextRef(entry_ref *ref);
    int32 GetNextDirents(dirent *buf, size_t length,
                         int32 count = INT_MAX)

For the basic story on these functions, see the [`BEntryList`](BEntryList.md "BEntryList") class and the function descriptions below. In addition to the info you'll find there, you should be aware of the following:

  * Entries are returned in "directory order". This is, roughly, the ASCII order of their names.

  * Try not to alter the directory while you're getting its entries. Entries are delivered on demand. If you do something to change the contents of the directory while you're iterating through those contents (such as change the name of the file "aaa" to "zzz") you could end up seeing an entry more than once (technically, you'll see the same node under the guise of different entries), or you could miss an entry.

  * Counting entries uses the same iterator that retrieves entries. You mustn't call [`CountEntries()`](BDirectory.md#BDirectory_CountEntries) while you're looping over a `GetNext…()` function.

* * *

### Creating New Directories

To create a new directory, you can use [`BDirectory`](BDirectory.md "BDirectory")'s [`CreateDirectory()`](BDirectory.md#BDirectory_CreateDirectory) function. The function creates a single new directory as identified by its argument. The new directory will be a subdirectory of the invoked-upon [`BDirectory`](BDirectory.md "BDirectory")'s directory.

You can also create an entire path full of new directories through the global [`create_directory()`](BDirectory.md#create_directory "create_directory\(\)") function. This convenient function attempts to create all "missing" directories along the path that you pass in.

* * *

### Finding a Directory

The [`find_directory()`](BDirectory.md#find_directory "find_directory\(\)") function gives you the pathnames for pre-defined directories. These directories, such as those that store Be-supplied applications and user-defined preferences settings, are represented by directory_which constants. These constants are not strings; you can't use them directly. You have to pass them through [`find_directory()`](BDirectory.md#find_directory "find_directory\(\)").

Note that the [`BDirectory`](BDirectory.md "BDirectory") class itself doesn't let you find directories on the basis of the [directory_which](BDirectory.md#Constants_directory_which "directory_which\(\) Constants") constants—you have to use the [`find_directory()`](BDirectory.md#find_directory "find_directory\(\)") function (which is documented at the end of this class description).

* * *

### Node Monitoring a Directory

Note

![Note](./images/admonitions/Info_32.png)

The following description is a brief, directory-specific view into the Node Monitor. For the full story, see "[The Node Monitor](The_Node_Monitor.md "The Node Monitor")" section of this chapter.

You can monitor changes to the contents of a directory by passing a [`BDirectory`](BDirectory.md "BDirectory")'s node_ref and the `B_WATCH_DIRECTORY` flag to the Node Monitor's [`watch_node()`](The_Node_Monitor.md#watch_node "watch_node\(\)") function. As with all invocations of [`watch_node()`](The_Node_Monitor.md#watch_node "watch_node\(\)"), you also have to pass a [`BMessenger`](BMessenger.md "BMessenger") (the "target") that will receive the Node Monitor notifications; here, we use [`be_app_messenger`](BApplication.md#BApplication_be_app_messenger "be_app_messenger"):
    
    
    BDirectory dir("/boot/home");
    node_ref nref;
    status_t err;
    
    if (dir.InitCheck() == B_OK) {
       dir.GetNodeRef(&nref);
       err = watch_node(&nref, B_WATCH_DIRECTORY, be_app_messenger);
       if (err != B_OK)
          /* handle the error */
    }

The following changes to the monitored directory cause [`BMessage`](BMessage.md "BMessage")s to be sent to the target. The `what` field for all Node Monitor messages is `B_NODE_MONITOR`; the `opcode` field (an integer code) describes the activity:

  * An entry was created (`opcode` = `B_ENTRY_CREATED`).

  * An entry was moved to a different name in the same directory (`B_ENTRY_RENAMED`).

  * An entry was from moved from this directory to a different directory, or vice versa (`B_ENTRY_MOVED`).

  * An entry (and the node it represents) was deleted from the file system (`B_ENTRY_REMOVED`).

The `B_WATCH_DIRECTORY` flag (by itself) doesn't monitor changes to the directory's own entry. For example, if you change the name of the directory that you're monitoring, the target isn't sent a message. If you want a [`BDirectory`](BDirectory.md "BDirectory") to watch changes to itself, you have to throw in one of the other Node Monitor flags (`B_WATCH_NAME`, `B_WATCH_STAT`, or `B_WATCH_ATTR`).

The other fields in the Node Monitor message describe the entry that changed. The set of fields depends on the opcode (the following is a summary of the list given in "[Notification Messages](The_Node_Monitor.md#TheNodeMonitor_NotificationMessages "Notification Messages")" in the Node Monitor documentation):

#### B_ENTRY_CREATED

Field| Type| Description  
---|---|---  
`device`| `B_INT32_TYPE`| dev_t of the directory's device.  
`directory`| `B_INT64_TYPE`| ino_t (node number) of the directory.  
`node`| `B_INT64_TYPE`| ino_t of the new entry's node.  
`name`| `B_STRING_TYPE`| The name of the new entry.  
  
#### B_ENTRY_MOVED

The `device`, `node`, and `name` fields are the same as for `B_ENTRY_CREATED`, plus…

Field| Type| Description  
---|---|---  
`from_directory`| `B_INT64_TYPE`| The ino_t number of the old directory.  
`to_directory`| `B_INT64_TYPE`| The ino_t number of the new directory.  
  
#### B_ENTRY_REMOVED

The `B_ENTRY_REMOVED` message takes the same form as `B_ENTRY_CREATED`, but without the `name` field. This, obviously, can be a problem—what good is it if you're told that a file has been removed, but you're not told the file's name? In some cases, simply being told that a file has been removed actually is good enough: You can simply re-read the contents of the directory.

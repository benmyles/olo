<!-- Classes And Methods - The Storage Kit - The Node Monitor -->

## The Node Monitor

Declared in: `storage/NodeMonitor.h`

The Node Monitor is a service that lets you ask to be notified of certain file system changes. You can ask to be told when a change is made to…

  * The contents of a specific directory.

  * The name of a specific entry.

  * Any stat field of a specific entry.

  * Any attribute of a specific entry.

You can also ask to be notified when…

  * Volumes are mounted and unmounted.

Note

![Note](./images/admonitions/Info_32.png)

Volume monitoring is also provided by the [`BVolumeRoster`](BVolumeRoster.md "BVolumeRoster") class: [`BVolumeRoster`](BVolumeRoster.md "BVolumeRoster") can talk to the Node Monitor for you. The [`BVolumeRoster`](BVolumeRoster.md "BVolumeRoster") volume-watching API is more humane than that which you'll find here.

When something interesting happens, the Node Monitor lets you know by sending a [`BMessage`](BMessage.md "BMessage") to the target of your choice.

* * *

### Node Monitor Functions

There are two Node Monitor functions, `watch_node()` and `stop_watching()`. The names are a wee bit misleading, so before we go on to the full technical descriptions, let's nip some buds:

  * [`watch_node()`](The_Node_Monitor.md#watch_node "watch_node\(\)") tells the Node Monitor to start _or stop_ watching a _specific_ node, or to watch for volumes being mounted and unmounted. Memorize the emphasized words.

  * [`stop_watching()`](The_Node_Monitor.md#stop_watching "stop_watching\(\)") tells the Node Monitor to stop sending notifications to a particular target.

#### watch_node()

`status_t watch_node(const node_ref* nref,  
uint32 flags,  
[`BMessenger`](BMessenger.md "BMessenger") messenger);` `status_t watch_node(const node_ref* nref,  
uint32 flags,  
const [`BHandler`](BHandler.md "BHandler")* handler,  
const [`BLooper`](BLooper.md "BLooper")* looper = NULL);`

`watch_node()` tells the Node Monitor to…

  * …start paying attention to the node specified by the [node_ref](BNode.md#node_ref "node_ref") argument. If you're watching for volumes (only), `nref` can be `NULL`. The easiest way to get a [node_ref](BNode.md#node_ref "node_ref") is to invoke [`BStatable::GetNodeRef()`](BStatable.md#BStatable_GetNodeRef "GetNodeRef\(\)") on any [`BEntry`](BEntry.md "BEntry") or [`BNode`](BNode.md "BNode") object.

  * The `flags` argument lists the changes that you want the Monitor to pay attention to. See below for details.

  * The target of the change notification messages is specified either as a [`BMessenger`](BMessenger.md "BMessenger"), or as a [`BHandler`](BHandler.md "BHandler") / [`BLooper`](BLooper.md "BLooper") pair. (The target specification follows the [`BInvoker::SetTarget()`](BInvoker.md#BInvoker_SetTarget "SetTarget\(\), Target\(\), IsTargetLocal\(\), Messenger\(\)") protocol; see the [`BInvoker`](BInvoker.md "BInvoker") class for details.) The notification shows up as a [`BMessage`](BMessage.md "BMessage") in the target's [`MessageReceived()`](BHandler.md#BHandler_MessageReceived "MessageReceived\(\)") function.

Note

![Note](./images/admonitions/Info_32.png)

You can't tell the Node Monitor to send its notifications to another application. Currently, the [`BMessenger`](BMessenger.md "BMessenger") that you specify must identify a target in the caller's team.

Jumping ahead a bit, here's a sample function that tells the Node Monitor to watch for name and attribute changes to a given entry. The Monitor's notifications will be sent to the application's main loop:
    
    
    status_t WatchThis(BEntry *entry)
    {
       node_ref nref;
       entry->GetNodeRef(&nref);
       return (watch_node(&nref,
                B_WATCH_NAME | B_WATCH_ATTR,
                be_app_messenger));
    }

#####  Monitor Flags

`watch_node()`'s `flags` argument is a combination of the following

Constant| Description  
---|---  
`B_WATCH_NAME`| Watches for name changes. This includes moving the node to a different directory, or removing the node altogether.  
`B_WATCH_STAT`| Watches for any change to the node's stat structure. This includes changes to the size, modification date, owner, and so on. See "The stat Structure" in the [`BStatable`](BStatable.md "BStatable") class for a description of what's in the stat structure.  
`B_WATCH_ATTR`| Watches for changes to any of the node's attributes. This includes adding and removing attributes.  
`B_WATCH_DIRECTORY`| Only applies to nodes that are directories. The flag tells the Monitor to watch for changes (new entries, entry deletions, entries being renamed) to the directory. (You can apply the other flags to a directory, as well). It's not an error to set `B_WATCH_DIRECTORY` on a node that isn't a directory—but it doesn't do anything for you.  
`B_WATCH_ALL`.| This is a convenience that combines all the above.  
`B_WATCH_MOUNT`| Watches for volumes being mounted and unmounted. As mentioned above, the `nref` argument isn't needed (it can be `NULL`) if all you're doing is watching volumes. `B_WATCH_MOUNT` isn't included in `B_WATCH_ALL`.  
  
There's one other constant, which lives in a class by itself:

Constant| Description  
---|---  
`B_STOP_WATCHING`| Tells the Node Monitor to stop watching the `nref` argument.  
  
You can't combine `B_STOP_WATCHING` with any of the others in an attempt to stop watching a specific category of changes. For example, if you call…
    
    
    watch_node(&nref, B_WATCH_STAT, be_app_messenger);
    watch_node(&nref, B_WATCH_ATTR, be_app_messenger);

…and then call…
    
    
    watch_node(&nref, B_STOP_WATCHING, be_app_messenger);

…both of the previous Monitor calls are stopped.

Warning

![Warning](./images/admonitions/Stop_32.png)

`B_STOP_WATCHING` does not apply to volume watching. The only way to stop monitoring volume un/mounts is to call `stop_watching()`.

##### Combining Flags and the 4096 Limit

If you can, you should combine as many flags as you're going to need in single calls to `watch_node()`. Recall the example used above:
    
    
    watch_node(&nref,
        B_WATCH_NAME | B_WATCH_ATTR,
        be_app_messenger);

This is better than making separate `watch_node()` calls (one to pass `B_WATCH_NAME` and another to pass `B_WATCH_ATTR`)—not only because the single call is naturally more efficient than two, but also because the Node Monitor can only monitor 4096 nodes per team at a time. Every call to `watch_node()` consumes a Node Monitor slot, even if you're already monitoring the requested node.

If you want to watch all aspects of a node, just pass `B_WATCH_ALL` to every `watch_node()` call. This will consume only a single Node Monitor slot.

##### Notification Messages

A [`BMessage`](BMessage.md "BMessage") notification sent by the Node Monitor looks like this:

  * The `what` value is `B_NODE_MONITOR`.

  * The field named `opcode` is an int32 constant that tells you what happened.

  * Additional fields give you information (device, node, name, and so on) about the node (or volume) that it happened to.

The `opcode` constants and additional fields are described in "[Opcode Constants](The_Node_Monitor.md#TheNodeMonitor_Opcode_Constants "Opcode Constants")." In general, the opcodes correspond to the flags that you passed to `watch_node()`; however, this correspondence isn't always one-to-one.

There are seven opcode constants:

  * `B_ENTRY_CREATED`
  * `B_ENTRY_REMOVED`
  * `B_ENTRY_MOVED`
  * `B_STAT_CHANGED`
  * `B_ATTR_CHANGED`
  * `B_DEVICE_MOUNTED`
  * `B_DEVICE_UNMOUNTED`

Return Code| Description  
---|---  
`B_OK`.| The Node Monitor is off and running.  
`B_BAD_VALUE`.| Bad `nref` argument (not applicable to mount-only watches), or poorly formed target.  
`B_NO_MEMORY`.| Couldn't allocate resources, or out of Node Monitor slots.  
`B_ERROR`.| Some cases of bad `nref` arguments erroneously return `B_ERROR`. This will be fixed.  
  
#### stop_watching()

`status_t ([`BMessenger`](BMessenger.md "BMessenger") messenger);` `status_t (const [`BHandler`](BHandler.md "BHandler")* handler,  
const [`BLooper`](BLooper.md "BLooper")* looper = NULL);`

Tells the Node Monitor to stop sending notifications to the target described by the arguments. All the Node Monitor "slots" that were allocated to the target are freed. Keep in mind that are only 4096 slots for the entire system.

Return Code| Description  
---|---  
`B_OK`.| The target is now out of the Node Monitor loop.  
`B_BAD_VALUE`.| Badly formed target description.  
  
* * *

### Opcode Constants

The following sections describe the "opcode" constants; these are the values that appear in the `opcode` field of the [`BMessage`](BMessage.md "BMessage")s that are generated by the Node Monitor. Note that in these descriptions, the use of the terms "entry" and "node" is sometimes blurred.

Declared in: `storage/NodeMonitor.h`

#### B_ENTRY_CREATED

A completely new entry was created in a monitored directory. (This doesn't include entries that are moved into this directory from some other directory—see [`B_ENTRY_MOVED`](The_Node_Monitor.md#B_ENTRY_MOVED "B_ENTRY_MOVED").)

You get this notification if you applied [`B_WATCH_DIRECTORY`](The_Node_Monitor.md#B_WATCH_DIRECTORY) to the directory in which the entry was created. The message's fields are:

Field| Type code| Description  
---|---|---  
`opcode`| `B_INT32_TYPE`| `B_ENTRY_CREATED`  
`name`| `B_STRING_TYPE`| The name of the new entry.  
`directory`| `B_INT64_TYPE`| The ino_t (node) number for the directory in which the entry was created.  
`device`| `B_INT32_TYPE`| The dev_t number of the device on which the new entry resides.  
`node`| `B_INT64_TYPE`| The ino_t number of the new entry itself. (More accurately, it identifies the node that corresponds to the entry.)  
  
##### Parsing and Tricks

In your code, you would parse a `B_ENTRY_CREATED` message like this:
    
    
    void MyTarget::MessageReceived(BMessage *msg)
    {
       int32 opcode;
       dev_t device;
       ino_t directory;
       ino_t node;
       const char *name;
    
       if (msg->what == B_NODE_MONITOR) {
          if (msg->FindInt32("opcode", &opcode) == B_OK) {
             switch (opcode) {
                case B_ENTRY_CREATED:
                   msg->FindInt32("device", &device);
                   msg->FindInt64("directory", &directory);
                   msg->FindInt64("node", &node);
                   msg->FindString("name", &name);
                   break;
                ...

So, what do you do with these fields?

###### Create an entry_ref to the entry.

The `device`, `directory`, and `name` fields can be used to create an entry_ref to the new entry:
    
    
    entry_ref ref;
    const char *name;
    ...
    msg->FindInt32("device", &ref.device);
    msg->FindInt64("directory", &ref.directory);
    msg->FindString("name", &name);
    ref.set_name(name);

###### Create a node_ref to the entry.

If you want to start Node Monitoring the new entry (or, more accurately, the node of the new entry), you stuff `device` and `directory` into a node_ref:
    
    
    node_ref nref;
    status_t err;
    
    ...
    msg->FindInt32("device", &nref.device);
    msg->FindInt64("node", &nref.node);
    
    err = watch_node(&nref, B_WATCH_ALL, be_app_messenger);

###### Create a node_ref to the entry's parent.

Note that the `directory` field is a node number. By combining this number with the `device` field, you can create a node_ref that points to the entry's parent. From there, you're a [`SetTo()`](BDirectory.md#BDirectory_SetTo "SetTo\(\), Unset\(\)") away from a [`BDirectory`](BDirectory.md "BDirectory") object:
    
    
    node_ref nref;
    BDirectory dir;
    status_t err;
    
    ...
    msg->FindInt32("device", &nref.device);
    msg->FindInt64("directory", &nref.node);
    err = dir.SetTo(&nref);

#### B_ENTRY_REMOVED

A node was removed (deleted) from a directory.

You get this if you applied [`B_WATCH_NAME`](The_Node_Monitor.md#B_WATCH_NAME) on the node itself, or [`B_WATCH_DIRECTORY`](The_Node_Monitor.md#B_WATCH_DIRECTORY) on the directory that the node lived in. The message's fields are:

Field| Type code| Description  
---|---|---  
`opcode`| `B_INT32_TYPE`| `B_ENTRY_REMOVED indicates that an entry was removed.`  
`directory`| `B_INT64_TYPE`| The ino_t (node) number of the directory from which the entry was removed.  
`device`| `B_INT32_TYPE`| The dev_t number of the device that the removed node used to live on.  
`node`| `B_INT64_TYPE`| The ino_t number of the node that was removed.  
  
Note

![Note](./images/admonitions/Info_32.png)

Since this message is telling you that the node was removed, the "node" value will be invalid. The node number can be useful (and sometimes necessary) for comparison with cached node numbers (as demonstrated below).

Parsing the message is the same as for [`B_ENTRY_CREATED`](The_Node_Monitor.md#B_ENTRY_CREATED "B_ENTRY_CREATED"), but without the `name` field. See "[Parsing and Tricks](The_Node_Monitor.md#TheNodeMonitor_ParsingAndTricks "Parsing and Tricks")," above.

Note that the `B_ENTRY_REMOVED` message is sent as soon as the node's entry is "unlinked" from its directory. The node itself may linger for while after that. Follow this logic:

  * When a file (regardless of flavor) is removed, the entry for that file is immediately removed ("unlinked") from the file hierarchy, and the Node Monitor message is immediately sent—even if you have an object that has opened the file's node.

  * The node isn't actually destroyed until the last open object (to that node) is destroyed. (In POSIX speak, the node is destroyed when the last file descriptor to the node is closed.)

  * Until the node is destroyed, the open objects (file descriptors) can still access the node's data.

You can take advantage of this to warn a user that a file is going to go away, or to make a backup, or whatever. For example, let's say you have an application that lets the user open files; each time a file is opened, your `OpenFile()` function creates a [`BFile`](BFile.md "BFile") object and starts the Node Monitor running:
    
    
    status_t YourApp::OpenFile(const char *pathname)
    {
       BFile *file;
       node_ref nref;
       status_t err;
    
       file = new BFile(pathname, B_READ_WRITE);
       if ((err=file->InitCheck()) != B_OK)
          return err;
    
       file->GetNodeRef(&nref);
       err = watch_node(&nref, B_WATCH_NAME, be_app_messenger);
    
       if (err != B_OK) {
          delete file;
          return err;
       }
    
       /* We've got the file and we're monitoring it; now we cache
       * the BFile by adding it to a BList (data member).
       * function. There's a race condition between the
       * watch_node() call above and the following AddItem().
       */
       return ((FileList->AddItem((void *)file)) ? B_OK : B_ERROR);
    
    }

Now we receive a Node Monitor message telling us the node has been removed. We stuff the `device` and `node` fields into a node_ref and pass them to a (fictitious) `AlertUser()` function:
    
    
    void YourApp::MessageReceived(BMessage *msg)
    {
       int32 opcode;
       node_ref nref;
    
       if (msg->what == B_NODE_MONITOR) {
          if (msg->FindInt32("opcode", &opcode) == B_OK) {
             switch (opcode) {
                case B_ENTRY_REMOVED:
                   msg->FindInt32("device", &nref.device);
                   msg->FindInt64("node", &nref.node);
                   GoodbyeFile(nref);
       ...
    }

The implementation of `GoodbyeFile()` (which we won't show here) would walk down the [`BFile`](BFile.md "BFile") list looking for a node_ref that matches the argument:
    
    
    void YourApp::GoodbyeFile(node_ref nref)
    {
       BFile *filePtr;
       int32 ktr = 0;
       node_ref cref;
    
       while ((*filePtr = (BFile *)FileList->ItemAt(ktr++))) {
          filePtr->GetNodeRef(&cref);
          if (nref == cref) {
             /* We found it. Now we do whatever
             * we need to do.
             */
          }
       }
    }

If a match is found, your app could then do whatever it needs to do. Remember—the node's data is still valid until your [`BFile`](BFile.md "BFile") is destroyed or re-initialized.

#### B_ENTRY_MOVED

A node was moved from one directory to a different directory.

You get this if you applied [`B_WATCH_NAME`](The_Node_Monitor.md#B_WATCH_NAME) on the node itself, or [`B_WATCH_DIRECTORY`](The_Node_Monitor.md#B_WATCH_DIRECTORY) on either of the directories. The message's fields are:

Field| Type code| Description  
---|---|---  
`opcode`| `B_INT32_TYPE`| `B_ENTRY_MOVED indicates that an existing entry moved from one directory to another.`  
`name`| `B_STRING_TYPE`| The name of the entry that moved.  
`from directory`| `B_INT64_TYPE`| The ino_t (node) number of the directory from that the node was removed from.  
`to directory`| `B_INT64_TYPE`| The ino_t (node) number of the directory that the node was added to.  
`device`| `B_INT32_TYPE`| The dev_t number of the device that the moved node entry lives on. (You can't move a file between devices, so this value will be apply to the file's old and new locations.)  
`node`| `B_INT64_TYPE`| The ino_t number of the node that moved.  
  
Note

![Note](./images/admonitions/Info_32.png)

Moving a node _does not_ change its ino_t number.

Parsing the message is much the same as for [`B_ENTRY_CREATED`](The_Node_Monitor.md#B_ENTRY_CREATED "B_ENTRY_CREATED"), modulo the `directory` field changes. See "[Parsing and Tricks](The_Node_Monitor.md#TheNodeMonitor_ParsingAndTricks "Parsing and Tricks")"

Moving a node doesn't affect the objects that hold the node open. They (the objects) can continue to read and write data from the node.

#### B_STAT_CHANGED

A field in the node's stat structure changed (this doesn't include the stat structure disappearing because the node was deleted).

You get this if you applied [`B_WATCH_STAT`](The_Node_Monitor.md#B_WATCH_STAT) on the node itself. The message's fields are:

Field| Type code| Description  
---|---|---  
`opcode`| `B_INT32_TYPE`| `B_STAT_CHANGED indicates that some statistic of a node (as recorded in its stat structure) changed.`  
`node`| `B_INT64_TYPE`| The ino_t number of the node.  
`device`| `B_INT32_TYPE`| The dev_t number of the node's device.  
  
The stat structure is described in "The stat Structure" in the [`BStatable`](BStatable.md "BStatable") class. The fields that you can change are:

  * Owner (`st_uid`), group (`st_gid`), and permissions (low four bytes of `st_mode`).

  * Creation (`st_ctime`), modification (`st_mtime`), and access times (`st_atime`; currently unused).

  * The size of the node's data (`st_size`). The measurement doesn't include attributes.

A couple of important points:

  * The `B_STAT_CHANGED` message doesn't give you enough information to construct an object from which you can get a stat structure. In other words, you can't play the same games that were described in "[Parsing and Tricks](The_Node_Monitor.md#TheNodeMonitor_ParsingAndTricks "Parsing and Tricks")."

  * The message also doesn't tell you which stat field changed.

In most uses of the `B_STAT_CHANGED` message, you have to cache the objects that you're monitoring so you can compare their node_refs to the message fields (an example of this is given in [`B_ENTRY_REMOVED`](The_Node_Monitor.md#B_ENTRY_REMOVED "B_ENTRY_REMOVED")). Furthermore, you may want to cache the objects' stat structures so you can figure out which field changed.

#### B_ATTR_CHANGED

An attribute of the node changed.

You get this if you applied [`B_WATCH_ATTR`](The_Node_Monitor.md#B_WATCH_ATTR) on the node itself. The message's fields are:

Field| Type code| Description  
---|---|---  
"opcode"| `B_INT32_TYPE`| `B_ATTR_CHANGED indicates that some attribute of a node changed.`  
"node"| `B_INT64_TYPE`| The ino_t number of the node.  
"device"| `B_INT32_TYPE`| The dev_t number of the node's device.  
  
Attributes are key/value pairs that can be "attached" to any file (regardless of flavor). They're described in the [`BNode`](BNode.md "BNode") class.

As with [`B_STAT_CHANGED`](The_Node_Monitor.md#B_STAT_CHANGED "B_STAT_CHANGED") messages, you may not be able to use the `B_ATTR_CHANGED` information directly. Instead, you have to cache references to the ([`BNode`](BNode.md "BNode")) objects that you're monitoring so you can compare their node_refs to the message fields (an example of this is given in [`B_ENTRY_REMOVED`](The_Node_Monitor.md#B_ENTRY_REMOVED "B_ENTRY_REMOVED")).

#### B_DEVICE_MOUNTED

A file system device (in other words, a volume) was mounted.

You get this if you passed [`B_WATCH_MOUNT`](The_Node_Monitor.md#B_WATCH_MOUNT) to [`watch_node()`](The_Node_Monitor.md#watch_node "watch_node\(\)"). The message's fields are:

Field| Type code| Description  
---|---|---  
"opcode"| `B_INT32_TYPE`| `B_DEVICE_MOUNTED indicates that a new device (or file system volume) has been mounted.`  
"new device"| `B_INT32_TYPE`| The dev_t number of the newly-mounted device.  
"device"| `B_INT32_TYPE`| The dev_t number of the device that holds the directory of the new device's mount point.  
"directory"| `B_INT64_TYPE`| The ino_t (node) number of the directory that acts as the new device's mount point.  
  
Obviously, there's no node involved, here, so the first argument to the [`watch_node()`](The_Node_Monitor.md#watch_node "watch_node\(\)") call can be `NULL`:
    
    
    watch_node(NULL, B_WATCH_MOUNT, be_app_messenger);

Unlike with the other "watch flags," the only way to stop the mount-watching is to call [`stop_watching()`](The_Node_Monitor.md#stop_watching "stop_watching\(\)").

#### B_DEVICE_UNMOUNTED

A file system device (in other words, a volume) was unmounted.

You get this if you passed [`B_WATCH_MOUNT`](The_Node_Monitor.md#B_WATCH_MOUNT) to [`watch_node()`](The_Node_Monitor.md#watch_node "watch_node\(\)"). The message's fields are:

Field| Type code| Description  
---|---|---  
"opcode"| `B_INT32_TYPE`| `B_DEVICE_UNMOUNTED indicates that a device has been unmounted.`  
"device"| `B_INT32_TYPE`| The dev_t number of the unmounted device.  
  
Be careful with the device number: dev_ts are quickly recycled. You should only need this number if you're keeping a list of the dev_ts of all mounted disks and you want to remove the dev_t for this recently-unmounted volume (keeping in mind that a device mounted message bearing this dev_t may arrive in the meantime).

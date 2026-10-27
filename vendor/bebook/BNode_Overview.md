<!-- System Overview - The Storage Kit - BNode -->

## BNode

The `BNode` class gives you access to the data that a file system entry (a file, directory, or symbolic link) contains. There are two parts to this data:

  1. There's the "data portion" itself…

  2. …and then there are the node's attributes.

The content of the data portion depends on the node's flavor:

  * If it's a regular file, the data is whatever it is that the file is meant to contain: ASCII text, binary image or sound data, executable code, and so on. Note that resources (as created by the [`BResources`](BResources.md "BResources") class) are kept in the data portion.

  * If it's a directory, the data is the list of entries that the directory contains.

  * If it's a symbolic link, the data is the path of the "linked-to" file. The path can be absolute or relative.

The content of the attributes, on the other hand, isn't qualified by the node's flavor: Any node can contain any set of attributes.

* * *

### Nodes are Dumb

Keep in mind that the concept of a "node" designates the data parts (data and attributes) of a file (a file, directory, or link). Contrast this with an "entry," which designates the entity's location within the file system: For example, you can write to a "node" (but not an entry), and you can rename an "entry" (but not a node).

This isn't just a conceptual crutch, it's the law: Nodes really don't know where they're located. For example, you can't ask a node for its name, or for the identity of its parent. This has some serious implications, the most important of which is…

  * If you need to store a reference to a file (or directory, or symbolic link), don't store the node—in other words, don't cache the BNode object. Instead, store the information that you used to create the `BNode` (typically, a pathname or entry_ref structure).

Now that we've got that straight, we'll relax the rules a bit:

  * [`BDirectory`](BDirectory.md "BDirectory") objects are node/entry hybrids. A [`BDirectory`](BDirectory.md "BDirectory") does know its own name (and parent, and so on).

This doesn't really change the "store the info" rule. Even if you're dealing exclusively with [`BDirectory`](BDirectory.md "BDirectory") objects, you should keep the generative information around. The primary reason for this is…

#### The "Node Pool" is Limited (File Descriptors)

Every `BNode` object consumes a "file descriptor." Your application can only maintain 256 file descriptors at a time. Because of this limit, you shouldn't keep `BNode`s around that you don't need. Keep in mind that [`BEntry`](BEntry.md "BEntry") objects also consumes file descriptors (one per object).

Note

![Note](./images/admonitions/Info_32.png)

The file descriptor limit will probably be lifted, or at least settable, in a subsequent release. But even then you should be frugal.

* * *

### Derived Classes and their Uses

`BNode` has three derived classes: [`BFile`](BFile.md "BFile"), [`BDirectory`](BDirectory.md "BDirectory"), and [`BSymLink`](BSymLink.md "BSymLink"). The derived classes define functions that let you access the node's data portion in the appropriate style; for example…

  * [`BFile`](BFile.md "BFile") implements [`Read()`](BFile.md#BFile_Read "Read\(\), ReadAt\(\), Write\(\), WriteAt\(\)") and [`Write()`](BFile.md#BFile_Write) functions that let you retrieve arbitrary amounts of data from arbitrary positions in the file.

  * [`BDirectory`](BDirectory.md "BDirectory") implements functions, such as [`GetNextEntry()`](BDirectory.md#BDirectory_GetNextEntry "GetNextEntry\(\), GetNextRef\(\), GetNextDirents\(\), CountEntries\(\),Rewind\(\)") and [`FindEntry()`](BDirectory.md#BDirectory_FindEntry "FindEntry\(\)"), that read entries from the directory.

  * [`BSymLink`](BSymLink.md "BSymLink")'s [`ReadLink()`](BSymLink.md#BSymLink_ReadLink "ReadLink\(\)") returns the pathname that it contains.

If you want to (sensibly) look at a node's data portion, you must create an instance of the appropriate derived class. In other words, if you want to browse a directory, you have to create a [`BDirectory`](BDirectory.md "BDirectory") instance; if you want to write to a file, you create a [`BFile`](BFile.md "BFile").

Be aware that it's not (always) an error to create an instance of the "wrong" derived class; setting a [`BFile`](BFile.md "BFile") to a symbolic link, for example, will traverse the link such that the [`BFile`](BFile.md "BFile") opens the file that the symbolic link is linked to. See the individual derived class specifications for more information.

* * *

### BNode Instances

In practice, you almost always want to create an instance of one of the `BNode`-derived classes; but if, for whatever reason, you find yourself holding a `BNode` instance, here's what you'll be able to do with it:

  * Read and write attributes. The attribute-accessing functions ([`ReadAttr()`](BNode.md#BNode_ReadAttr "ReadAttr\(\), WriteAttr\(\), RemoveAttr\(\)"), [`WriteAttr()`](BNode.md#BNode_WriteAttr), and so on) are general—they work without regard for the node's flavor. Thus, you don't need an instance of a specific derived class to read and write attributes.

  * Get stat information. The [`BStatable`](BStatable.md "BStatable") functions can be invoked on any flavor of node.

  * Lock the node. This prevents other "agents" (other objects, other apps, the user) from accessing reading or writing the node's data and attributes. See "[Node Locking](BNode_Overview.md#BNode_Node_Locking "Node Locking")".

* * *

### Converting a BNode to an Instance of a Derived Class

Note

![Note](./images/admonitions/Info_32.png)

This section describes situations and presents solutions to problems that are a bit esoteric. If you never create direct instances of BNode (and you never have to), then you should skip this and go to "[Node Locking](BNode_Overview.md#BNode_Node_Locking "Node Locking")".

There may be times when you find yourself holding on to a `BNode` (instance) that you want to convert into a [`BFile`](BFile.md "BFile"), [`BDirectory`](BDirectory.md "BDirectory"), or [`BSymLink`](BSymLink.md "BSymLink"). However, you can't go directly from a `BNode` instance to an instance of [`BFile`](BFile.md "BFile"), [`BDirectory`](BDirectory.md "BDirectory"), or [`BSymLink`](BSymLink.md "BSymLink")—you can't tell your `BNode` to "cast itself" as one of its children.

There are solutions, however…

#### Converting to BDirectory

Converting from a `BNode` to a [`BDirectory`](BDirectory.md "BDirectory"), while not transparent, is pretty simple: Grab the node_ref out of the `BNode` and pass it to the [`BDirectory`](BDirectory.md "BDirectory") constructor or [`SetTo()`](BDirectory.md#BDirectory_SetTo "SetTo\(\), Unset\(\)") function. Regard this example function:
    
    
    void Node2Directory(BNode *node, BDirectory *dir)
    {
       node_ref nref;
    
       if (!node || !dir) {
          dir.Unset();
          return;
       }
    
       node.GetNodeRef(&nref);
    
       /* Set the BDirectory. If nref isn't a directory node,
       * the SetTo() will fail.
       */
       dir.SetTo(&nref);
    }

#### Converting to BFile or BSymLink

Converting a `BNode` instance to a [`BFile`](BFile.md "BFile") or [`BSymLink`](BSymLink.md "BSymLink") isn't as neat as the foregoing. Instead, you have to cache the information that you used to initialize the `BNode` in the first place, and then reuse it to create the [`BFile`](BFile.md "BFile") or [`BSymLink`](BSymLink.md "BSymLink").

For example, let's say you receive an entry_ref. You turn it into a `BNode`, but then decide you need the data-writing power of a [`BFile`](BFile.md "BFile"). If, in the meantime, you lost the original entry_ref, you're sunk—there's nothing you can do.

* * *

### Node Locking

Another feature provided by the `BNode` class is "node locking": Through `BNode`'s [`Lock()`](BNode.md#BNode_Lock "Lock\(\), Unlock\(\)") function you can restrict access to the node. The lock is removed when [`Unlock()`](BNode.md#BNode_Unlock) is called, or when the `BNode` object is deleted.

  * When you lock a node, you prevent other objects (or agents) from reading or writing the node's data and attributes. No other agent can even open the node—other `BNode` constructions and POSIX `open()` calls (on that node) will fail while you hold the lock.

  * You can only acquire a node lock if there are no file descriptors open on the node (with one exception). This means that no other `BNode` may be open on the node (locked or not), nor may the node be held open because of a POSIX `open()` (or `opendir()`) call.

The one exception to the no-file descriptors rule has to do with [`BEntry`](BEntry.md "BEntry")s: Let's say you lock a directory, and then you initialize a [`BEntry`](BEntry.md "BEntry") to point to an entry within that directory. Even though the [`BEntry`](BEntry.md "BEntry") creates a file descriptor to the directory (as explained in the [`BEntry`](BEntry.md "BEntry") class), the initialization will succeed.

#### Implications

For files (and, less importantly, symlinks), the implications of locking are pretty clear: No one else can read or write the file. For directories, it's worth a closer look:

  * Locking a directory means that the contents of the directory can't change: You can't create new nodes in the directory, or rename or remove existing ones. (You can, however, create abstract entries within the directory; see [`BEntry`](BEntry.md "BEntry") for more on abstract entries.)

Locking a node does not lock the node's entry: You can't "lock out" entry operations, such as rename, move, and remove. Even if you have a node locked, the entry that acts as the "container" for that node could disappear. If you want to prevent such operations on a node's entry, lock the entry's parent directory.

In general, you should try to avoid locking your nodes. If you must lock, try to make it brief. The primary reason (and, pretty much, the only reason) to lock is if separate elements in the data and/or attributes must be kept in a consistent state. In such a case, you should hold the lock just long enough to ensure consistency.

Warning

![Warning](./images/admonitions/Stop_32.png)

You shouldn't use locks to "privatize" data. Locking isn't meant to be used as a heightened permissions bit.

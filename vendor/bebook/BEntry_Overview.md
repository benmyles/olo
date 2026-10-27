<!-- System Overview - The Storage Kit - BEntry -->

## BEntry

The `BEntry` class defines objects that represent "locations" in the file system hierarchy. Each location (or entry) is given as a name within a directory. For example, when you create a `BEntry` thus…
    
    
    BEntry entry("/boot/home/fido");

…you're telling the `BEntry` object to represent the location of the file called `fido` within the directory `/boot/home`.

A `BEntry` doesn't care whether the entry you tell it to represent is a plain file, a directory, or a symbolic link—it doesn't even care if the entry even exists (but we'll get to that later in "[Abstract Entries](BEntry_Overview.md#BEntry_Abstract_Entries "Abstract Entries")"):

  * All the `BEntry` cares about is a name in a directory.

The most important implication of this is the object's attitude towards data. `BEntry`s don't know how to operate on data. You can't use a `BEntry` to read or write a file's data or attributes. For data operations, you have to turn your `BEntry` into a [`BNode`](BNode.md "BNode").

Nonetheless, it's often convenient to speak of a `BEntry` as having data; for example, the phrase "the entry's data" really means "the data that lies in the file that's located by the entry."

* * *

### Talents and Abilities

A properly initialized `BEntry` object (we'll get to the rules of initialization later) knows the following:

  * Location info. A `BEntry` knows its own (leaf) name ([`GetName()`](BEntry.md#BEntry_GetName "GetName\(\), GetPath\(\)")), its full pathname ([`GetPath()`](BEntry.md#BEntry_GetPath)), and the identity of its parent directory ([`GetParent()`](BEntry.md#BEntry_GetParent "GetParent\(\)")).

  * [`BStatable`](BStatable.md "BStatable") info. As a descendant of [`BStatable`](BStatable.md "BStatable"), a `BEntry` can return statistical information about the entry's data—its size, creation date, owner, and so on.

  * entry_ref identifier. A `BEntry` can return the entry_ref that globally identifies the entry ([`GetRef()`](BEntry.md#BEntry_GetRef "GetRef\(\)")).

A `BEntry` can do these things:

  * Perform hierarchical operations. A `BEntry` can change the name of its entry ([`Rename()`](BEntry.md#BEntry_Rename "Rename\(\), MoveTo\(\)")), move it to another directory ([`MoveTo()`](BEntry.md#BEntry_MoveTo)), and remove it from the file hierarchy ([`Remove()`](BEntry.md#BEntry_Remove "Remove\(\)")).

  * Initialize [`BNode`](BNode.md "BNode") objects. The constructors and [`SetTo()`](BEntry.md#BEntry_SetTo) initializers for [`BNode`](BNode.md "BNode") and its children ([`BFile`](BFile.md "BFile"), [`BDirectory`](BDirectory.md "BDirectory"), and [`BSymLink`](BSymLink.md "BSymLink")) accept `BEntry` arguments.

As mentioned above, the most important thing that a `BEntry` can't do is access its own data: A `BEntry` can't read or write data or attributes. To do these things you need a [`BNode`](BNode.md "BNode") object.

(Actually, this isn't entirely true: A `BEntry` can set the size of its data through the [`BStatable::SetSize()`]() function. The function only works on plain files.)

* * *

### Initializing and Traversing

To initialize a `BEntry`, you have to tell it which entry to represent; in other words, you have to identify a directory and a name. You can initialize a `BEntry` object directly…

  * during construction,

  * through the [`SetTo()`](BEntry.md#BEntry_SetTo) function,

  * or through the assignment operator.

Or you can have some other object initialize your `BEntry` for you, by passing the `BEntry` as an argument to…

  * [`BDirectory`](BDirectory.md "BDirectory")'s [`FindEntry()`](BDirectory.md#BDirectory_FindEntry "FindEntry\(\)") or [`GetEntry()`](BDirectory.md#BDirectory_GetEntry "GetEntry\(\)") function,

  * [`BEntryList`](BEntryList.md "BEntryList")'s [`GetNextEntry()`](BEntryList.md#BEntryList_GetNextEntry "GetNextEntry\(\), GetNextRef\(\), GetNextDirents\(\)") function (implemented by [`BDirectory`](BDirectory.md "BDirectory") and [`BQuery`](BQuery.md "BQuery")).

  * `BEntry`'s [`GetParent()`](BEntry.md#BEntry_GetParent "GetParent\(\)") function.

In all cases (except the assignment operator) you're asked if you want to "traverse" the entry during initialization. Traversal is used to "resolve" symbolic links:

  * If you traverse: The `BEntry` will point to the entry that the symbolic link is linked to.

  * If you don't traverse: The `BEntry` will point to the symbolic link itself.

For example, let's say `/boot/home/fidoLink` is linked to `/fido`, to wit:
    
    
    $ cd /boot/home
    $ ln -s ./fido fidoLink
    

Now let's make a traversed `BEntry` for `fidoLink`:
    
    
    /* The second argument is the traversal bool. */
    BEntry entry("/boot/home/fidoLink", true);

If we ask for the entry's pathname…
    
    
    BPath path;
    entry.GetPath(&path);
    printf("Pathname: %sn", path.Path());

…we see
    
    
    Pathname: /boot/home/fido
    

In other words, the `BEntry` refers to `fido`, not `fidoLink`.

Traversal resolves nested links—it really wants to find a "real" file (or directory). If the entry that you're initializing to isn't a link, then the traversal flag is ignored.

#### When to Traverse

When should you traverse, and when not? Here are a few rules of thumbs:

  * If somebody hands you a file reference—if your app gets a [`RefsReceived()`](BApplication.md#BApplication_RefsReceived "RefsReceived\(\)") message—then you probably want to traverse the entry.

  * If you're pawing over the contents of a directory (through [`BDirectory`](BDirectory.md "BDirectory")'s [`GetNextEntry()`](BDirectory.md#BDirectory_GetNextEntry "GetNextEntry\(\), GetNextRef\(\), GetNextDirents\(\), CountEntries\(\),Rewind\(\)")), then you probably don't want to traverse.

  * If you're looking at the result of a query (through [`BQuery`](BQuery.md "BQuery")'s [`GetNextEntry()`](BQuery.md#BQuery_GetNextEntry "GetNextEntry\(\), GetNextRef\(\), GetNextDirents\(\)")), then you almost certainly don't want to traverse. The query finds entries that satisfy certain criteria; if a symbolic link is in the list, it's because the link itself was a winner. If the linked-to file is also a winner, it will show up on its own.

#### Traverso Post Facto

Let's say you create a `BEntry` (to a symlink) without traversing, but then you decide that you do want to resolve the link. Unfortunately, you can't resolve in-place; instead, you have to initialize another `BEntry` using info (entry_ref or pathname) that you get from the link entry:
    
    
    BEntry entry1("/boot/home/fidoLink", false);
    BEntry entry2;
    entry_ref ref;
    
    /* First we check to see if it's a link. */
    if (entry1.IsSymLink()) {
       /* Get the link's entry_ref... */
       entry1.GetRef(&ref);
    
       /* ...and use it to initialize the other BEntry. */
       entry2.SetTo(&ref, true);
    }

* * *

### Abstract Entries

As we all should know by now, a `BEntry` identifies a name within a specific directory. The directory that a `BEntry` identifies must exist, but the entry that corresponds to the name doesn't have to. In other words…

  * A `BEntry` can represent a file that doesn't exist. The entry is said to be "abstract."

For example, the following construction creates a `BEntry` object based on a [`BDirectory`](BDirectory.md "BDirectory") and a name:
    
    
    BEntry entry(someDir, "myFile.h");

Let's assume that `myFile.h` doesn't exist. As long as the directory that's referred to by `someDir` does exist, then the construction is legal. Some of the `BEntry` functions (those inherited from [`BStatable`](BStatable.md "BStatable"), for instance) won't work, but the object itself is valid.

But validity doesn't equal existence:

  * [`SetTo()`](BEntry.md#BEntry_SetTo) and [`InitCheck()`](BEntry.md#BEntry_InitCheck "InitCheck\(\)") _do not_ tell you if a `BEntry`'s entry actually exists. Don't be confused; a return value of `B_OK` simply means the object is valid.

If you want to know if a `BEntry`'s entry actually exists, use the [`Exists()`](BEntry.md#BEntry_Exists "Exists\(\)") function.

#### Creating a File From an Abstract Entry

To turn an abstract `BEntry` into a real entry (or, more accurately, a real node), you have to specify the flavor of node that you want. There are two methods for creating a node; the first is general, the second applies to plain files only.

##### The General Approach.

[`BDirectory`](BDirectory.md "BDirectory")'s [`CreateFile()`](BDirectory.md#BDirectory_CreateFile "CreateFile\(\), CreateDirectory\(\), CreateSymLink\(\)"), [`CreateDirectory()`](BDirectory.md#BDirectory_CreateDirectory), [`CreateSymLink()`](BDirectory.md#BDirectory_CreateSymLink) functions create nodes of the designated flavor. The functions don't take `BEntry` arguments directly; instead, you invoke the functions on the `BEntry`'s directory, passing the entry's leaf name as an argument. Here we turn an abstract entry (`entry`) into a directory:
    
    
    BPath path;
    char name[B_FILE_NAME_LENGTH]; /* A buffer for the name. */
    BDirectory parent; /* The parent of our entry. */
    BDirectory target_dir; /* The product of the transformation. */
    
    if (!entry.Exists()) {
       entry.GetParent(&path);
       entry.GetName(name);
       parent.SetTo(&path);
       parent.CreateDirectory(name, &dir);
    }

##### The Plain-File-Only Approach.

You can create a plain file by passing the `BEntry` to the [`BFile`](BFile.md "BFile") constructor or [`SetTo()`](BFile.md#BFile_SetTo "SetTo\(\), Unset\(\)") function. To do this, you also have to add `B_CREATE_FILE` to the "open mode" flags:
    
    
    BFile file;
    
    if (!entry.Exists())
       file.SetTo(&entry, B_CREATE_FILE|B_READ_WRITE);

* * *

### Subtleties and Details

The following details understand you should, particularly if you want to participate in bedevtalk.

#### File Descriptors

Although it's not intuitively obvious, a `BEntry` object does consume a file descriptor. The file descriptor is opened on the entry's directory.

Your app has a limited number of file descriptors (currently 128, max), so you may not want to cache `BEntry` objects as your primary means for identifying an entry. If you're going to be dealing with a lot of entries and you want to keep track of them all, it's better to cache entry_ref structures or [`BPath`](BPath.md "BPath") objects.

#### Directories are Persistent, Names Are Not

One more time: A `BEntry` identifies an entry as a name in a directory. As described above, the directory is maintained internally as a file descriptor; the name is simply a string. This means that…

  * The directory for a given BEntry is persistent. If you move the directory, the file descriptor, and so the BEntry, moves with it.

  * The name isn't persistent. If the user renames the leaf that a BEntry is pointing to, the BEntry will become abstract.

For example, take the following `BEntry`…
    
    
    BEntry entry("/boot/home/lbj/footFetish.jpeg");

If the user moves the directory…
    
    
    $ cd /boot/home
    $ mv lbj jfk
    

The `BEntry` (`entry`) "moves" with the directory. If you print the pathname and ask if the `BEntry`'s entry exists…
    
    
    BPath path;
    entry.GetPath(&path);
    printf("> Foot movie: %sn", path.Path());
    printf("> Exists? %sn", entry.Exists()?"Oui":"Non");

…you'll see this:
    
    
    > Foot movie: /boot/home/jfk/footFetish.jpeg
    > Exists? Oui
    

The same isn't so for the name portion of a `BEntry`. If the user now moves `footFetish.jpeg`…
    
    
    $ cd /boot/home/jfk
    $ mv footFetish.jpeg hammerToe.jpeg
    

…your `BEntry` will not follow the file (it doesn't "follow the data"). The object will still represent the entry called `footFetish.jpeg`. The `BEntry` will, in this case, become abstract.

Don't be confused: The `BEntry` only "loses track" of a renamed entry if the name change is made behind the object's back. Manipulating the entry name through the `BEntry` object's [`Rename()`](BEntry.md#BEntry_Rename "Rename\(\), MoveTo\(\)") function (for example), doesn't baffle the object. For example:
    
    
    BPath path;
    BEntry entry("/boot/home/lbj/footFetish.jpeg");
    
    entry.Rename("hammerToe.jpeg");
    entry.GetPath(&path);
    printf("> Foot movie: %sn", path.Path());
    printf("> Exists? %sn", entry.Exists()?"Oui":"Non");
    

…and we see…
    
    
    > Foot movie: /boot/home/lbj/hammerToe.jpeg
    > Exists? Oui
    

#### BEntries and Locked Nodes

You can't lock an entry, but you can lock the entry's node (through [`BNode`](BNode.md "BNode")'s [`Lock()`](BNode.md#BNode_Lock "Lock\(\), Unlock\(\)") function). Initializing a `BEntry` to point to a locked node is permitted, but the entry's directory must not be locked. If the directory is locked, the `BEntry` constructor and [`SetTo()`](BEntry.md#BEntry_SetTo) function fail and set [`InitCheck()`](BEntry.md#BEntry_InitCheck "InitCheck\(\)") to `B_BUSY`.

Furthermore, the destination directories in `BEntry`'s [`Rename()`](BEntry.md#BEntry_Rename "Rename\(\), MoveTo\(\)") and [`MoveTo()`](BEntry.md#BEntry_MoveTo) must be unlocked for the functions to succeed. And all directories in the path to the entry must be unlocked for [`GetPath()`](BEntry.md#BEntry_GetPath) to succeed.

If you get a `B_BUSY` error, you may want to try again—it's strongly advised that locks be held as briefly as possible.

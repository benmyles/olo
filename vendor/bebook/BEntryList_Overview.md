<!-- System Overview - The Storage Kit - BEntryList -->

## BEntryList

`BEntryList` is a pure abstract class that defines the protocol for iterating over a set of file system entries. Each derived class must figure out how to create (or "discover") the entry list in the first place: `BEntryList` only supplies functions for getting entries out of the list, it doesn't let you put them in. The `BEntryList` class has two derived classes: [`BDirectory`](BDirectory.md "BDirectory") and [`BQuery`](BQuery.md "BQuery").

At the heart of the `BEntryList` class are the three `GetNext…()` functions, which let you retrieve the entries as…

  * [`BEntry`](BEntry.md "BEntry") objects ([`GetNextEntry()`](BEntryList.md#BEntryList_GetNextEntry "GetNextEntry\(\), GetNextRef\(\), GetNextDirents\(\)")),

  * entry_ref structures ([`GetNextRef()`](BEntryList.md#BEntryList_GetNextRef)),

  * or dirent ("directory entry") structures ([`GetNextDirents()`](BEntryList.md#BEntryList_GetNextDirents)).

You call these functions iteratively; each call gets the "next" entry (or set of entries in the case of [`GetNextDirents()`](BEntryList.md#BEntryList_GetNextDirents)). You check the `GetNext…()` return value to detect the end of the list:

  * For [`GetNextEntry()`](BEntryList.md#BEntryList_GetNextEntry "GetNextEntry\(\), GetNextRef\(\), GetNextDirents\(\)") and [`GetNextRef()`](BEntryList.md#BEntryList_GetNextRef), `B_ENTRY_NOT_FOUND` indicates that there are no more entries to get.

  * [`GetNextDirents()`](BEntryList.md#BEntryList_GetNextDirents) returns 0 when it's at the end of the list.

To get back to the top of an entry list, you call [`Rewind()`](BEntryList.md#BEntryList_Rewind "Rewind\(\)"), but note…

Warning

![Warning](./images/admonitions/Stop_32.png)

[`Rewind()`](BEntryList.md#BEntryList_Rewind "Rewind\(\)") applies to [`BDirectory`](BDirectory.md "BDirectory")s only. You can't rewind a [`BQuery`](BQuery.md "BQuery")'s entry list.

Here's an example of an iteration over all the entries in a [`BDirectory`](BDirectory.md "BDirectory"), retrieved as [`BEntry`](BEntry.md "BEntry") objects:
    
    
    BDirectory dir("/boot/home/fido");
    BEntry entry;
    
    dir.Rewind();
    while (dir.GetNextEntry(&entry) == B_OK)
       /* do something with entry here. */

The final `BEntryList` function, [`CountEntries()`](BEntryList.md#BEntryList_CountEntries "CountEntries\(\)"), also only applies to [`BDirectory`](BDirectory.md "BDirectory")s; but even there you shouldn't depend on it. The count is stale as soon as [`CountEntries()`](BEntryList.md#BEntryList_CountEntries "CountEntries\(\)") returns. The user could create a new file or delete a file in the directory while you're iterating over the entries. Also, [`CountEntries()`](BEntryList.md#BEntryList_CountEntries "CountEntries\(\)") shares the entry list pointer with the `GetNext…()` functions. You mustn't intermingle a [`CountEntries()`](BEntryList.md#BEntryList_CountEntries "CountEntries\(\)") call within your `GetNext…()` loop.

One more [`BDirectory`](BDirectory.md "BDirectory") wrinkle:

  * Entries are retrieved in "directory order". (This is a POSIX term that means, roughly, ASCII order.) If the user renames a file while you're iterating over the directory, it's possible that the file won't be seen, or will show up under its old name and its new name.

* * *

### The Entry List Pointer

Each `BEntryList` object has a single iterator pointer that's shared by all three `GetNext…()` formats (and [`CountEntries()`](BEntryList.md#BEntryList_CountEntries "CountEntries\(\)")). Thus, each successive call to a `GetNext…()` function gets the next entry, regardless of the format. For example:
    
    
    BEntry entry;
    entry_ref ref;
    
    dir.GetNextEntry(&entry);
    dir.GetNextRef(&ref);

Here, `entry` represents the first entry in the directory, and `ref` represents the second entry.

* * *

### Multiple Retrieval

[`GetNextDirents()`](BEntryList.md#BEntryList_GetNextDirents) is different from the other two flavors in that it can retrieve more than one entry at a time. Or it will, someday; currently [`GetNextDirents()`](BEntryList.md#BEntryList_GetNextDirents) retrieves only one entry at a time, no matter how many you ask for.

* * *

### Choosing an Iterator

So, which flavor of `GetNext…()` should you use? Here's how they compare:

  * [`GetNextDirents()`](BEntryList.md#BEntryList_GetNextDirents) is by far the fastest (even in the current one-struct-at-a-time version), but it's also the least wieldy—the protocol isn't nearly as nice as the other two functions. The dirent structure, while jam-packed with fun facts, usually has to be turned into other structures (node_refs or entry_refs) in order to be useful.

  * [`GetNextRef()`](BEntryList.md#BEntryList_GetNextRef) is slower, but the entry_ref structure can be immediately usable (or, at least, cachable). Nonetheless, you're still a step away from a "real" object.

  * [`GetNextEntry()`](BEntryList.md#BEntryList_GetNextEntry "GetNextEntry\(\), GetNextRef\(\), GetNextDirents\(\)") is the slowest, but at least it hands you an object that you can sink your teeth into.

The actual timing numbers depend on your machine, the class that you're invoking the functions through, and some other factors. But the difference is (ahem) significant: [`GetNextDirents()`](BEntryList.md#BEntryList_GetNextDirents) is about an order of magnitude faster than [`GetNextEntry()`](BEntryList.md#BEntryList_GetNextEntry "GetNextEntry\(\), GetNextRef\(\), GetNextDirents\(\)"), with [`GetNextRef()`](BEntryList.md#BEntryList_GetNextRef) right about in the middle.

If, for example, you're simply compiling a list of leaf names, you should certainly use [`GetNextDirents()`](BEntryList.md#BEntryList_GetNextDirents) (painful though it may be). But if you plan on actually doing something with each and every entry that you retrieve, then bite the bullet: Use [`GetNextEntry()`](BEntryList.md#BEntryList_GetNextEntry "GetNextEntry\(\), GetNextRef\(\), GetNextDirents\(\)").

* * *

### The dirent Structure and GetNextDirents()

Of the three iterator functions, [`GetNextDirents()`](BEntryList.md#BEntryList_GetNextDirents) needs some explanation. The dirent structure, which is what the function returns, describes aspects of the retrieved entry:
    
    
    typedef struct dirent {
       dev_t d_dev;
       ino_t d_ino;
       dev_t d_pdev;
       ino_t d_pino;
       unsigned short d_reclen;
       char d_name[1];
    } dirent;

The fields are:

  * `d_dev` is a device id that identifies the device (file system) on which this entry lies.

  * `d_ino` is the node number for this entry's node.

  * `d_pdev` and `d_pino` are the device and inode numbers for the parent directory.

  * `d_reclen` is the length of this dirent structure. The length is variable because…

  * `d_name` is a buffer that's allocated to hold the (`NULL`-terminated) name of this entry.

So—let's pretend we've retrieved a dirent and we want to do something with it. In addition to looking at individual fields, we can combine some of them to make other structures:

  * `d_dev` \+ `d_ino` = node_ref of the entry's node

  * `d_pdev` \+ `d_pino` = node_ref of the parent directory

  * `d_pdev` \+ `d_pino` \+ `d_name` = entry_ref for the entry

In code:
    
    
    dirent* dent;
    entry_ref ref;
    node_ref nref;
    node_ref pnref;
    
    /* Allocate and fill the dirent here... */
    ...
    
    /* Make a node_ref to this entry's node. */
    nref.device = dirent->d_dev;
    nref.node = dirent->d_ino;
    
    /* Make a node_ref to this entry's parent. */
    pnref.device = dirent->d_pdev;
    pnref.node = dirent->d_pino;
    
    /* Make an entry_ref to this entry. */
    ref.device = dirent->d_pdev;
    ref.directory = dirent->d_pino;
    ref.set_name(dirent->d_name);

Where you go from here is a simple matter of programming. Me? I'm going to lunch.

#### Getting a dirent

Now that we know what to do with a dirent, let's see how to get one. The [`GetNextDirents()`](BEntryList.md#BEntryList_GetNextDirents) protocol looks like this:

`int32 GetNextDirents(dirent* buf,  
size_t bufsize,  
int32 count = INT_MAX);`

By default, the function stuffs as many dirent structs as it can into the first `bufsize` bytes of `buf`. These structures represent the next N entries in the entry list. The `count` argument lets you set a limit to the number of structures that you want to be retrieved at a time. The function returns the number of structures that it actually got.

Warning

![Warning](./images/admonitions/Stop_32.png)

Keep in mind that currently [`GetNextDirents()`](BEntryList.md#BEntryList_GetNextDirents) can only read one dirent at a time, regardless of the size of `buf`, or the value of `count`.

Let's try it. For the purposes of this example, we'll convert each dirent into an entry_ref, as described in the previous section.
    
    
    /* This is the buffer that we'll stuff structures into. */
    char buf[4096];
    dirent* dent;
    entry_ref ref;
    
    /* We'll assume dir is a valid BDirectory object. */
    while ((count = dir.GetNextDirents((dirent*)buf, 4096)) > 0) {
       dent = (dirent*)buf;
    
       /* Now we step through the dirents. */
       while (count-- > 0) {
          ref.device = dent->d_pdev;
          ref.directory = dent->d_pino;
          ref.set_name(dent->d_name);
    
          /* Do something with the ref. */
          ...
    
          /* Bump the pointer. */
          dent = (dirent*)((char*)dent + dent->d_reclen);
       }
    }

Remember, the structure is variable length—you have to increment the pointer by hand, as shown here.

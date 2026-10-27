<!-- System Overview - The Storage Kit - BSymLink -->

## BSymLink  
  
A "symbolic link" or symlink is a file that "points to" some other entry. The pointed-to (or, better, "linked-to") entry can be a plain file, directory, or another symlink (which links to yet another entry, and so on). Furthermore, the entry can be abstract—you can create a symlink to an entry that doesn't exist.

The data in a symlink is the pathname to the linked-to entry. The pathname can be absolute or relative. If it's relative, the linked-to entry is found by reckoning the pathname of off the directory in which the symlink lives. Relative pathnames can contain "." and ".." components.

The thing to keep in mind, when dealing with symlinks, is that they link to entries, not nodes. If you link a symlink to an (existing) entry named `/boot/home/fido` and then the user moves fido to rover (or deletes fido), the symlink is not updated. It will still link to `/boot/home/fido`.

Furthermore, symlinks that contain relative pathnames have a further "problem": Let's say you create a symlink in `/boot/home` that links to `fido`. If you move the symlink to some other directory, it will link to the entry named `fido` in the new directory.

The `BSymLink` class creates objects that know how to read a symlink's data. The class does not create new symlinks; to create a symlink, you use [`BDirectory`](BDirectory.md "BDirectory")'s [`CreateSymLink()`](BDirectory.md#BDirectory_CreateSymLink) function.

Note

![Note](./images/admonitions/Info_32.png)

`BSymLink` objects are no smarter than the symlinks files themselves. For example, `BSymLinks` can't resolve the fido/rover "problem".

The only really useful `BSymLink` function is [`ReadLink()`](BSymLink.md#BSymLink_ReadLink "ReadLink\(\)"). This function returns the data that the symlink contains. The other functions are convenient, but they're not essential.

* * *

### Initialization and File Descriptors

When you initialize a `BSymLink` object, you pass in a pathname or entry_ref (or whatever) that refers to an existing symlink. The `BSymLink` object then represents that symlink—it doesn't represent the (node of the) linked-to entry. Furthermore, you can't ask a `BSymLink` to "resolve itself"—it can't pass you back a [`BEntry`](BEntry.md "BEntry") object that represents the linked-to entry.

If you want the [`BEntry`](BEntry.md "BEntry") of the linked-to entry, simply initialize a [`BEntry`](BEntry.md "BEntry") object with the ref (or whatever) to the symlink and tell it to traverse (set the trailing argument to `true`).

For example, in the following code, `link` is a `BSymLink` to the symlink `/boot/home/fidoLink` and `entry` is a [`BEntry`](BEntry.md "BEntry") to the entry that the symlink links-to:
    
    
    BSymLink link("/boot/home/fidoLink");
    BEntry entry("/boot/home/fidoLink", true);

Like all nodes, `BSymLink` allocates a file descriptor. Remember, this is a file descriptor that's open on the symlink node itself, not the (node of the) linked-to entry.

<!-- System Overview - The Storage Kit - Entries And Nodes -->

## Entries And Nodes

The most important concept that you should keep in mind when you're using the Storage Kit is that a file is considered both an entry and a node:

  * The entry part of a file is its location in the file hierarchy. An entry is similar to a pathname: It tells you where a file is (or should be), but it doesn't let you look at its contents.

  * The node part of a file is its data. A node is an actual "thing" that's separate from the file's entry—when you rename a file, for example, all you're doing is tagging the node with a different pathname (or, in our lingo, you move the node from one entry to another). Just as entries don't know about data, nodes don't know anything about entries: A node doesn't know where its entry is located.

This concept really isn't new: If you're familiar with POSIX, then you've already dealt with entries and nodes, except you called them pathnames and file descriptors.

* * *

### Entries

In the Storage Kit, entries are represented three ways:

  * As pathnames,

  * as entry_ref structures,

  * and as [`BEntry`](BEntry.md "BEntry") objects.

Any entry can be given by any of these representations. Furthermore, the representations are fairly easily converted: Given an entry_ref, it's trivial to get a [`BEntry`](BEntry.md "BEntry"), from which you can easily get a pathname, which can be turned into an entry_ref. Which representation you use depends on what you're doing:

  * You use pathnames or entry_refs to keep track of the entries you're interested in.

  * You use [`BEntry`](BEntry.md "BEntry") objects to query and manipulate the entries. For example, if you want to know if an entry is a directory or a file, you need a [`BEntry`](BEntry.md "BEntry") object.

Note

![Note](./images/admonitions/Info_32.png)

An entry_ref or [`BEntry`](BEntry.md "BEntry") isn't necessarily valid across reboots; don't archive these and expect them to work when the system is restarted. Instead, you should save pathnames (either complete or partial, depending on your specific needs; when possible, you should use the directory constants used by the [`find_directory()`](BDirectory.md#find_directory "find_directory\(\)") function).

* * *

### Nodes

Nodes are represented in two ways:

  * As node_ref structures,

  * and as [`BNode`](BNode.md "BNode") objects.

Here, again, the representations are easily converted. As for use…

  * node_refs are used for purposes that we're going to ignore for now (we're just covering the basics, here).

  * The [`BNode`](BNode.md "BNode") class is where the action is. If you want to read and write the data in a file, you need a [`BNode`](BNode.md "BNode") object—more specifically, you need an instance of the [`BFile`](BFile.md "BFile") class, which derives from [`BNode`](BNode.md "BNode").

Every node has a type, or flavor. There are three node flavors:

  * plain files

  * directories

  * symbolic links

These flavors are represented by subclasses of [`BNode`](BNode.md "BNode"): [`BFile`](BFile.md "BFile"), [`BDirectory`](BDirectory.md "BDirectory"), and [`BSymLink`](BSymLink.md "BSymLink"). Note that a node_ref doesn't know its node's flavor.

* * *

### More Facts

Some more facts you should be aware of:

  * Every node has an entry; not every entry has a node.

If you've got your hands on a node, then you can assume that there's an entry somewhere that "contains" that node. (This isn't entirely true, but it's true enough for now. For the real story, see "[Lies](TheStorageKit_Overview_Entries_And_Nodes.md#TheStorageKit_Overview_Lies "Lies")")

The converse isn't true: An entry needn't have any data. Such entry's are called "abstract". Abstract entries are useful for expressing the location of a file before it's created (for example). But don't be misled: Abstract entries do not exist in the file hierarchy, they're simply placeholders that your app uses to designate a location. This leads us to our next fact:

  * Every file in the file hierarchy has an entry and a node.

This might seem obvious; if it does, then go to the next fact. For the skeptics, here's the gospel: The files that "normal" apps work with are real—they actually exist as bytes on a disk. Such files have a location in the hierarchy, and they contain data.

  * You can convert an entry into a node, but not the other way around.

The [`BNode`](BNode.md "BNode") class accepts any form of entry representation as an argument to its constructor. In other words, given a pathname, entry_ref, or [`BEntry`](BEntry.md "BEntry") object, you can create a [`BNode`](BNode.md "BNode"). But once you've got your [`BNode`](BNode.md "BNode"), you can't go back: There's no way to get an entry from a node.

Returning to the [`BNode`](BNode.md "BNode") constructor: You can only create a [`BNode`](BNode.md "BNode") by passing the constructor an entry (in one of its representations). This is an important point that we'll pick up in the next section.

* * *

### Lies

Here are some more facts, slight alterations to the near truths spoken above:

#### A Node can Lose its Entry

Consider this scenario: You create a [`BFile`](BFile.md "BFile") object to some file. While you're reading and writing the file, the user deletes the file through the Tracker or from a Terminal. What the user has done is delete the node's entry, not the node itself. The node isn't destroyed until all references to the node, including your [`BFile`](BFile.md "BFile"), are deleted (or, more accurately, "closed"). The twist is that your [`BFile`](BFile.md "BFile") by itself has no way of knowing that the entry is gone.

So what are you supposed to do? In general, whenever you free a [`BFile`](BFile.md "BFile") object, you should first check to make sure the entry still exists; of course, the [`BFile`](BFile.md "BFile") itself can't tell you (remember: A node doesn't know about its entry), so you have to save the entry that was used to create the [`BFile`](BFile.md "BFile"). You ask the entry if it still exists, and then do whatever you have to do if it doesn't, such as alert the user, ask for a new entry name, and so on.

Unfortunately, this problem has another wrinkle: What if the user moves the entry while you're using the entry's node? In this case, the node isn't going to be destroyed, but if you ask the generative entry (the entry that was used to create the [`BFile`](BFile.md "BFile") object), it looks like the entry is gone.

There's no generic solution to the entire problem. Not because it's impossible to implement, but because the "right" solution depends on what the user meant by deleting or moving the entry. Most applications take this approach: The user knows files as entries, not as nodes. If a user opens a file through your app, moves the entry (through some other vehicle, such as the Tracker), and then asks your app to save the file, what the user really want is for you to save the node under the same name that was used to open the node.

#### A BDirectory Knows its Entry

[`BDirectory`](BDirectory.md "BDirectory") is an exception to the "ignorant node" rule: You can ask a [`BDirectory`](BDirectory.md "BDirectory") object for its entry (through its [`GetEntry()`](BDirectory.md#BDirectory_GetEntry "GetEntry\(\)") function).

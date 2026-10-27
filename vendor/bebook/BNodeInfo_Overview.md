<!-- System Overview - The Storage Kit - BNodeInfo -->

## BNodeInfo  
  
`BNodeInfo` provides file type information about a particular node; specifically:

  * The (MIME) file type.

  * The node's icons, including the node-specific icon that the Tracker displays.

  * The "preferred app"; this is the application that's used to access the node's contents.

Except for the Tracker icon, all this information can also be set through the `BNodeInfo` class. None of the information is passed on to the File Type database; if you want to record a node's file type information with the database, you have to create a [`BMimeType`](BMimeType.md "BMimeType") object (based on the node's file type) and go from there.

* * *

### Initialization

You initialize a `BNodeInfo` object by passing it a [`BNode`](BNode.md "BNode") object. Although you can pass any flavor of node, you typically only care about files; passing a [`BFile`](BFile.md "BFile") object (or any subclass of [`BNode`](BNode.md "BNode")) is, of course, acceptable. The `BNodeInfo` object maintains its own pointer to the BNode you pass in. You don't have to avoid touching the [`BNode`](BNode.md "BNode") while a `BNodeInfo` is looking at it (or changing it); the only thing you shouldn't do is delete the [`BNode`](BNode.md "BNode").

`BNodeInfo` doesn't care if the [`BNode`](BNode.md "BNode") is locked—there's no particular reason to lock the [`BNode`](BNode.md "BNode") before passing it in, but the `BNodeInfo` won't balk if you do. If you pass in a [`BFile`](BFile.md "BFile") object, `BNodeInfo` does not obey the [`BFile`](BFile.md "BFile")'s read/write flags. For example, you can set the node info for a [`BFile`](BFile.md "BFile") even if you've opened it in read-only mode.

* * *

### Node Info Equals Attributes

The `BNodeInfo` class does nothing more than look in a node's attributes for the information it sets or gets. The attribute names for the various information particles are given in the function descriptions, below. If you want, you can bypass `BNodeInfo` and get the node information directly by passing the attribute names to [`BNode`](BNode.md "BNode")'s [`ReadAttr()`](BNode.md#BNode_ReadAttr "ReadAttr\(\), WriteAttr\(\), RemoveAttr\(\)") and [`WriteAttr()`](BNode.md#BNode_WriteAttr) functions.

The one exception to this is [`GetTrackerIcon()`](BNodeInfo.md#BNodeInfo_GetTrackerIcon): This function starts by looking in the node's attributes, but then it goes out hunting if it has to (if the icon isn't found in the attributes).

* * *

### BAppFileInfo

`BNodeInfo` has a single subclass: [`BAppFileInfo`](BAppFileInfo.md "BAppFileInfo"). You use a [`BAppFileInfo`](BAppFileInfo.md "BAppFileInfo") object to get more information about a specific executable image (file).

* * *

### Errors

Unlike most of the other Storage Kit classes, when you ask a `BNodeInfo` to retrieve some information by reference, the object doesn't clear the reference argument if it fails. Because of this, you should always check the error code that's returned by the `Get…()` functions.

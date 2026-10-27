<!-- Classes And Methods - The Storage Kit - BNode -->

## BNode

| Derived From:| [`BStatable`](BStatable.md "BStatable")  
---|---  
Mix-in Classes:| None  
Declared In:| `storage/Node.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BNode_Overview.md "BNode")

  * [Constructor and Destructor](BNode.md#BNode_ConstructorDestructor)
  * [Member Functions](BNode.md#BNode_Member_Functions)
  * [Operators](BNode.md#BNode_Operators)
  * [Defined Types](BNode.md#BNode_DefinedTypes)

  
  
* * *

### Constructor and Destructor

#### BNode()

`BNode();` `BNode(const entry_ref* ref);` `BNode(const [`BEntry`](BEntry.md "BEntry")* entry);` `BNode(const char* path);` `BNode(const [`BDirectory`](BDirectory.md "BDirectory")* dir,  
const char* path);` `BNode(const BNode& node);`

Creates a new `BNode` object that's initialized to represent a specific file system node. To retrieve the status of the initialization, call [`InitCheck()`](BNode.md#BNode_InitCheck "InitCheck\(\)") immediately after constructing the object:
    
    
    BNode node("/boot/lbj/FidoOnFire.gif");
    if (node.InitCheck() != B_OK)
       /* The object wasn't initialized. */

A successfully initialized `BNode` object creates a "file descriptor" through which the object reads and writes the node's data and attributes. You can only have 256 file descriptors at a time (per application). The object's file descriptor is closed when the object is deleted, reset (through [`SetTo()`](BNode.md#BNode_SetTo "SetTo\(\), Unset\(\)")), or unset ([`Unset()`](BNode.md#BNode_Unset)).

  * Default constructor. The object's status will be `B_NO_INIT`, and the file descriptor isn't allocated until you actually initialize the object with a call to [`SetTo()`](BNode.md#BNode_SetTo "SetTo\(\), Unset\(\)").

  * Copy constructor. The new `BNode` is set to the same node as the argument. Each of the two `BNode` objects has its own file descriptor.

  * Other constructors. See the [`SetTo()`](BNode.md#BNode_SetTo "SetTo\(\), Unset\(\)") functions.

#### ~BNode()

`virtual ~BNode();`

Frees the object's file descriptor, unlocks the node (if it was locked), and destroys the object.

* * *

### Member Functions

#### GetAttrInfo()

`status_t GetAttrInfo(const char* attr,  
[attr_info](TheStorageKit_DefinedTypes.md#attr_info "attr_info")* info) const;`

Gets information about the attribute named by `attr`. The information is copied into the [attr_info](TheStorageKit_DefinedTypes.md#attr_info "attr_info") parameter `info`, which must be allocated before it's passed in.

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_ENTRY_NOT_FOUND`.| The node doesn't have an attribute named `attr`.  
`B_FILE_ERROR`.| The object is uninitialized.  
  
####  GetNextAttrName(), RewindAttrs()

`status_t GetNextAttrName(char* buffer);``status_t RewindAttrs();`

Every `BNode` maintains a pointer into its list of attributes. `GetNextAttrName()` retrieves the name of the attribute that the pointer is currently pointing to, and then bumps the pointer to the next attribute. The name is copied into the buffer, which should be at least `B_ATTR_NAME_LENGTH` characters long. The copied name is `NULL`-terminated. When you've asked for every name in the list, `GetNextAttrName()` returns an error.

Warning

![Warning](./images/admonitions/Stop_32.png)

`GetNextAttrName()` does _not_ clear its argument if it returns an error. This will be corrected in a subsequent release.

`RewindAttrs()` resets the `BNode`'s attribute pointer to the first elementin the list.

To visit every attribute name, you would do something like this:
    
    
    /* Print every attribute name. */
    char buf[B_ATTR_NAME_LENGTH];
    
    while (node.GetNextAttrName(buf) == B_OK) {
       printf("> Attr name: %sn", buf);
    }

The attribute list is not static; when you ask for the next attribute name, you're asking for the next name in the list _as it exists right now_.

Furthermore, the ordinal position of an attribute within the list is indeterminate. "Newer" attributes are not necessarily added to the end of the list: If you alter the list while you're walking through it, you may get curious results—you may not see the attribute that you just now added (for example).

In general, it's best to avoid altering the list while you're iterating over it.

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_ENTRY_NOT_FOUND`.| You've hit the end of the list.  
`B_FILE_ERROR`| The object is uninitialized.  
  
#### InitCheck()

`status_t InitCheck() const;`

Returns the status of the most recent initialization.

Return Code| Description  
---|---  
`B_OK`.| The object was successfully initialized.  
`B_NO_INIT`.| The object is uninitialized.  
Other return values| See the [`SetTo()`](BNode.md#BNode_SetTo "SetTo\(\), Unset\(\)") function.  
  
####  Lock(), Unlock()

`status_t Lock();``status_t Unlock();`

Locks and unlocks the `BNode`'s node. While the node is locked, no other object can access the node's data or attributes. More precisely, no other agent can create a file descriptor to the node. If a file descriptor already exists to this node, the `Lock()` function fails.

See "[Node Locking](BNode_Overview.md#BNode_Node_Locking "Node Locking")" for details.

Return Code| Description  
---|---  
`B_OK`.| The node was successfully locked or unlocked.  
`B_BUSY`.| (`Lock()`) The node can't be locked.  
`B_BAD_VALUE`.| (`Unlock()`) The node isn't locked.  
`B_FILE_ERROR`.| The object is uninitialized.  
  
####  ReadAttr(), WriteAttr(), RemoveAttr()

`ssize_t ReadAttr(const char* name,  
type_code type,  
off_t offset,  
void* buffer,  
size_t length);``ssize_t WriteAttr(const char* name,  
type_code type,  
off_t offset,  
const void* buffer,  
size_t length);``status_t RemoveAttr(const char* attr);`

These functions read, write, and remove the node's attributes. Attributes are name/data pairs, where names must be unique (within a given node) and the data can be of arbitrary length.

`ReadAttr()` reads the data in the attribute named `name`, and copies it in `buffer`. The length of the buffer (the maximum number of bytes to copy) is given by `length`. Currently, the `type` and `offset` arguments are unused (or unreliable). The function returns the number of bytes that were actually read.

`WriteAttr()` erases the data currently held by `name` (if such an attribute exists) and replaces it with a copy of the first `length` bytes of data in `buffer`. The `type` argument _is_ remembered—you can retrieve an attribute's type through [`GetAttrInfo()`](BNode.md#BNode_GetAttrInfo "GetAttrInfo\(\)"), for example—and you need to specify the correct type when you're forming a query (see [`BQuery`](BQuery.md "BQuery") and the note below). But, as mentioned above, you don't need to match types when you're reading the attribute. The `offset` argument is currently unreliable and shouldn't be used. The functions returns the number of bytes that were written.

Note

![Note](./images/admonitions/Info_32.png)

If you want to use the attribute in a query, its type must be either string, int32, uint32, int64, uint64, double, or float. (In other words, `type` must be `B_STRING_TYPE`, or `B_INT32_TYPE`, or `B_UINT32_TYPE`, and so on.)

Warning

![Warning](./images/admonitions/Stop_32.png)

The value of an indexed attribute must be no more than 255 bytes long.

`RemoveAttr()` deletes the attribute given by `name`.

`ReadAttr()` and `WriteAttr()`, if successful, return the number of bytes read or written.

Return Code| Description  
---|---  
`B_OK`.| (`RemoveAttr()`) The attribute was successfully removed.  
`B_ENTRY_NOT_FOUND`.| (`ReadAttr()` and `RemoveAttr()`) The attribute doesn't exist.  
`B_FILE_ERROR`.| The object is uninitialized.  
`B_FILE_ERROR`.| (`WriteAttr()` and `RemoveAttr()`) This object is a read-only [`BFile`](BFile.md "BFile").  
`B_NOT_ALLOWED`.| (`WriteAttr()` and `RemoveAttr()`) The node is on a read-only volume.  
`B_DEVICE_FULL`.| (`WriteAttr()`) Out of disk space.  
`B_NO_MEMORY`.| (`WriteAttr()`) Not enough memory to complete the operation.  
  
#### RenameAttr()

`status_t RenameAttr(const char* name,  
const char* new_name);`

Moves the attribute given by `name` to `new_name`. If `new_name` exists, it's clobbered.

Return Code| Description  
---|---  
`B_OK`.| The attribute was successfully renamed.  
`B_ENTRY_NOT_FOUND`.| The `name` attribute doesn't exist.  
`B_FILE_ERROR`.| The object is uninitialized.  
`B_FILE_ERROR`.| This object is a read-only [`BFile`](BFile.md "BFile").  
`B_NOT_ALLOWED`.| The node is on a read-only volume.  
  
####  SetTo(), Unset()

`status_t SetTo(const entry_ref* ref);` `status_t SetTo(const [`BEntry`](BEntry.md "BEntry")* entry);` `status_t SetTo(const char* path);` `status_t SetTo(const [`BDirectory`](BDirectory.md "BDirectory")* dir,  
const char* path);`

`void Unset();`

Closes the `BNode`'s current file descriptor and opens it on the node (of the entry) that's designated by the arguments.

  * In the `path` version, `path` can be absolute or relative, and can contain "." and ".." elements. If `path` is relative, it's reckoned off of the current working directory.

  * In the `dir`/`path` version, `path` must be relative. It's reckoned off of the directory given by `dir`.

`BNode` instances never traverse symbolic links. If the designated entry is a symbolic link, the BNode will open the link's node. (Conversely, [`BFile`](BFile.md "BFile") instances _always_ traverse symbolic links.)

`Unset()` closes the `BNode`'s file descriptor and sets [`InitCheck()`](BNode.md#BNode_InitCheck "InitCheck\(\)") to `B_NO_INIT`.

Return Code| Description  
---|---  
`B_OK`.| All is well.  
`B_BAD_VALUE`.| The designated entry doesn't exist.  
`B_BAD_VALUE`.| Uninitialized or malformed argument.  
`B_BUSY`.| The node is locked.  
  
#### Sync()

`status_t Sync();`

Immediately performs any pending disk transactions for the file, returning `B_OK` on success and an appropriate error message otherwise.

* * *

### Operators

#### = (assignment)

`BNode& operator=(const BNode& node);`

In the expression
    
    
    BNode a = b;

`BNode` `a` is initialized to refer to the same node as `b`. To gauge the success of the assignment, you should call [`InitCheck()`](BNode.md#BNode_InitCheck "InitCheck\(\)") immediately afterwards. It's safe to assign a `BNode` to itself.

####  == , != (comparison)

`bool operator==(const BNode& node) const;``bool operator!=(const BNode& node) const;`

Two `BNode` objects are said to be equal if they're set to the same node, or if they're both `B_NO_INIT`.

* * *

### Defined Types

#### node_ref

Declared in: `storage/Node.h`
    
    
    struct node_ref {
                  node_ref();
                  node_ref(const node_ref& ref);
                  ~node_ref();
    
        bool      operator==(const node_ref& ref) const;
        bool      operator!=(const node_ref& ref) const;
        node_ref& operator=(const node_ref& ref);
    
        dev_t     device;
        ino_t     node;
    }

The node_ref structure describes a node in a file system.

  * `device` contains the device number on which the node is located.

  * `node` contains the inode of the node.

`node_ref();``node_ref(const node_ref& ref);`

The constructor for the node_ref structure. The first of these creates an empty node_ref, and the second duplicates an existing node_ref.

`~node_ref();`

The destructor for node_ref.

`operator ==`

Lets you perform comparisons of node_ref structures to see if they refer to the same node.

`operator !=`

Lets you test to see if two node_ref structures refer to different nodes.

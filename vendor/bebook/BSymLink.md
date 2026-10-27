<!-- Classes And Methods - The Storage Kit - BSymLink -->

## BSymLink

| Derived From:| [`BNode`](BNode.md "BNode")  
---|---  
Mix-in Classes:| None  
Declared In:| `storage/SymLink.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BSymLink_Overview.md "BSymLink")

  * [Constructor and Destructor](BSymLink.md#BSymLink_ConstructorDestructor)
  * [Member Functions](BSymLink.md#BSymLink_Member_Functions)

  
  
* * *

### Constructor and Destructor

#### BSymLink()

`BSymLink();` `BSymLink(const entry_ref* ref);` `BSymLink(const [`BEntry`](BEntry.md "BEntry")* entry);` `BSymLink(const char* path);` `BSymLink(const [`BDirectory`](BDirectory.md "BDirectory")* dir,  
const char* path);` `BSymLink(const BSymLink& link);`

Creates a new `BSymLink` object, initializes it according to the arguments, and sets [`InitCheck()`](BNode.md#BNode_InitCheck "InitCheck\(\)") to return the status of the initialization.

  * The default constructor does nothing and sets [`InitCheck()`](BNode.md#BNode_InitCheck "InitCheck\(\)") to `B_NO_INIT`. To initialize the object, call [`SetTo()`](BNode.md#BNode_SetTo "SetTo\(\), Unset\(\)").

  * The copy constructor creates a new `BSymLink` that's open on the same node as that of the argument.

  * For information on the other constructors, see the analogous [`SetTo()`](BNode.md#BNode_SetTo "SetTo\(\), Unset\(\)") functions in the [`BNode`](BNode.md "BNode") class; `BSymLink` inherits them without change.

#### ~BSymLink()

`virtual ~BSymLink();`

Closes the object's file descriptor and destroys the object.

* * *

### Member Functions

#### IsAbsolute()

`bool IsAbsolute();`

Returns `true` if the symlink contains an absolute pathname.

#### MakeLinkedPath()

`ssize_t MakeLinkedPath(const [`BDirectory`](BDirectory.md "BDirectory")* dir,  
[`BPath`](BPath.md "BPath")* path) const;` `ssize_t MakeLinkedPath(const char* dirPath,  
[`BPath`](BPath.md "BPath")* path) const;`

This function creates an absolute pathname to the linked-to entry and returns it as a [`BPath`](BPath.md "BPath") object. For this to work you have to tell the object which directory you want to reckon off of (in case the symlink specifies a relative path). This should be the directory in which the symlink itself lives.

Note

![Note](./images/admonitions/Info_32.png)

Remember—a `BSymLink` is a node, and nodes don't know what directory they live in. That's why you have to tell it here.

If the symlink contains an absolute path, then the `dir` or `dirPath` arguments are ignored. Nonetheless, they must be supplied.

The function returns the length of the pathname that's set in [`BPath`](BPath.md "BPath") (or an error).

Return Code| Description  
---|---  
`B_FILE_ERROR`.| The object is uninitialized.  
`B_BAD_VALUE`.| The object doesn't refer to a symlink, or `dir`/`dirPath` is `NULL`.  
`B_NAME_TOO_LONG`.| They concatenated pathname is too long.  
  
#### ReadLink()

`ssize_t ReadLink(char* buf,  
size_t length);`

Copies the contents of the symlink into `buf`. `length` is the size of the buffer; to be perfectly safe, the buffer should be `B_PATH_NAME_LENGTH` characters long. The function returns the number of bytes that were copied (or it returns an error).

The symlink's contents is the pathname (relative or absolute) to the linked-to entry. Note that since the pathname might be relative, `ReadLink()` can't give you a [`BPath`](BPath.md "BPath") object. If you want a [`BPath`](BPath.md "BPath") to the linked-to entry, see [`MakeLinkedPath()`](BSymLink.md#BSymLink_MakeLinkedPath "MakeLinkedPath\(\)").

Return Code| Description  
---|---  
`B_FILE_ERROR`.| The object is uninitialized.  
`B_BAD_VALUE`.| The object doesn't refer to a symlink.

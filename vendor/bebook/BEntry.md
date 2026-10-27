<!-- Classes And Methods - The Storage Kit - BEntry -->

## BEntry

| Derived From:| [`BStatable`](BStatable.md "BStatable")  
---|---  
Mix-in Classes:| None  
Declared In:| `storage/Entry.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BEntry_Overview.md "BEntry")

  * [Constructor and Destructor](BEntry.md#BEntry_ConstructorDestructor)
  * [Member Functions](BEntry.md#BEntry_Member_Functions)
  * [Operators](BEntry.md#BEntry_Operators)
  * [Global C Function](BEntry.md#BEntry_Functions)
  * [Defined Types](BEntry.md#BEntry_DefinedTypes)

  
  
* * *

### Constructor and Destructor

#### BEntry()

`BEntry();` `BEntry(const [`BDirectory`](BDirectory.md "BDirectory")* dir,  
const char* path,  
bool traverse = false);` `BEntry(const entry_ref* entry,  
bool traverse = false);` `BEntry(const char* path,  
bool traverse = false);` `BEntry(const BEntry& entry);`

Creates a new `BEntry` object that represents the entry described by the arguments. See the analogous [`SetTo()`](BEntry.md#BEntry_SetTo) functions for descriptions of the flavorful constructors.

The default constructor does nothing; it should be followed by a call to [`SetTo()`](BEntry.md#BEntry_SetTo).

The copy constructor points the new object to the entry that's represented by the argument. The two objects themselves maintain separate representation of the entry; in other words, they each contain their own a) file descriptor and b) string to identify the entry's a) directory and b) name.

To see if the initialization was successful, call [`InitCheck()`](BEntry.md#BEntry_InitCheck "InitCheck\(\)").

#### ~BEntry

`virtual BEntry();`

Closes the `BEntry`'s file descriptor and destroys the `BEntry` object.

* * *

### Member Functions

#### Exists()

`bool Exists() const;`Return Code| Description  
---|---  
`true`| The entry exists.  
`false`| otherwise.  
  
####  GetName(), GetPath()

`status_t GetName(char* buffer) const;``status_t GetPath([`BPath`](BPath.md "BPath")* path) const;`

These functions return the leaf name and full pathname of the `BEntry`'s entry. The arguments must be allocated before they're passed in.

`GetName()` copies the leaf name into `buffer`. The `buffer` must be large enough to accommodate the name; `B_FILE_NAME_LENGTH` is a 100% safe bet:
    
    
    char name[B_FILE_NAME_LENGTH];
    entry.GetName(name);

If `GetName()` fails, `*buffer` is pointed at `NULL`.

`GetPath()` takes the entry's full pathname and initializes the [`BPath`](BPath.md "BPath") argument with it. To retrieve the path from the [`BPath`](BPath.md "BPath") object, call [`BPath::Path()`](BPath.md#BPath_Path "Path\(\), Leaf\(\)"):
    
    
    BPath path;
    entry.GetPath(&path);
    printf(">Entry pathname: %sn", path.Path());

If `GetPath()` fails, the argument is [`Unset()`](BEntry.md#BEntry_Unset "SetTo\(\), Unset\(\)").

Return Code| Description  
---|---  
`B_OK`.| The information was successfully retrieved.  
`B_NO_INIT`.| The `BEntry` isn't initialized.  
`B_BUSY`| (`GetPath()` only). A directory in the entry's path is locked.  
  
#### GetParent()

`status_t GetParent(BEntry* entry) const;` `status_t GetParent([`BDirectory`](BDirectory.md "BDirectory")* dir) const;`

Gets the directory, as a `BEntry` or [`BDirectory`](BDirectory.md "BDirectory") object, in which the object's entry lives. The argument must be allocated before it's passed in.

If the function is unsuccessful, the argument is [`Unset()`](BEntry.md#BEntry_Unset "SetTo\(\), Unset\(\)"). Because of this, you should be particularly careful if you're using the `BEntry`-argument version to destructively get a `BEntry`'s parent:
    
    
    if (entry.GetParent(&entry) != B_OK) {
       /* you just lost 'entry' */
    }

This example _is_ legal; for example, you can use destructive iteration to loop your way up to the root directory. When you reach the root ("/"), `GetParent()` returns `B_ENTRY_NOT_FOUND`:
    
    
    BEntry entry("/boot/home/fido");
    status_t err;
    char name[B_FILE_NAME_LENGTH];
    
    /* Spit out the path components backwards, one at a time. */
    do {
       entry.GetName(name);
       printf("> %sn", name);
    } while ((err=entry.GetParent(&entry)) == B_OK);
    
    /* Complain for reasons other than reaching the top. */
    if (err != B_ENTRY_NOT_FOUND)
       printf(">> Error: %sn", strerror(err));

This produces:
    
    
    > fido
    > home
    > boot
    > /
    

Return Code| Description  
---|---  
`B_OK`.| The information was successfully retrieved.  
`B_NO_INIT`.| This `BEntry` isn't initialized.  
`B_ENTRY_NOT_FOUND`.| Attempt to get the parent of the root directory.  
`B_NO_MORE_FDS`.| Couldn't get another file descriptor.  
  
#### GetRef()

`status_t GetRef(entry_ref* ref) const;`

Gets the entry_ref for the object's entry; `ref` must be allocated before it's passed in. As with `BEntry` objects, entry_ref structures can be abstract—getting a valid entry_ref does _not_ guarantee that the entry actually exists.

If the function isn't successful, `ref` is unset.

Return Code| Description  
---|---  
`B_OK`.| The entry_ref was successfully retrieved.  
`B_NO_INIT`.| This object isn't initialized.  
`B_NO_MEMORY`.| Storage for the entry_ref's name couldn't be allocated.  
  
#### GetStat()

`virtual status_t GetStat(struct stat* st) const;`

`GetStat()` returns the stat structure for the entry. The structure is copied into the `st` argument, which must be allocated. The [`BStatable`](BStatable.md "BStatable") object does not cache the stat structure; every time you call `GetStat()`, fresh stat information is retrieved.

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_NO_MEMORY`.| Couldn't get the necessary resources to complete the transaction.  
`B_BAD_VALUE`.| The entry doesn't exist (abstract entry).  
  
#### InitCheck()

`status_t InitCheck() const;`

Returns the status of the previous construction, assignment operation, or [`SetTo()`](BEntry.md#BEntry_SetTo) call.

Return Code| Description  
---|---  
`B_OK`.| The initialization was successful.  
`B_NO_INIT`.| The object is uninitialized (this includes [`Unset()`](BEntry.md#BEntry_Unset "SetTo\(\), Unset\(\)")).  
Other errors.| See [`SetTo()`](BEntry.md#BEntry_SetTo)  
  
#### Remove()

`status_t Remove();`

`Remove()` "unlinks" the entry from its directory. The entry's node isn't destroyed until all file descriptors that are open on the node are closed. This means that if you create [`BFile`](BFile.md "BFile") based on a `BEntry`, and then `Remove()` the `BEntry`, the [`BFile`](BFile.md "BFile") will still be able to read and write the file's data—the [`BFile`](BFile.md "BFile") has no way of knowing that the entry is gone. When the [`BFile`](BFile.md "BFile") is deleted, the node will be destroyed as well.

Note

![Note](./images/admonitions/Info_32.png)

`Remove()` does _not_ invalidate the `BEntry`. It simply makes it abstract (see "[Abstract Entries](BEntry_Overview.md#BEntry_Abstract_Entries "Abstract Entries")").

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_NO_INIT`.| The `BEntry` is not initialized.  
`B_BUSY`.| The entry's directory is locked.  
  
####  Rename(), MoveTo()

`status_t Rename(const char* path,  
bool clobber = false);``status_t MoveTo([`BDirectory`](BDirectory.md "BDirectory")* dir,  
const char* path = NULL,  
bool clobber = false);`

These functions move the `BEntry`'s entry and node to a new location. In both cases, the `BEntry` must not be abstract—you can't rename or move an abstract entry.

`Rename()` moves the entry to a new name, as given by `path`. `path` is usually a simple leaf name, but it can be a relative path. In the former case (simple leaf) the entry is renamed within its current directory. In the latter, the entry is moved into a subdirectory of its current directory, as given by the argument.

`MoveTo()` moves the entry to a different directory and optionally renames the leaf. Again, `path` can be a simple leaf or a relative path; in both cases, `path` is reckoned off of `dir`. If path is `NULL`, the entry is moved to `dir`, but retains its old leaf name.

If the entry's new location is already taken, the clobber argument decides whether the existing entry is removed to make way for yours. If it's `true`, the existing entry is removed; if it's `false`, the `Rename()` or `MoveTo()` function fails.

Upon success, this is updated to reflect the change to its entry. For example, when you invoke `Rename()` on a `BEntry`, the name of _that specific_ `BEntry` object also changes. If the rename or move-to isn't successful, this isn't altered.

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_NO_INIT`.| The `BEntry` is not initialized.  
`B_ENTRY_NOT_FOUND`.| A directory to the new location doesn't exist, or this is an abstract entry.  
`B_FILE_EXISTS`.| The new location is already taken (and you're not clobbering).  
`B_BUSY`.| The directory that you're moving the entry into is locked.  
  
####  SetTo(), Unset()

`status_t SetTo(const entry_ref* ref,  
bool traverse = false);` `status_t SetTo(const char* path,  
bool traverse = false);` `status_t SetTo(const [`BDirectory`](BDirectory.md "BDirectory")* dir,  
const char* path,  
bool traverse = false);`

`void Unset();`

Frees the `BEntry`'s current entry reference, and initializes it to refer to the entry identified by the argument(s):

  * In the `ref` version, the `BEntry` is initialized to refer to the given entry_ref.

  * In the `path` version, `path` can be absolute or relative, and can contain "." and ".." elements. If `path` is relative, it's reckoned off of the current working directory.

  * In the `dir`/`path` version, `path` must be relative. It's reckoned off of the directory given by `dir`.

The `traverse` argument is used to resolve (or not) entries that are symlinks:

  * If `traverse` is `true`, the link is resolved.

  * If `traverse` is `false`, the `BEntry` refers to the link itself.

See "[Initializing and Traversing](BEntry_Overview.md#BEntry_Initializing_And_Traversing "Initializing and Traversing")" for more information.

When you initialize a `BEntry`, you're describing a leaf name within a directory. The directory _must_ exist, but the leaf doesn't have to. This allows you to create a `BEntry` to a file that doesn't exist (yet). See "[Abstract Entries](BEntry_Overview.md#BEntry_Abstract_Entries "Abstract Entries")" for more information.

Note

![Note](./images/admonitions/Info_32.png)

Remember—successfully initializing a `BEntry` consumes a file descriptor. When you re-initialize, the old file descriptor is closed.

`Unset()` removes the object's association with its current entry, and sets [`InitCheck()`](BEntry.md#BEntry_InitCheck "InitCheck\(\)") to `B_NO_INIT`.

Return Code| Description  
---|---  
`B_OK`.| The `BEntry` was successfully initialized.  
`B_BAD_VALUE`.| Bad argument value; uninitialized `ref` or `dir`.  
`B_ENTRY_NOT_FOUND`.| A directory in the path to the entry doesn't exist.  
`B_ENTRY_NOT_FOUND`.| The entry's directory is locked.  
  
* * *

### Operators

#### = (assignment)

`BEntry& operator=(const BEntry& entry);`

In the expression
    
    
    BEntry a = b;

`BEntry` `a` is initialized to refer to the same entry as `b`. To gauge the success of the assignment, you should call [`InitCheck()`](BEntry.md#BEntry_InitCheck "InitCheck\(\)") immediately afterwards. Assigning a `BEntry` to itself is safe.

Assigning from an uninitialized `BEntry` is "successful": The assigned-to `BEntry` will also be uninitialized (`B_NO_INIT`).

####  == , != (comparison)

`bool operator==(const BEntry& entry) const;``bool operator!=(const BEntry& entry) const;`

Two `BEntry` objects are said to be equal if they refer to the same entry (even if the entry is abstract), or if they're both uninitialized.

* * *

### Global C Function

#### get_ref_for_path()

`status_t get_ref_for_path(const char* path,  
entry_ref* ref);`

Returns in `ref` an entry_ref for the file specified by the `path` argument.

Return Code| Description  
---|---  
`B_OK`.| The ref was returned successfully.  
`B_ENTRY_NOT_FOUND`.| The file wasn't found, or the pathname string is empty.  
`B_NO_MEMORY`.| Not enough memory.  
Other file errors.|   
  
* * *

### Defined Types

#### entry_ref

Declared in: `storage/Entry.h`
    
    
    struct entry_ref {
                   entry_ref();
                   entry_ref(dev_t device, ino_t dir, const char* name);
                   entry_ref(const entry_ref& ref);
                   ~entry_ref();
    
        status_t   set_name(const char* name);
        bool       operator==(const entry_ref& ref) const;
        bool       operator!=(const entry_ref& ref) const;
        entry_ref& operator=(const entry_ref& ref);
    
        dev_t      device;
        ino_t      directory;
        char*      name;
    }

The entry_ref structure describes a single entry in a directory.

  * `device` contains the device number on which the entry's target is located.

  * `directory` contains the inode of the directory that contains the entry's target.

  * `name` contains the name of the entry.

`entry_ref();``entry_ref(const entry_ref& ref);``entry_ref(dev_t device,  
ino_t dir,  
const char* name);`

The constructor for the entry_ref structure. The first of these creates an empty entry_ref, the second duplicates an existing entry_ref, and the last version of the constructor accepts a device number, directory inode number, and a file name and constructs an entry_ref referring to that entry.

`~entry_ref();`

The destructor for entry_ref.

`status_t set_name(const char* name);`

Lets you change the name of the file referred to by the entry_ref structure.

`operator ==`

Lets you perform comparisons of entry_ref structures to see if they refer to the same entry.

`operator !=`

Lets you test to see if two entry_ref structures refer to different entries.

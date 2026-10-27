<!-- Classes And Methods - The Storage Kit - BDirectory -->

## BDirectory

| Derived From:| [`BNode`](BNode.md "BNode"), [`BEntryList`](BEntryList.md "BEntryList")  
---|---  
Mix-in Classes:| None  
Declared In:| `storage/Directory.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BDirectory_Overview.md "BDirectory")

  * [Constructor and Destructor](BDirectory.md#BDirectory_ConstructorDestructor)
  * [Member Functions](BDirectory.md#BDirectory_Member_Functions)
  * [Operators](BDirectory.md#BDirectory_Operators)
  * [C Functions](BDirectory.md#BDirectory_Functions)
  * [Constants](BDirectory.md#BDirectory_Constants)

  
  
* * *

### Constructor and Destructor

#### BDirectory()

`BDirectory();` `BDirectory(const entry_ref* ref);` `BDirectory(const [`BEntry`](BEntry.md "BEntry")* entry);` `BDirectory(const node_ref* nref);` `BDirectory(const char* path);` `BDirectory(const BDirectory* dir,  
const char* path);` `BDirectory(const BDirectory& directory);`

Creates a new `BDirectory` object that represents the directory as given by the arguments. See the analogous [`SetTo()`](BDirectory.md#BDirectory_SetTo "SetTo\(\), Unset\(\)") functions for descriptions of the flavorful constructors.

  * The default constructor does nothing; it should be followed by a call to [`SetTo()`](BDirectory.md#BDirectory_SetTo "SetTo\(\), Unset\(\)").

  * The copy constructor points the `BDirectory` to the same directory as is represented by the argument. The two objects have their own entry iterators.

To check to see if an initialization was successful, call [`InitCheck()`](BNode.md#BNode_InitCheck "InitCheck\(\)").

#### ~BDirectory()

`virtual ~BDirectory()();`

Deletes the object.

* * *

### Member Functions

#### Contains()

`bool Contains(const char* path,  
int32 nodeFlags = B_ANY_NODE) const;` `bool Contains(const [`BEntry`](BEntry.md "BEntry")* entry,  
int32 nodeFlags = B_ANY_NODE) const;`

Returns `true` if `path` or `entry` is contained within this directory, or in any of its subdirectories (no matter how deep). You can use the `nodeFlags` argument to limit the search to a particular flavor of node:

Constant| Description  
---|---  
`B_FILE_NODE`| Looks for a "plain" file.  
`B_DIRECTORY_NODE`| Looks for a directory.  
`B_SYMLINK_NODE`| Looks for a symbolic link.  
`B_ANY_NODE`| (The default) Doesn't discriminate between flavors.  
  
####  CreateFile(), CreateDirectory(), CreateSymLink()

`status_t CreateFile(const char* path,  
[`BFile`](BFile.md "BFile")* file,  
bool failIfExists = false);``status_t CreateDirectory(const char* path,  
BDirectory* dir);``status_t CreateSymLink(const char* path,  
const char* linkToPath,  
[`BSymLink`](BSymLink.md "BSymLink")* link);`

These functions create a new file, directory, or symbolic link. The new node is located at `path`. If `path` is relative, it's reckoned off of the directory represented by this `BDirectory`; if it's absolute, the path of this `BDirectory` is ignored.

  * `CreateFile()` fails if the file already exists and `failIfExists` is `true`. If the flag is `false` (and the file exists), the old file is clobbered and a new one is created. If successful, the [`BFile`](BFile.md "BFile") argument that you pass in is opened on the new file in `B_READ_WRITE` mode.

  * `CreateDirectory()` and `CreateSymLink()` fail if `path` already exists—you can't clobber an existing directory or link.

  * The `linkToPath` argument (`CreateSymLink()`) is the path that the new symbolic link will be linked to.

The object argument (the `BDirectory`, [`BFile`](BFile.md "BFile"), or [`BSymLink`](BSymLink.md "BSymLink")) may be `NULL`. If the function fails, the object argument, if non-`NULL`, is [`Unset()`](BDirectory.md#BDirectory_Unset).

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_BAD_VALUE`.| Illegal `path`, `file`, `dir`, or `link` specified; may be `NULL`. path may be empty.  
`B_BUSY`.| A busy node could not be accessed.  
`B_ENTRY_NOT_FOUND`.| The specified `path` does not exist or is an empty string.  
`B_FILE_ERROR`.| A file system error prevented the operation.  
`B_FILE_EXISTS`.| The file specified by `path` already exists.  
`B_LINK_LIMIT`.| A cyclic loop has been detected in the file system.  
`B_NAME_TOO_LONG`.| The `path` specified is too long.  
`B_NO_MEMORY`.| Insufficient memory to perform the operation.  
`B_NO_MORE_FDS`.| All file descriptors are in use (too many open files).  
`B_IS_A_DIRECTORY`.| Can't replace a directory with a file.  
`B_NOT_A_DIRECTORY`.| A component of the `path` is not a directory.  
`B_NOT_ALLOWED`.| The volume is read-only.  
`B_PERMISSION_DENIED`.| Create access is denied in the specified `path`.  
`E2BIG`.| `linkToPath` is too long (`CreateSymLink()` only).  
  
#### FindEntry()

`status_t FindEntry(const char* path,  
[`BEntry`](BEntry.md "BEntry")* entry,  
bool traverse = false) const;`

Finds the entry with the given name, and sets the second argument to refer to that entry.

  * `path` must be a relative pathname. It's reckoned off of the `BDirectory`'s directory.

  * You are allowed to look for "." and "..". The former represents this directory's entry. The latter refers to this directory's parent.

  * The `entry` argument must be allocated before it's passed in (it needn't be initialized).

  * The `traverse` applies to symbolic links: If the flag is `true`, the link is traversed. If it's `false`, you get the [`BEntry`](BEntry.md "BEntry") that points to the link itself.

If `path` isn't found, the second argument is automatically [`Unset()`](BDirectory.md#BDirectory_Unset). To find out why the lookup failed, invoke [`InitCheck()`](BNode.md#BNode_InitCheck "InitCheck\(\)") on the `entry` argument:
    
    
    BEntry entry;
    status_t err;
    
    if (dir.FindEntry("aFile", &entry) != B_OK) {
       err = entry.InitCheck();
    }

The direct return value is also informative, but it may not be as precise as the [`InitCheck()`](BNode.md#BNode_InitCheck "InitCheck\(\)") value.

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_BAD_VALUE`.| Invalid `path` specified; it may be `NULL` or empty.  
`B_ENTRY_NOT_FOUND`.| The specified `path` does not exist.  
`B_NAME_TOO_LONG`.| The `path` specified is too long.  
`B_LINK_LIMIT`.| A cyclic loop has been detected in the file system.  
`B_NO_MEMORY`.| Insufficient memory to perform the operation.  
`B_FILE_ERROR`.| An invalid file prevented the operation.  
  
#### GetEntry()

`status_t GetEntry([`BEntry`](BEntry.md "BEntry")* entry) const;`

Initializes `entry` to represent this `BDirectory`. If the initialization fails, `entry` is [`Unset()`](BDirectory.md#BDirectory_Unset).

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_NAME_TOO_LONG`.| The path specified by `entry` is too long.  
`B_ENTRY_NOT_FOUND`.| The specified path does not exist.  
`B_LINK_LIMIT`.| A cyclic loop has been detected in the file system.  
`B_BAD_VALUE`.| `entry` is uninitialized.  
`B_NO_MEMORY`.| Insufficient memory to perform the operation.  
`B_BUSY`.| A busy node could not be accessed.  
`B_FILE_ERROR`.| An invalid file prevented the operation.  
`B_NO_MORE_FDS`.| All file descriptors are in use (too many open files).  
`B_NOT_A_DIRECTORY`.| The path includes non-directory entries.  
  
####  GetNextEntry(), GetNextRef(), GetNextDirents(), CountEntries(),Rewind()

`virtual status_t GetNextEntry([`BEntry`](BEntry.md "BEntry")* entry,  
bool traverse = false);``virtual status_t GetNextRef(entry_ref* ref);``virtual int32 GetNextDirents(dirent* buf,  
size_t bufsize,  
int32 count = INT_MAX);``virtual int32 CountEntries();``virtual status_t Rewind();`

The three `GetNext…()` functions retrieve the "next" entry that lives in the `BDirectory` and returns it as a [`BEntry`](BEntry.md "BEntry"), entry_ref, or dirent structure.

  * `GetNextEntry()` returns the entry as a [`BEntry`](BEntry.md "BEntry") object. If `traverse` is `true` and the entry is a symbolic link, the link is traversed. In other words, `entry` could end up being in a different directory than the one referred to by this. When all entries have been visited, the function returns `B_ENTRY_NOT_FOUND`. The `entry` argument must be allocated before it's passed in.

  * `GetNextRef()` return the next entry in `ref`. Since an entry_ref doesn't supply enough information to determine if the entry is a link, there's no question of traversal: The entry_ref points to exactly the next entry. When all entries have been visited, the function returns `B_ENTRY_NOT_FOUND`. The `ref` argument must be allocated before it's passed in.

  * `GetNextDirents()` returns some number of dirent structures, either as many as can be stuffed into `buf` (where `bufsize` gives the size of `buf`), or `count` structures, whichever is smaller. The function returns the number of structures that were stuffed into `buf`; when all entries have been visited, it returns 0.

Warning

![Warning](./images/admonitions/Stop_32.png)

Currently, `GetNextDirents()` only reads one dirent at a time, no matter how many you ask for.

`GetNextEntry()` and `GetNextRef()` are reasonably clear; the dirent version deserves more explanation. You'll find this explanation (and an example) in the [`BEntryList`](BEntryList.md "BEntryList") class. Also, keep in mind that the set of candidate entries is different for the dirent version: `GetNextDirents()` finds _all_ entries, including the entries for "." and "..". The other two versions skip these entries.

When you're done reading the `BDirectory`'s entries, you can rewind the object's entry iterator by calling [`Rewind()`](BDirectory.md#BDirectory_Rewind).

`CountEntries()` returns the number of entries (not counting "." and "..") in the directory.

Warning

![Warning](./images/admonitions/Stop_32.png)

Never call `CountEntries()` while you're iterating through the directory. `CountEntries()` does a rewind, iterates through the entries, and then rewinds again.

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_FILE_ERROR`.| `BDirectory` object has not been properly initialized.  
`B_NOT_A_DIRECTORY`.| The directory is invalid.  
`B_NAME_TOO_LONG`.| The dirent's name is too long.  
`B_ENTRY_NOT_FOUND`.| End of directory reached.  
`B_LINK_LIMIT`.| A cyclic loop has been detected in the file system.  
`B_BAD_VALUE`.| Invalid input specified, or `BDirectory` object has not been properly initialized.  
`B_NO_MEMORY`.| Insufficient memory to perform the operation.  
  
#### GetStatFor()

`status_t GetStatFor(const char* path,  
stat* st) const;`

Gets the stat structure for the entry designated by `path`. `path` may be either absolute or relative; if it's relative, it's reckoned off of the `BDirectory`'s directory. This is, primarily, a convenience function; but it's also provided for efficiency.

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_FILE_ERROR`.| An invalid file prevented the operation.  
`B_NAME_TOO_LONG`.| The `path` specified is too long.  
`B_ENTRY_NOT_FOUND`.| The specified `path` does not exist.  
`B_LINK_LIMIT`.| A cyclic loop has been detected in the file system.  
`B_BAD_VALUE`.| Invalid input specified; the `path` may be `NULL` or empty.  
`B_NO_MEMORY`.| Insufficient memory to perform the operation.  
  
#### IsRootDirectory()

`bool IsRootDirectory() const;`

Returns `true` if this `BDirectory` represents a root directory. A root directory is the directory that's at the root of a volume's file hierarchy. Every volume has exactly one root directory; all other files in the volume's hierarchy descend from the root directory.

####  SetTo(), Unset()

`status_t SetTo(const entry_ref* ref);` `status_t SetTo(const node_ref* nref);` `status_t SetTo(const [`BEntry`](BEntry.md "BEntry")* entry);` `status_t SetTo(const char* path);` `status_t SetTo(const BDirectory* directory,  
const char* path);`

`void Unset();`

Closes the `BDirectory`'s current directory (if any), and initializes the object to open the directory as given by the arguments.

  * In the `path` version, path can be absolute or relative, and can contain "." and ".." elements. If `path` is relative, it's reckoned off of the current working directory.

  * In the `dir`/`path` version, `path` must be relative. It's reckoned off of the directory given by `dir`.

If the specification results in a symbolic link that resolves to a directory, then the linked-to directory is opened. If the specification is (or resolves to) a regular file, the initialization fails.

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_NAME_TOO_LONG`.| The `path` specified is too long.  
`B_ENTRY_NOT_FOUND`.| The directory does not exist.  
`B_LINK_LIMIT`.| A cyclic loop has been detected in the file system.  
`B_BAD_VALUE`.| Invalid input specified.  
`B_NO_MEMORY`.| Insufficient memory to perform the operation.  
`B_BUSY`.| A busy node could not be accessed.  
`B_FILE_ERROR`.| An invalid file prevented the operation.  
`B_NO_MORE_FDS`.| All file descriptors are in use (too many open files).  
  
* * *

### Operators

#### = (assignment)

`BDirectory& operator=(const BDirectory& directory);`

In the expression
    
    
    BDirectory a = b;

`BDirectory` `a` is initialized to refer to the same directory as `b`. To gauge the success of the assignment, you should call [`InitCheck()`](BNode.md#BNode_InitCheck "InitCheck\(\)") immediately afterwards. Assigning a `BDirectory` to itself is safe.

Assigning from an uninitialized `BDirectory` is "successful": The assigned-to `BDirectory` will also be uninitialized (`B_NO_INIT`).

* * *

### C Functions

#### create_directory()

`status_t create_directory(const char* path,  
mode_t mode);`

Creates all missing directories along the path specified by `path`.

  * The pathname can be absolute or relative. If it's relative, the path is reckoned of the current working directory. If any symlinks are found in the existing portion of the path, they're traversed.

  * `path` can contain ".", but it may not contain "..".

  * `mode` is the permissions setting (typically expressed as an octal number) that's assigned to all directories that are created. To set the directories to be readable, writable, and "enterable" by all (for example), you would set the mode to 0777.

Return Code| Description  
---|---  
`B_OK`.| `path` now fully exists (or did in the first place).  
`B_BAD_VALUE`.| `path` is `NULL`, is empty, or contains "..".  
`B_NOT_ALLOWED`.| Read-only volume.  
`B_NO_MEMORY`.| Insufficient memory to perform the operation.  
  
#### find_directory()

Declared in: `storage/FindDirectory.h`

`status_t find_directory(directory_which which,  
dev_t volume,  
bool create_it,  
char* path_string,  
int32 length);` `status_t find_directory(directory_which which,  
[`BPath`](BPath.md "BPath")* path_obj,  
bool create_it = false,  
[`BVolume`](BVolume.md "BVolume") volume = NULL);`

Note

![Note](./images/admonitions/Info_32.png)

The first version of this function can be used in either C or C++ code. The second version is for C++ code only.

Finds the path to the directory symbolized by `which` and copies it into `path_string`, or uses it to initialize `path_obj`.

  * The `create_it` argument tells the function to create the directory if it doesn't already exist.

  * `volume` identifies the volume (as a dev_t identifier or [`BVolume`](BVolume.md "BVolume") object) on which you want to look. The C++ default (`NULL`) means to look in the boot volume.

  * The `length` argument (first version only) gives the length of `path`.

The [directory_which](BDirectory.md#Constants_directory_which "directory_which\(\) Constants") constants are described below.

Return Code| Description  
---|---  
`B_OK`.| The directory was found.  
Other codes.| The directory wasn't found or couldn't be created.  
  
* * *

### Constants

####  directory_which() Constants

Declared in: `storage/FindDirectory.h`

Constant| Description  
---|---  
`B_DESKTOP_DIRECTORY`| Desktop directory.  
`B_TRASH_DIRECTORY`| Trash directory.  
`B_APPS_DIRECTORY`| Applications directory.  
`B_PREFERENCES_DIRECTORY`| Preferences directory.  
`B_BEOS_DIRECTORY`| BeOS directory.  
`B_BEOS_SYSTEM_DIRECTORY`| System directory.  
`B_BEOS_ADDONS_DIRECTORY`| BeOS add-ons directory.  
`B_BEOS_BOOT_DIRECTORY`| Boot volume's root directory.  
`B_BEOS_FONTS_DIRECTORY`| BeOS fonts directory.  
`B_BEOS_LIB_DIRECTORY`| BeOS libraries directory.  
`B_BEOS_SERVERS_DIRECTORY`| BeOS servers directory.  
`B_BEOS_APPS_DIRECTORY`| BeOS applications directory.  
`B_BEOS_BIN_DIRECTORY`| `/bin` directory.  
`B_BEOS_ETC_DIRECTORY`| `/etc` directory.  
`B_BEOS_DOCUMENTATION_DIRECTORY`| BeOS documentation directory.  
`B_BEOS_PREFERENCES_DIRECTORY`| BeOS preferences directory.  
`B_COMMON_DIRECTORY`| The common directory, shared by all users.  
`B_COMMON_SYSTEM_DIRECTORY`| The shared system directory.  
`B_COMMON_ADDONS_DIRECTORY`| The shared addons directory.  
`B_COMMON_BOOT_DIRECTORY`| The shared boot directory.  
`B_COMMON_FONTS_DIRECTORY`| The shared fonts directory.  
`B_COMMON_LIB_DIRECTORY`| The shared libraries directory.  
`B_COMMON_SERVERS_DIRECTORY`| The shared servers directory.  
`B_COMMON_BIN_DIRECTORY`| The shared `/bin` directory.  
`B_COMMON_ETC_DIRECTORY`| The shared `/etc` directory.  
`B_COMMON_DOCUMENTATION_DIRECTORY`| The shared documentation directory.  
`B_COMMON_SETTINGS_DIRECTORY`| The shared settings directory.  
`B_COMMON_DEVELOP_DIRECTORY`| The shared develop directory.  
`B_COMMON_LOG_DIRECTORY`| The shared log directory.  
`B_COMMON_SPOOL_DIRECTORY`| The shared spool directory.  
`B_COMMON_TEMP_DIRECTORY`| The shared temporary items directory.  
`B_COMMON_VAR_DIRECTORY`| The shared `/var` directory.  
`B_USER_DIRECTORY`| The user's home directory.  
`B_USER_CONFIG_DIRECTORY`| The user's config directory.  
`B_USER_ADDONS_DIRECTORY`| The user's add-ons directory.  
`B_USER_BOOT_DIRECTORY`| The user's `/boot` directory.  
`B_USER_FONTS_DIRECTORY`| The user's fonts directory.  
`B_USER_LIB_DIRECTORY`| The user's libraries directory.  
`B_USER_SETTINGS_DIRECTORY`| The user's settings directory.  
`B_USER_DESKBAR_DIRECTORY`| The user's Deskbar directory.  
  
These constants are used when calling the [`find_directory()`](BDirectory.md#find_directory "find_directory\(\)") function to determine the pathname of a particular directory of interest.

`B_DESKTOP_DIRECTORY` and `B_TRASH_DIRECTORY` are per-volume directories; if you don't specify the volume you wish to locate these directories on, [`find_directory()`](BDirectory.md#find_directory "find_directory\(\)") will assume you mean the boot disk.

`B_APPS_DIRECTORY` and `B_PREFERENCES_DIRECTORY` are global directories, and always refer to the standard apps and preferences directories.

The `B_BEOS_*` constants refer to BeOS-owned directories, the `B_COMMON_*` constants refer to directories that are common to all users of the system, and the `B_USER_*` constants refer to the current user's directories (currently these are all in a subtree rooted at `/boot/home`, but when multiuser support is implemented in a future version of BeOS, these won't necessarily all be the same anymore).

In general, global application and system settings should be kept in `B_COMMON_*`; while settings that each user should be able to configure individually should be kept in `B_USER_*`.

By using these constants properly, your code will be compatible with future generations of the BeOS.

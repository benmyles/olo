<!-- Classes And Methods - The Storage Kit - Functions -->

## Functions

* * *

### Attribute Functions

Declared In: `kernel/fs_attr.h`

#### fs_close_attr_dir()

`int fs_close_attr_dir(DIR* dirp);`

Closes the specified attribute directory. You should pass into this function the pointer returned when you called [`fs_open_attr_dir()`](TheStorageKit_Functions.md#fs_open_attr_dir "fs_open_attr_dir\(\), fs_fopen_attr_dir\(\)") or [`fs_fopen_attr_dir()`](TheStorageKit_Functions.md#fs_fopen_attr_dir).

If successful, this function returns 0; otherwise it returns -1 and sets `errno` to a descriptive code.

Return Code| Description  
---|---  
`B_FILE_ERROR`.| Invalid directory reference specified.  
  
####  fs_open_attr_dir(), fs_fopen_attr_dir()

`DIR* fs_open_attr_dir(const char* path);` `DIR* fs_open_attr_dir(int fd);`

Opens the attribute directory for the file specified by `path` or file descriptor `fd`.

The attribute directory for a file contains a list of the attributes that are attached to the file. Once the attribute directory is open, you can use the `fs_read_attr_dir()` function to find out which attributes are present.

If the directory is opened successfully, a pointer to the directory structure is returned. This pointer should be passed to the other `fs_attr` functions to read entries from the attribute directory, as well as to close the directory when you're finished with it.

If an error occurs while opening the attribute directory, this function returns `NULL` and sets `errno` to one of the values listed below.

Return Code| Description  
---|---  
`B_FILE_ERROR`.| Invalid file descriptor, or a file error prevented the operation.  
`B_ENTRY_NOT_FOUND`.| No matching attribute was found for the specified file.  
`B_NAME_TOO_LONG`.| `path` is longer than `B_PATH_NAME_LENGTH` characters.  
`B_LINK_LIMIT`.| A loop was detected in the directory structure.  
`B_NO_MEMORY`.| Insufficient memory to complete the operation.  
`B_BUSY`.| The specified file is currently in use.  
`B_NO_MORE_FDS`.| Too many open files; all file descriptors are in use.  
  
#### fs_read_attr()

`ssize_t fs_read_attr(int fd,  
const char* attribute,  
uint32 type,  
off_t pos,  
void* buffer,  
size_t count);`

Reads the attribute of the type and name specified by `type` and `attribute` from the file descriptor `fd`. The attribute's data is read starting at the offset `pos` and stored in the `buffer`. `count` specifies the maximum number of bytes to be read.

The `type` can be any of the standard BeOS type definitions, such as `B_STRING_TYPE`. See the header file `support/TypeConstants.h` for a complete list of these types. Note that the type of the attribute is a hint only; there's no reason you can't read a `B_STRING_TYPE` attribute as an integer (except that the data would probably not make any sense if you did).

If the attribute is read successfully, `fs_read_attr()` returns the actual number of bytes of data read from the attribute into the buffer. If an error occurs, this function returns -1 sets `errno` to an appropriate value.

Return Code| Description  
---|---  
`B_FILE_ERROR`.| Invalid file descriptor `fd` specified.  
`B_ENTRY_NOT_FOUND`.| No matching attribute was found for the specified file descriptor.  
  
#### fs_read_attr_dir()

`struct dirent* fs_read_attr_dir(DIR* dirp);`

Reads the current attribute from the specified attribute directory, and bumps the dirp so it points to the next attribute. The dirp pointer that you pass to this function should have been retrieved through a previous call to [`fs_open_attr_dir()`](TheStorageKit_Functions.md#fs_open_attr_dir "fs_open_attr_dir\(\), fs_fopen_attr_dir\(\)") or [`fs_fopen_attr_dir()`](TheStorageKit_Functions.md#fs_fopen_attr_dir).

A pointer to a dirent structure is returned. This structure contains information about the attribute. Do not dispose of or alter the data contained by this pointer; it belongs to the operating system.

Once a file's attribute directory has been opened, you can loop over this function to iteratively retrieve the names of all the attributes in the file. An attribute's name is recorded in the `d_name` field of the dirent structure; see the example in "[An Example](AttributeFunctions_Overview.md#AttributeFunctions_An_Example "An Example")".

This function doesn't let you get the type or value of an attribute. For that, use [`fs_stat_attr()`](TheStorageKit_Functions.md#fs_stat_attr "fs_stat_attr\(\)").

If you're looking for files that have a particular value for one or more attributes, however, you should try using the `fs_query` functions, which allow you to more easily establish complex search operations on the values of one or more attributes.

If an error occurs (including reaching the end of the directory), this function returns `NULL`.

Return Code| Description  
---|---  
`B_FILE_ERROR`.| Invalid directory reference specified.  
`B_NOT_A_DIRECTORY`.| The attribute directory is invalid.  
`B_ENTRY_NOT_FOUND`.| You've reached the end of the attribute directory.  
  
#### fs_remove_attr()

`int fs_remove_attr(int fd,  
const char* attribute);`

Deletes the named attribute from the file given by the file descriptor `fd`.

If the function is successful, `fs_remove_attr()` returns 0. Otherwise, it returns -1 and sets `errno` to an appropriate value.

Return Code| Description  
---|---  
`B_FILE_ERROR`.| `fd` is invalid, or file opened with read-only access.  
`B_BAD_VALUE`.| Invalid attribute name specified.  
`B_NOT_ALLOWED`.| Disk containing the file is read-only.  
`B_ENTRY_NOT_FOUND`.| No matching attribute was found.  
  
#### fs_rewind_attr_dir()

`int fs_rewind_attr_dir(DIR* dirp);`

Rewinds the attribute directory to the beginning. This lets you start over again at the top of a file's attribute directory and read your way down toward the bottom.

Returns a result code specifying whether or not the operation was successful.

Note

![Note](./images/admonitions/Info_32.png)

Unlike most of the other file system C functions, `fs_rewind_attr_dir()` doesn't set `errno`.

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_FILE_ERROR`.| Invalid directory reference specified.  
  
#### fs_stat_attr()

`int fs_stat_attr(int fd,  
const char* name,  
struct attr_info* info);`

Returns, in the attr_info structure pointed to by `info`, the type and size of the specified attribute on the file whose descriptor is specified by `fd`.

The attr_info structure is defined as follows:
    
    
    typedef struct attr_info
    {
       uint32 type;
       off_t  size;
    }

`type` contains a code defining the format of the data contained by the attribute; standard values for this field are defined in the `support/TypeConstants.h` header file.

`size` specifies the size of the data the attribute contains.

Upon success, the function returns 0. Otherwise, it returns -1 and sets `errno` to an appropriate value.

Return Code| Description  
---|---  
`B_ENTRY_NOT_FOUND`.| The requested attribute was not found for the file.  
`B_FILE_ERROR`.| Invalid file descriptor specified.  
  
#### fs_write_attr()

`ssize_t fs_write_attr(int fd,  
const char* attribute,  
uint32 type,  
off_t pos,  
const void* buffer,  
size_t count);`

Sets the value of the named attribute to the data in the specified buffer. The data's type is specified by type and should be one of the codes defined in `support/TypeConstants.h`. The length of the data to be written is specified by count.

Note

![Note](./images/admonitions/Info_32.png)

At this time, writing at an offset within an attribute is not fully supported, so you should always specify a `pos` of 0 to write at the beginning of the attribute.

If the attribute already has a value, this function wholly replaces that value with the new data—even if the new data is smaller than the existing data.

Warning

![Warning](./images/admonitions/Stop_32.png)

Under the Be File System, files have a special attribute storage area which contains the smaller attributes. When this space is filled up, or an attribute too large to fit into the space is added to the file, additional disk blocks are allocated for the new attributes. However, only one attribute is written per block in these additional blocks. The moral of the story is: use attributes wisely. They can be your friends, but if you overuse them, you'll bloat your files.

Upon success, the function returns the number of bytes that were actually written. Otherwise, it returns -1 and sets `errno` to an appropriate value.

Return Code| Description  
---|---  
`B_BAD_VALUE`.| The attribute name is too long or is an empty string.  
`B_NO_MEMORY`.| Insufficient memory to complete the operation.  
`B_FILE_ERROR`.| Invalid file descriptor `fd` specified, or the the file is read-only.  
  
* * *

### Index Functions

Declared In: `kernel/fs_index.h`

#### fs_close_index_dir()

`int fs_close_index_dir(DIR* dirp);`

Closes the specified index directory and frees dirp. You should pass into this function the pointer returned from a previous [`fs_open_index_dir()`](TheStorageKit_Functions.md#fs_open_index_dir "fs_open_index_dir\(\)").

You should always use this function to close index directories after you finish using them.

If successful, this function returns 0; otherwise it returns -1 and sets `errno` to one of the following codes.

Return Code| Description  
---|---  
`B_FILE_ERROR`.| Invalid directory reference specified.  
`B_BAD_FILE`.| Invalid directory reference specified.  
  
#### fs_create_index()

`int fs_create_index(dev_t device,  
const char* name,  
int type,  
uint flags);`

Creates a new index called `name` on the specified `device`. Once this has been done, adding an attribute named `name` to a file causes the file to be added to the name index, such that subsequent queries will be able to search for files that contain the name attribute.

`type` indicates the kind of data the attribute will contain. Standard types are defined in the `support/TypeConstants.h` header file. Different file systems may support different types. BFS supports the following types of attributes:

  * `B_INT32_TYPE`

  * `B_INT64_TYPE`

  * `B_FLOAT_TYPE`

  * `B_DOUBLE_TYPE`

  * `B_STRING_TYPE`

  * `B_MIME_STRING_TYPE`

`flags` is currently unused and should always be 0.

If successful, this function returns 0; otherwise it returns -1 and sets `errno` to one of the following codes.

Return Code| Description  
---|---  
`B_BAD_VALUE`.| The device does not exist, or `name` is reserved.  
`B_NOT_ALLOWED`.| The device is read-only.  
`B_NO_MEMORY`.| Insufficient memory to complete the operation.  
`B_FILE_EXISTS`.| The index `name` already exists.  
`B_DEVICE_FULL`.| There's not enough room on the device to create the index.  
`B_FILE_ERROR`.| Invalid directory reference.  
`B_ERROR`.| The index type passed isn't supported.  
  
#### fs_open_index_dir()

`DIR* fs_open_index_dir(dev_t device);`

Opens the index directory for the volume identified by device. Once open, you can retrieve the names of the indices on the volume by calling [`fs_read_index_dir()`](TheStorageKit_Functions.md#fs_read_index_dir "fs_read_index_dir\(\)").

When you have finished using the index directory, call [`fs_close_index_dir()`](TheStorageKit_Functions.md#fs_close_index_dir "fs_close_index_dir\(\)") to close it.

If the index directory is opened successfully, a pointer to a directory structure is returned. This pointer should be passed to the other `fs_index` functions to read entries from the index directory, as well as to close the directory when you're finished with it.

If an error occurs while opening the index directory, this function returns `NULL` and sets `errno` to an appropriate value.

Return Code| Description  
---|---  
`B_FILE_ERROR`.| A file error prevented the operation.  
`B_BAD_VALUE`.| Invalid `device` number specified.  
`B_LINK_LIMIT`.| A cyclic loop was detected in the directory structure.  
`B_NO_MEMORY`.| Insufficient memory to complete the operation.  
`B_BUSY`.| The specified file is currently in use.  
`B_NO_MORE_FDS`.| Too many open files; all file descriptors are in use.  
  
#### fs_read_index_dir()

`struct dirent* fs_read_index_dir(DIR* dirp);`

Reads the current entry from the open index directory referenced by `dirp`, and bumps the pointer to point to the next entry. `dirp` should have been obtained through a previous call to [`fs_open_index_dir()`](TheStorageKit_Functions.md#fs_open_index_dir "fs_open_index_dir\(\)"). The returned dirent pointer contains information about the index entry, including the name of the attribute represented by the index. This pointer belongs to the system; you must not delete it.

Through repeated calls to `fs_read_index_dir()`, you can obtain a list of all the indices available on the device. When you reach the end of the list, `errno` is set to `B_ENTRY_NOT_FOUND`.

If an error occurs, this function returns `NULL` and sets `errno` to an appropriate value.

Return Code| Description  
---|---  
`B_FILE_ERROR`.| Invalid file descriptor `fd` specified.  
`B_NOT_A_DIRECTORY`.| `dirp` does not represent a valid index directory.  
`B_ENTRY_NOT_FOUND`.| You've reached the end of the list.  
  
#### fs_remove_index()

`int fs_remove_index(dev_t device,  
const char* index_name);`

Deletes the index named `index_name` from the specified `device`. Once the index is deleted, it will no longer be possible to use the query system to search for files with the corresponding attribute.

Use this function to remove an index that you no longer wish or need to be able to search upon. For example, if your application is being uninstalled by your user-friendly uninstaller program, and it's no longer meaningful to be able to search on a given attribute, you should use this function to delete the index for that attribute.

You should be careful when deciding whether or not to delete an index, however. If the user still has files around that they want to be able to search, using the Tracker's Find panel, for instance, and you've deleted the index for that attribute, they'll be most displeased. There's a grey area you need to wade through in determining whether or not to delete your indices; your decision needs to be based on the specifics of what your application does and how it will be used.

If the index is removed successfully, `fs_remove_index()` returns 0. Otherwise, it returns -1 and sets `errno` to an appropriate value.

Return Code| Description  
---|---  
`B_FILE_ERROR`.| A file system error prevented the operation.  
`B_BAD_VALUE`.| Invalid `device` number specified.  
`B_NOT_ALLOWED`.| Can't remove a system-reserved index ("name," "size," "last_modified"), or the device is read-only.  
`B_NO_MEMORY`.| Insufficient memory to complete the operation.  
`B_ENTRY_NOT_FOUND`.| The specified index does not exist.  
  
#### fs_rewind_index_dir()

`int fs_rewind_index_dir(DIR* dirp);`

Rewinds the specified index directory to the beginning of its list of indices. This allows you to start over again at the top of a device's index directory and make your way down toward the bottom.

Returns a result code specifying whether or not the operation was successful.

Note

![Note](./images/admonitions/Info_32.png)

Unlike the most of the other file system functions, `fs_rewind_index_dir()` doesn't set `errno`.

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_FILE_ERROR`.| Invalid directory reference specified.  
  
#### fs_stat_index()

`int fs_stat_index(dev_t device,  
const char* index_name,  
struct index_info* info);`

Returns, in the index_info structure pointed to by `info`, information about the index named `index_name` on the specified device.

The index_info structure is defined as follows:
    
    
    typedef struct index_info
    {
       uint32  type;
       off_t   size;
       time_t  modification_time;
       time_t  creation_time;
       uid_t   uid;
       gid_t   gid;
    }

Field| Description  
---|---  
`type`| Contains a code defining the format of the data contained by the attribute represented by the index; standard values for this field are defined in the `support/TypeConstants.h` header file.  
`size`| Specifies the size of the data the attribute contains.  
`modification_time`| Contains the time the index was last changed, in seconds since January 1, 1970.  
`creation_time`| Contains the date and time the index was originally created, in seconds since January 1, 1970.  
`uid`| Contains the user ID of the owner of the index.  
`gid`| Contains the group ID of the owner of the index.  
  
If the function is successful, it returns 0; otherwise, it returns -1 and sets `errno` to an appropriate value

Return Code| Description  
---|---  
`B_ENTRY_NOT_FOUND`.| The requested index was not found.  
`B_BAD_VALUE`.| Invalid device number specified.  
  
* * *

### Query Functions

Declared In: `kernel/fs_query.h`

#### fs_close_query()

`int fs_close_query(DIR* dir);`

Closes a query which was previously opened using the [`fs_open_query()`](TheStorageKit_Functions.md#fs_open_query "fs_open_query\(\)") function. You pass in the DIR* returned by either of these functions. The pointer `dir` is freed by this function.

If the query closes successfully, `fs_close_query()` returns 0; otherwise, it returns -1 and sets `errno` to an appropriate value.

Return Code| Description  
---|---  
`B_FILE_ERROR`.|  A file system error prevented the operation from succeeding.  
  
#### fs_open_query()

`DIR* fs_open_query(dev_t device,  
const char* query,  
uint32 flags);`

Opens a new query on the specified device. The `query` string is the criteria or "predicate" that describes the files that you're looking for. For information on how to construct the query string, see the [`BQuery`](BQuery.md "BQuery") class. Note that you can't use the "push" method: `fs_open_query()` only understands predicate strings.

`flags` is currently unused; pass 0 as its value.

The pointer returned by this function is used to identify your query to the other query functions; you should not dispose of it yourself—this will be done for you when you call [`fs_close_query()`](TheStorageKit_Functions.md#fs_close_query "fs_close_query\(\)"). If the query couldn't be opened, the function returns `NULL` and sets `errno` to an appropriate value.

Return Code| Description  
---|---  
`B_BAD_VALUE`.| `device` does not specify a valid device, `query` is `NULL` or an invalid expression, or a live query was requested without specifying a valid port.  
`B_NO_MEMORY`.| Insufficient memory to complete the operation.  
`B_FILE_ERROR`.| A file system error prevented the operation from succeeding.  
`B_BUSY`.| A busy node could not be accessed.  
`B_NO_MORE_FDS`.| All file descriptors are in use (too many open files).  
`E2BIG`.| query expression is too big.  
  
#### fs_read_query()

`struct dirent* fs_read_query(DIR* d);`

Returns the next item that matches the specified query. The `d` argument should have been gotten from a previous call to [`fs_open_query()`](TheStorageKit_Functions.md#fs_open_query "fs_open_query\(\)"); it identifies the particular query from which to read.

You mustn't free the pointer returned to you by this function.

If an error occurs while reading the query, this function returns `NULL` and sets `errno` to an appropriate value.

Return Code| Description  
---|---  
`B_FILE_ERROR`.| A file system error prevented the operation from succeeding.  
`B_BAD_VALUE`.| An error occurred processing the query predicate.  
`B_ENTRY_NOT_FOUND`.| No more matches.  
`B_NOT_A_DIRECTORY`.| A non-directory node was found where a directory was expected.  
`B_INTERRUPTED`.| A signal interrupted the read.  
`E2BIG`.| Query predicate is too big.  
  
* * *

### FileSystem Information Functions

Declared In: `kernel/fs_info.h`

#### dev_for_path()

`dev_t dev_for_path(const char* path);`

Given a pathname, returns the device number of the device on which the path is located. If the result is negative, it is a return code specifying an error.

Return Code| Description  
---|---  
`B_ENTRY_NOT_FOUND`.| `path` does not exist, or is `NULL` or an empty string.  
`B_BAD_VALUE`.| `path` is `NULL` or an empty string.  
`B_NAME_TOO_LONG`.| `path` is too long.  
`B_NO_MEMORY`.| Insufficient memory to complete the operation.  
`B_FILE_ERROR`.| A file system error prevented the operation from succeeding.  
  
#### next_dev()

`dev_t next_dev(int32* pos);`

The `next_dev()` function allows you to iterate through all devices, receiving their device numbers as a result each time. If the result is negative, it is an error code. When the end of the device list is reached, the return value `B_BAD_VALUE` is returned.

You should initially set `pos` to 0, then call `next_dev()` in a loop to obtain each device number until an error occurs. For example:
    
    
    void ScanDevices(void) {
       int pos;
    
       pos = 0;
       while(next_dev(&pos) >=0) {
          do_something(pos);
       }
    }

Return Code| Description  
---|---  
`B_BAD_VALUE`.| No matching device found.  
  
#### fs_stat_dev()

`int fs_stat_dev(dev_t dev,  
fs_info* info);`

`fs_stat_dev()` returns information about the specified device. This can be used in conjunction with [`next_dev()`](TheStorageKit_Functions.md#next_dev "next_dev\(\)") to scan all devices and record information your application requires.

The information about the device is returned in the [fs_info()](TheStorageKit_DefinedTypes.md#fs_info "fs_info") structure `info`

This function returns 0 if the request was successful or -1 if an error occurred. Use the `errno()` function to determine what `error` in particular occurred.

Return Code| Description  
---|---  
`B_OK`.| The device was found; `info` contains valid information.  
`B_BAD_VALUE`.| `dev` doesn't identify an existing device.  
  
* * *

### MimeType Functions

Declared In: `storage/Mime.h`

#### create_app_meta_mime()

`status_t create_app_meta_mime(const char* path,  
int recursive,  
int synchronous,  
int force);`

Creates an entry in the File Types database for a specific application, or for all applications:

  * To create an entry for a specific application, pass the path to the application in `path`. Information is taken from the app and written into the database.

  * To create entries for all applications, pass a `NULL` path.

The `recursive` flag is currently unused.

If `synchronous` is `true`, the function doesn't return until the operation is complete. If it's `false`, the function returns immediately while the operation continues in the background.

If `force` is `true`, entries are created even if they already exist.

Return Code| Description  
---|---  
`B_OK`.| No error.  
`B_BAD_VALUE`.| You can't request recursive operation when a path is specified.  
  
#### get_device_icon()

`status_t get_device_icon(const char* device,  
void* icon,  
int32 size);`

Returns the icons that are associated with the given device. You specify which icon you want (large or small) by passing 32 or 16 as the `size` argument. The icon is then returned through the `icon` argument.

See Also: [`BVolume::GetIcon()`](BVolume.md#BVolume_GetIcon "GetIcon\(\)")

#### update_mime_info()

`int update_mime_info(const char* path,  
int recursive,  
int synchronous,  
int force);`

Updates the MIME information for one or more files, specified by `path`. If `path` is `NULL`, all files are scanned recursively, and the value of `recursive` is ignored.

If `recursive` is `true` and `path` indicates a directory, the directory tree contained by `path` is scanned, with every file in the tree being updated; otherwise, just the file indicated by `path` is updated.

If `synchronous` is `true`, `update_mime_info()` doesn't return until the update operation is complete. If it's `false`, `update_mime_info()` runs asynchronously, and returns immediately while updating continues in the background.

If `force` is `true`, files are updated even if they've been updated already.

If an error occurs, `update_mime_info()` returns a negative error code; otherwise, it returns `B_OK`.

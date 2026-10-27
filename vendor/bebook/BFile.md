<!-- Classes And Methods - The Storage Kit - BFile -->

## BFile

| Derived From:| [`BNode`](BNode.md "BNode"), [`BPositionIO`](BPositionIO.md "BPositionIO")  
---|---  
Mix-in Classes:| None  
Declared In:| `storage/File.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BFile_Overview.md "BFile")

  * [Constructor and Destructor](BFile.md#BFile_ConstructorDestructor)
  * [Member Functions](BFile.md#BFile_Member_Functions)
  * [Operators](BFile.md#BFile_Operators)

  
  
* * *

### Constructor and Destructor

#### BFile()

`BFile();` `BFile(const entry_ref* ref,  
uint32 openMode);` `BFile(const [`BEntry`](BEntry.md "BEntry")* entry,  
uint32 openMode);` `BFile(const char* path,  
uint32 openMode);` `BFile([`BDirectory`](BDirectory.md "BDirectory")* dir,  
const char* path,  
uint32 openMode);`

Creates a new `BFile` object, initializes it according to the arguments, and sets [`InitCheck()`](BFile.md#BFile_InitCheck "InitCheck\(\)") to return the status of the initialization.

The default constructor does nothing and sets and sets [`InitCheck()`](BFile.md#BFile_InitCheck "InitCheck\(\)") to `B_NO_INIT`. To initialize the object, call [`SetTo()`](BFile.md#BFile_SetTo "SetTo\(\), Unset\(\)").

The copy constructor creates a new `BFile` that's open on the same file as that of the argument. Note that the two objects maintain _separate_ data pointers into the _same_ file:

  * Separate pointers: Reading and writing through one object _does not_ affect the position of the data pointer in the other object.

  * Same file: If one object writes to the file, the other object will see the written data.

For information on the other constructors, see the analogous [`SetTo()`](BFile.md#BFile_SetTo "SetTo\(\), Unset\(\)") functions.

#### ~BFile()

`virtual ~BFile();`

Closes the object's file, frees its file descriptor, and destroys the object.

* * *

### Member Functions

####  GetSize(), SetSize()

`virtual status_t GetSize(off_t* size) const;``virtual status_t SetSize(off_t size);`

These functions get and set the size, in bytes, of the object's file.

`GetSize()` returns the size of the file's data portion in the `size` argument; the measurement doesn't include attributes.

`SetSize()` sets the size of the data portion to the size given by the argument:

  * Enlarging a file adds (uninitialized) bytes to its end.

  * Shrinking a file removes bytes from the end.

Return Code| Description  
---|---  
`B_OK`.| The file's size was successfully gotten or set.  
`B_NOT_ALLOWED`.| (`SetSize()`) The file lives on a read-only volume.  
`B_DEVICE_FULL`.| (`SetSize()`) No more room on the file's device.  
  
#### InitCheck()

`status_t InitCheck() const;`

Returns the status of the most recent initialization.

Return Code| Description  
---|---  
`B_OK`.| The object is initialized.  
`B_NO_INIT`.| The object is uninitialized.  
For other errors.| See [`SetTo()`](BFile.md#BFile_SetTo "SetTo\(\), Unset\(\)")  
  
####  IsReadable(), IsWritable()

`bool IsReadable() const;``bool IsWritable() const;`

These functions tell you whether the `BFile` was initialized to read or write its file. If the object isn't (properly) initialized, they both return `false`.

Note that these functions don't query the actual file to check permissions, they only tell you what the access request was when the `BFile` object was initialized.

####  Read(), ReadAt(), Write(), WriteAt()

`virtual ssize_t Read(void* buffer,  
size_t size);``virtual ssize_t ReadAt(off_t location,  
void* buffer,  
size_t size);``virtual ssize_t Write(const void* buffer,  
size_t size);``virtual ssize_t WriteAt(off_t location,  
const void* buffer,  
size_t size);`

These functions, which are inherited from [`BPositionIO`](BPositionIO.md "BPositionIO"), read and write the file's data; note that they don't touch the file's attributes.

The `Read()` and `ReadAt()` functions read `size` bytes of data from the file and place this data in `buffer`. The buffer that `buffer` points to must already be allocated, and must be large enough to accommodate the read data. Note that the read-into buffer is _not_ null-terminated by the reading functions.

The two functions differ in that…

  * `Read()` reads the data starting at the current location of the file's data pointer, and increments the file pointer as it reads.

  * `ReadAt()` reads the data from the location specified by the `location` argument, which is taken as a measure in bytes from the beginning of the file. `ReadAt()` does not bump the file's data pointer.

`Write()` and `WriteAt()` write `size` bytes of data into the file; the data is taken from the `buffer` argument. The two functions differ in their use (or non-use) of the file's data pointer in the same manner as `Read()` and `ReadAt()`.

All four functions return the number of bytes that were actually read or written; negative return values indicate an error.

Reading fewer-than-`size` bytes isn't uncommon—consider the case where the file is smaller than the size of your buffer. If you want your buffer to be `NULL`-terminated, you can use the return value to set the `NULL`:
    
    
    char buf[1024];
    ssize_t amt_read;
    
    if ((amt_read = file.Read((void *)buf, 1024)) < 0)
       /* handle errors first */
    else
       /* otherwise set null */
       buf[amt_read] = '0';

A successful `Write()` or `WriteAt()`, on the other hand, will always write exactly the number of bytes you requested. In other words, `Write()` returns either the `size` value that you passed to it, or else it returns a negative (error) value.

Note

![Note](./images/admonitions/Info_32.png)

Error codes returned by these functions can vary depending on the file system handling the operation. For this reason, specific error codes aren't listed here.

####  Seek(), Position()

`virtual off_t Seek(off_t offset,  
int32 seekMode);``virtual off_t Position() const;`

`Seek()` sets the location of the file's data pointer. The new location is reckoned as `offset` bytes from the position given by the `seekMode` constant:

Constant| Description  
---|---  
`SEEK_SET`| Seek from the beginning of the file.  
`SEEK_CUR`| Seek from the pointer's current position.  
`SEEK_END`| Seek from the end of the file.  
  
If you `Seek()` to a position that's past the end of the file and then do a [`Write()`](BFile.md#BFile_Write), the file will be extended (padded with garbage) from the old end of file to the `Seek()`'d position. If you don't follow the `Seek()` with a `Write()`, the file isn't extended.

`Seek()` returns the new position as measured (in bytes) from the beginning of the file.

`Position()` returns the current position as measured (in bytes) from the beginning of the file. It doesn't move the pointer.

Return Code| Description  
---|---  
`B_ERROR`.| Attempted to `Seek()` "before" the beginning of the file, or you called `Position()` after such a `Seek()`. You also get `B_ERROR` if you call `Seek()` on an uninitialized file.  
`B_BAD_FILE`.| `Position()` called on an uninitialized file.  
  
Warning

![Warning](./images/admonitions/Stop_32.png)

If you do a "before the beginning" seek, subsequent [`Read()`](BFile.md#BFile_Read "Read\(\), ReadAt\(\), Write\(\), WriteAt\(\)") and [`Write()`](BFile.md#BFile_Write) calls do not fail. But they almost certainly aren't doing what you want (you shouldn't be "before the file," anyway). The moral: Always check your `Seek()` return.

####  SetTo(), Unset()

`status_t SetTo(const entry_ref* ref,  
uint32 openMode);``status_t SetTo(const [`BEntry`](BEntry.md "BEntry")* entry,  
uint32 openMode);``status_t SetTo(const char* path,  
uint32 openMode);``status_t SetTo(const [`BDirectory`](BDirectory.md "BDirectory")* dir,  
const char* path,  
uint32 openMode);``void Unset();`

Closes the `BFile`'s current file (if any), and opens the file specified by the arguments. If the specified file is a symbolic link, the link is automatically traversed (recursively, if necessary). Note that you're not prevented from opening a directory as a `BFile`, but you are prevented from writing to it.

  * In the `path` function, `path` can be absolute or relative, and can contain "." and ".." elements. If `path` is relative, it's reckoned off of the current working directory.

  * In the `dir`/`path` function, `path` must be relative and is reckoned off of `dir`.

`openMode` is a combination of flags that determines how the file is opened and what this object can do with it once it is open. There are two sets of flags; you must pass one (and only one) of the following "read/write" constants:

Constant| Description  
---|---  
`B_READ_ONLY`| This object can read, but not write, the file.  
`B_WRITE_ONLY`| This object can write, but not read, the file.  
`B_READ_WRITE`| This object can read and write the file.  
  
You can also pass any number of the following (these are optional):

Constant| Description  
---|---  
`B_CREATE_FILE`| Create the file if it doesn't already exist.  
`B_FAIL_IF_EXISTS`| If the file already exists, the initialization (of the `BFile` object) fails.  
`B_ERASE_FILE`| If the file already exists, erase all its data and attributes.  
`B_OPEN_AT_END`| Sets the data pointer to point to the end of the file.  
  
To open a file for reading and writing, for example, you simply pass:
    
    
    file.SetTo(entry, B_READ_WRITE);

Here we create a new file or erase its data if it already exists:
    
    
    file.SetTo(entry, B_READ_WRITE | B_CREATE_FILE | B_ERASE_FILE);

And here we create a new file, but only if it doesn't already exist:
    
    
    file.SetTo(entry, B_READ_WRITE | B_CREATE_FILE | B_FAIL_IF_EXISTS);

`Unset()` closes the object's file and sets its [`InitCheck()`](BFile.md#BFile_InitCheck "InitCheck\(\)") value to `B_NO_INIT`.

Return Code| Description  
---|---  
`B_OK`.| The file was successfully opened.  
`B_BAD_VALUE`.| `NULL` `path` in `dir`/`path`, or some other argument is uninitialized.  
`B_ENTRY_NOT_FOUND`.| File not found, or couldn't create the file.  
`B_FILE_EXISTS`.| File exists (and you set `B_FAIL_IF_EXISTS`).  
`B_PERMISSION_DENIED`.| Read or write permission request denied.  
`B_NO_MEMORY`.| Couldn't allocate necessary memory to complete the operation.  
  
* * *

### Operators

#### = (assignment)

`BFile& operator=(const [`BFile`](BFile.md "BFile")& file);`

In the expression
    
    
    BFile a = b;

`BFile` `a` is initialized to refer to the same file as `b`. To gauge the success of the assignment, you should call [`InitCheck()`](BFile.md#BFile_InitCheck "InitCheck\(\)") immediately afterwards. You can't assign a `BFile` to itself (`B_BAD_VALUE`).

Assigning to an uninitialized `BFile` is "successful": The assigned-to `BFile` will also be uninitialized (`B_NO_INIT`).

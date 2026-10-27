<!-- Classes And Methods - The Support Kit - BMallocIO -->

## BMallocIO

| Derived From:|  [`BPositionIO`](BPositionIO.md "BPositionIO")  
---|---  
Mix-in Classes:| –  
Declared In:| `support/DataIO.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BMallocIO_Overview.md "BMallocIO")

  * [Constructor and Destructor](BMallocIO.md#BMallocIO_ConstructorDestructor)
  * [Member Functions](BMallocIO.md#BMallocIO_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BMallocIO

`BMallocIO();`

The `BMallocIO` constructor creates an empty object and sets the default block size to 256 bytes. The constructor doesn't allocate any memory; memory is allocated when you first write to the object or when you call [`SetSize()`](BMallocIO.md#BMallocIO_SetSize) to set the amount of memory.

#### ~BMallocIO()

`virtual ~BMallocIO();`

The `BMallocIO` destructor frees all memory that was allocated by the object.

* * *

### Member Functions

####  Buffer(), BufferLength()

`const void* Buffer() const;``size_t BufferLength() const;`

`Buffer()` returns a pointer to the memory that the `BMallocIO` object has allocated, or `NULL` if it hasn't yet had occasion to allocate any memory.

`BufferLength()` returns the number of data bytes in the buffer (not necessarily the full number of bytes that were allocated).

#### ReadAt()

`virtual ssize_t ReadAt(off_t position,  
void* buffer,  
size_t numBytes);`

Reads up to `numBytes` bytes of data from the object and copies it into the `buffer`. Returns the actual number of bytes placed in the buffer. The data is read beginning at the `position` offset.

This function doesn't read beyond the end of the data. If there are fewer than `numBytes` of data available at the `position` offset, it reads only through the last data byte and returns a smaller number than `numBytes`. If `position` is out of range, it returns 0.

####  Seek(), Position()

`virtual off_t Seek(off_t position,  
int32 mode);``virtual off_t Position() const;`

`Seek()` sets the position in the data buffer where the [`Read()`](BPositionIO.md#BPositionIO_Read "Read\(\), ReadAt\(\)") and [`Write()`](BPositionIO.md#BPositionIO_Write "Write\(\), WriteAt\(\)") functions (inherited from [`BPositionIO`](BPositionIO.md "BPositionIO")) begin reading and writing. How the `position` argument is understood depends on the `mode` flag. There are three possible modes:

Constant| Description  
---|---  
`SEEK_SET`.| The `position` passed is an offset from the beginning of allocated memory; in other words, the current position is set to `position`. For this mode, `position` should be a positive value.  
`SEEK_CUR`.| The `position` argument is an offset from the current position; the value of the argument is added to the current position.  
`SEEK_END`.| The `position` argument is an offset from the end of the object's data (not necessarily from the end of allocated memory). Positive values seek beyond the end of the data; negative values seek backwards into the data.  
  
Attempts to seek beyond the end of allocated memory are legal: When [`Write()`](BDataIO.md#BDataIO_Write "Write\(\)") is subsequently called, the object updates its conception of where the data ends to bring the current position within range. If necessary, enough memory will be allocated to accommodate any data added at the current position.

Both `Seek()` and `Position()` return the current position as an offset in bytes from the beginning of allocated memory.

####  SetBlockSize(), SetSize()

`void SetBlockSize(size_t blockSize);``virtual status_t SetSize(off_t numBytes);`

`SetBlockSize()` sets the size of the memory blocks that the `BMallocIO` object deals with. The object allocates memory in multiples of the block size. The default is 256 bytes.

`SetSize()` sets the size of allocated memory to `numBytes` (modulo the block size). Shrinking the buffer should always be successful (`B_OK`); if the buffer can't be grown, `B_NO_MEMORY` is returned.

#### WriteAt()

`virtual ssize_t WriteAt(off_t position,  
const void* buffer,  
size_t numBytes);`

Copies `numBytes` bytes of data from `buffer` into the object's data beginning at `position`.

A successful `WriteAt()` always returns `numBytes` \- `WriteAt()` reallocates the buffer (in multiples of the block size) if it needs more room. If the reallocation fails, this function returns `B_NO_MEMORY`.

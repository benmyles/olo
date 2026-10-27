<!-- Classes And Methods - The Support Kit - BMemoryIO -->

## BMemoryIO

| Derived From:|  [`BPositionIO`](BPositionIO.md "BPositionIO")  
---|---  
Mix-in Classes:| –  
Declared In:| `support/DataIO.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BMemoryIO_Overview.md "BMemoryIO")

  * [Constructor and Destructor](BMemoryIO.md#BMemoryIO_ConstructorDestructor)
  * [Member Functions](BMemoryIO.md#BMemoryIO_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BMemoryIO()

`BMemoryIO(void* buffer,  
size_t numBytes);` `BMemoryIO(const void* buffer,  
size_t numBytes);`

The `BMemoryIO` constructor assigns the object a buffer with at least `numBytes` of available memory. If the buffer is declared _const_ , the object is read-only; calls to [`Write()`](BPositionIO.md#BPositionIO_Write "Write\(\), WriteAt\(\)") (inherited from [`BPositionIO`](BPositionIO.md "BPositionIO")) and [`WriteAt()`](BMemoryIO.md#BMemoryIO_WriteAt "WriteAt\(\)") will fail. Otherwise, the buffer can be both read and written. In either case, the caller retains responsibility for the buffer; the `BMemoryIO` object won't free it.

#### ~BMemoryIO()

`virtual ~BMemoryIO();`

The `BMemoryIO` destructor does nothing.

* * *

### Member Functions

#### ReadAt()

`virtual ssize_t ReadAt(off_t position,  
void* buffer,  
size_t numBytes);`

Copies up to `numBytes` bytes of data from the object into the `buffer` and returns the actual number of bytes placed in the buffer. The data is read beginning at the `position` offset.

This function doesn't read beyond the end of the data. If there are fewer than `numBytes` of data available at the `position` offset, it reads only through the last data byte and returns a smaller number than `numBytes`. If `position` is out of range, it returns 0.

####  Seek(), Position()

`virtual off_t Seek(off_t position,  
int32 mode);``virtual off_t Position() const;`

`Seek()` sets the position in the data buffer where the [`Read()`](BPositionIO.md#BPositionIO_Read "Read\(\), ReadAt\(\)") and [`Write()`](BPositionIO.md#BPositionIO_Write "Write\(\), WriteAt\(\)") functions (inherited from [`BPositionIO`](BPositionIO.md "BPositionIO")) begin reading and writing. How the `position` argument is understood depends on the `mode` flag. There are three possible modes:

Constant| Description  
---|---  
`SEEK_SET`.| The `position` passed is an offset from the beginning of allocated memory; in other words, the current position is set to `position`. For this mode, `position` should be a positive value.  
`SEEK_CUR`.| The `position` argument is an offset from the current position; the value of the argument is added to the current position.  
`SEEK_END`.| The `position` argument is an offset from the end of the object's data buffer. In this mode the `position` argument must be negative.  
  
[`Write()`](BPositionIO.md#BPositionIO_Write "Write\(\), WriteAt\(\)") fails if the current position is beyond the end of assigned memory.

Both `Seek()` and `Position()` return the current position as an offset in bytes from the beginning of allocated memory.

#### WriteAt()

`virtual ssize_t WriteAt(off_t position,  
const void* buffer,  
size_t numBytes);`

Copies `numBytes` bytes of data from `buffer` into allocated memory beginning at `position`, and returns the number of bytes written.

`WriteAt()` won't write outside the memory buffer. If `position` is beyond the end of the buffer, it returns 0. If the object is read-only, it returns `B_NOT_ALLOWED`.

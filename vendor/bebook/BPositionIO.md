<!-- Classes And Methods - The Support Kit - BPositionIO -->

## BPositionIO

| Derived From:|  [`BDataIO`](BDataIO.md "BDataIO")  
---|---  
Mix-in Classes:| –  
Declared In:| `support/DataIO.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BPositionIO_Overview.md "BPositionIO")

  * [Constructor and Destructor](BPositionIO.md#BPositionIO_ConstructorDestructor)
  * [Member Functions](BPositionIO.md#BPositionIO_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BPositionIO()

`BPositionIO();`

Does nothing. Constructors in derived classes should initialize the object to default values—for example, set the current position to the beginning of the data.

#### ~BPositionIO()

`virtual ~BPositionIO();`

Does nothing.

* * *

### Member Functions

####  Read(), ReadAt()

`virtual ssize_t Read(void* buffer,  
size_t numBytes);``virtual ssize_t ReadAt(off_t position,  
void* buffer,  
size_t numBytes) = 0;`

`Read()` copies `numBytes` bytes of data from the object to the `buffer`. It returns the number of bytes actually read (which may be 0) or an error code if something goes wrong. `Read()` is implemented using [`Seek()`](BPositionIO.md#BPositionIO_Seek "Seek\(\), Position\(\)"), `ReadAt()`, and [`Position()`](BPositionIO.md#BPositionIO_Position), all of which are pure virtual functions.

`ReadAt()` is a pure virtual that must be implemented by derived classes to read `numBytes` bytes of data beginning at `position` in the data source, and to place them in the `buffer`. It should return the number of bytes actually read, or an error code if something goes wrong.

####  Seek(), Position()

`virtual off_t Seek(off_t position,  
int32 mode) = 0;``virtual off_t Position() const = 0;`

`Seek()` is implemented by derived classes to modify the current position maintained by the object. The current position is an offset in bytes from the beginning of the object's data. How the `position` argument is interpreted will depend on the `mode` flag. Three possible modes should be supported:

Constant| Description  
---|---  
`SEEK_SET`.| The `position` passed is an offset from the beginning of allocated memory; in other words, the current position should be set to `position`.  
`SEEK_CUR`.| The `position` argument is an offset from the current position; the current position should be incremented by `position`.  
`SEEK_END`.| The `position` argument is an offset from the end of the data; the current position should be the sum of `position` plus the number of bytes in the data.  
  
For the `SEEK_SET` mode, `position` should be a positive value. The other modes permit negative offsets.

`Seek()` should return the new position.

`Position()` should return the current position.

#### SetSize()

`virtual status_t SetSize(off_t numBytes);`

Returns `B_ERROR` to indicate that, in general, `BPositionIO` objects can't set the amount of memory in the repositories they represent. However, the [`BMallocIO`](BMallocIO.md "BMallocIO") class in this kit and [`BFile`](BFile.md "BFile") in the [Storage Kit](TheStorageKit.md "The Storage Kit") implement `SetSize()` functions that override this default.

####  Write(), WriteAt()

`virtual ssize_t Write(const void* buffer,  
size_t numBytes);``virtual ssize_t WriteAt(off_t position,  
const void* buffer,  
size_t numBytes) = 0;`

`Write()` copies `numBytes` bytes of data from the object into the `buffer`. It returns the number of bytes actually written (which may be 0) or an error code if something goes wrong. `Write()` is implemented using [`Seek()`](BPositionIO.md#BPositionIO_Seek "Seek\(\), Position\(\)"), [`Position()`](BPositionIO.md#BPositionIO_Position), and `WriteAt()`, all of which are pure virtual functions.

`WriteAt()` is a pure virtual that must be implemented by derived classes to copy `numBytes` bytes of data from `buffer` and write them into the object, starting at byte `position` within the object. It should return the number of bytes actually read, or an error code if something goes wrong.

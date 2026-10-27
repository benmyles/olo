<!-- Classes And Methods - The Support Kit - BDataIO -->

## BDataIO

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `support/DataIO.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BDataIO_Overview.md "BDataIO")

  * [Constructor and Destructor](BDataIO.md#BDataIO_ConstructorDestructor)
  * [Member Functions](BDataIO.md#BDataIO_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BDataIO()

`BDataIO();`

Does nothing.

#### ~BDataIO()

`virtual ~BDataIO();`

Does nothing.

* * *

### Member Functions

#### Read()

`virtual ssize_t Read(void* buffer,  
size_t numBytes) = 0;`

`Read()` is implemented by derived classes to copy `numBytes` bytes of data from the object to the `buffer`. It should return the number of bytes actually read, which may be 0, or an error code if something goes wrong.

#### Write()

`virtual ssize_t Write(const void* buffer,  
size_t numBytes) = 0;`

`Write()` is implemented by derived classes to copy `numBytes` bytes of data from the `buffer` to the object. It should return the number of bytes actually written, which may be 0, or an error code if the operation fails.

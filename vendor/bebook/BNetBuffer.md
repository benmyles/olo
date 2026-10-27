<!-- Classes And Methods - The Network Kit - BNetBuffer -->

## BNetBuffer

| Derived From:| None  
---|---  
Mix-in Classes:| [`BArchivable`](BArchivable.md "BArchivable")  
Declared In:| `net/NetBuffer.h`  
Library:| `libnetapi.so`  
Allocation:| Constructor only  
[Class Overview](BNetBuffer_Overview.md "BNetBuffer")

  * [Constructor and Destructor](BNetBuffer.md#BNetBuffer_ConstructorDestructor)
  * [Member Functions](BNetBuffer.md#BNetBuffer_MemberFunctions)
  * [Operators](BNetBuffer.md#BNetBuffer_Operators)

  
  
* * *

### Constructor and Destructor

#### BNetBuffer()

`BNetBuffer(size_t size = 0);` `BNetBuffer(const BNetBuffer& from);` `BNetBuffer([`BMessage`](BMessage.md "BMessage")* archive);`

Creates a `BNetBuffer`. The first form creates a new buffer capable of holding up to `size` bytes of data. In this case, the buffer begins life empty.

The second form creates a new buffer which is an exact duplicate of the `BNetBuffer` specified by the `from` argument, including any data that might already be in the buffer. The third form reconstructs an archived `BNetBuffer`.

#### ~BNetBuffer()

`virtual ~BNetBuffer();`

A typical destructor.

* * *

### Member Functions

####  AppendInt8(), AppendUint8(), AppendInt16(), AppendUint16(), AppendInt32(), AppendUint32(), AppendInt64(), AppendUint64(), AppendFloat(), AppendDouble(), AppendString(), AppendData(), AppendMessage()

`status_t AppendInt8(int8 value);``status_t AppendUint8(uint8 value);``status_t AppendInt16(int16 value);``status_t AppendUint16(uint16 value);``status_t AppendInt32(int32 value);``status_t AppendUint32(uint32 value);``status_t AppendInt64(int64 value);``status_t Appenduint64(uint64 value);``status_t AppendFloat(float value);``status_t AppendDouble(double value);``status_t AppendString(const char* string);``status_t AppendData(const void* data,  
size_t size);``status_t AppendMessage([`BMessage`](BMessage.md "BMessage")& message);`

Appends the specified data type to the end of the buffer. Integer values are automatically converted to network byte ordering (but floats and doubles are not, nor are values inside structures added using `AppendData()`).

Strings are appended as null-terminated strings.

`AppendData()` copies `size` bytes from the buffer pointed at by `data`.

Return Code| Description  
---|---  
`B_OK`| The data was appended without error.  
`B_ERROR`| The data couldn't be appended.  
  
####  Data(), Size(), BytesRemaining()

`unsigned char* Data() const;``size_t Size() const;``size_t BytesRemaining() const;`

`Data()` returns a pointer to the `BNetBuffer`'s internal data buffer.

`Size()` returns the number of bytes currently in the buffer.

`BytesRemaining()` returns the number of unused bytes in the buffer.

####  RemoveInt8(), RemoveUint8(), RemoveInt16(), RemoveUint16(), RemoveInt32(), RemoveUint32(), RemoveInt64(), RemoveUint64(), RemoveFloat(), RemoveDouble(), RemoveString(), RemoveData(), RemoveMessage()

`status_t RemoveInt8(int8& value);``status_t RemoveUint8(uint8& value);``status_t RemoveInt16(int16& value);``status_t RemoveUint16(uint16& value);``status_t RemoveInt32(int32& value);``status_t RemoveUint32(uint32& value);``status_t RemoveInt64(int64& value);``status_t Removeuint64(uint64& value);``status_t RemoveFloat(float& value);``status_t RemoveDouble(double& value);``status_t RemoveString(const char* string,  
size_t size);``status_t RemoveData(const void* data,  
size_t size);``status_t RemoveMessage([`BMessage`](BMessage.md "BMessage")& message);`

Removes the specified data type from the buffer. Integer values are automatically converted from network byte ordering (but floats and doubles are not, nor are values inside structures removed using `RemoveData()`).

Strings are removed as null-terminated strings, up to `size` bytes. Be sure the string buffer is at least `size` bytes long.

`RemoveData()` removes `size` bytes and copies them into the buffer pointed at by `data`. Be sure the `data` buffer is at least `size` bytes long.

These functions start at the beginning of the buffer. After each item is removed, the next `Remove…()` call will start at the next byte in the buffer.

Return Code| Description  
---|---  
`B_OK`| The data was removed without error.  
`B_ERROR`| The data couldn't be removed.  
  
* * *

### Operators

#### = (assignment)

`BNetBuffer& operator =(const BNetBuffer& from);`

Copies the `BNetBuffer` specified by `from` into the left-side object, thereby duplicating that object. If `from` is connected, the left-side object will duplicate and open the same connection. Even the data in the buffer is copied, if there is any.

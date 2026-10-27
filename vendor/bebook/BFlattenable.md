<!-- Classes And Methods - The Support Kit - BFlattenable -->

## BFlattenable

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `support/Flattenable.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BFlattenable_Overview.md "BFlattenable")

  * [Member Functions](BFlattenable.md#BFlattenable_MemberFunctions)

  
  
* * *

### Member Functions

####  Flatten(), Unflatten()

`virtual status_t Flatten(void* buffer,  
ssize_t numBytes) const = 0;``virtual status_t Unflatten(type_code code,  
const void* buffer,  
ssize_t numBytes) = 0;`

`Flatten()` is implemented by derived classes to write the object into the buffer. There are `numBytes` bytes of memory available at the buffer address. If this isn't at least as much memory as the [`FlattenedSize()`](BFlattenable.md#BFlattenable_FlattenedSize "FlattenedSize\(\)") function says is necessary, `Flatten()` should return an error. If successful, it should return `B_OK`.

`Unflatten()` is implemented by derived classes to set object values from `numBytes` bytes of data taken from the buffer. However, it should read the data only if the type `code` it's passed indicates that the data is a type that it supports—that is, only if its [`AllowsTypeCode()`](BFlattenable.md#BFlattenable_AllowsTypeCode) function returns `true` for the code. If successful in reconstructing the object from the flattened data, `Unflatten()` should return `B_OK`. If not, it should return `B_ERROR` or a more descriptive error code.

#### FlattenedSize()

`virtual ssize_t FlattenedSize() const = 0;`

Implemented by derived classes to return the amount of memory needed to hold the flattened object. This is the minimal amount that must be allocated and passed to [`Flatten()`](BFlattenable.md#BFlattenable_Flatten "Flatten\(\), Unflatten\(\)").

#### IsFixedSize()

`virtual bool IsFixedSize() const = 0;`

Implemented by derived classes to return `true` if all instances of the class take up the same amount of memory when they're flattened, and `false` if their flattened sizes can differ. The sizes will differ, for example, if a variable-length string is part of the flattened data.

####  TypeCode() , AllowsTypeCode()

`virtual type_code TypeCode() const = 0;``virtual bool AllowsTypeCode(type_code code) const;`

`TypeCode()` is implemented by derived classes to return the type code that identifies the class type. The code is used to identify an instance of the class in its flattened state, for example when it's added to a [`BMessage`](BMessage.md "BMessage").

`AllowsType()` returns `true` if the code it's passed matches the code returned by `TypeCode()` and `false` if not. If can be modified in derived classes to apply a more liberal standard—to allow more than one type code to identify the object.

See also: [`BMessage::AddData()`](BMessage.md#BMessage_AddData "AddData\(\), AddBool\(\), AddInt8\(\), AddInt16\(\), AddInt32\(\), AddInt64\(\), AddFloat\(\), AddDouble\(\), AddString\(\), AddPoint\(\), AddRect\(\), AddRef\(\), AddMessage\(\), AddMessenger\(\), AddPointer\(\), AddFlat\(\)")

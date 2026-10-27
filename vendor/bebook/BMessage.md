<!-- Classes And Methods - The Application Kit - BMessage -->

## BMessage

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `app/Message.h`  
Library:| `libbe.so`  
Allocation| new, static, or automatic  
[Class Overview](BMessage_Overview.md "BMessage")

  * [Data Members](BMessage.md#BMessage_DataMembers)
  * [Constructor and Destructor](BMessage.md#BMessage_ConstructorDestructor)
  * [Member Functions](BMessage.md#BMessage_MemberFunctions)
  * [Operators](BMessage.md#BMessage_Operators)
  * [Constants](BMessage.md#BMessage_Constants)

  
  
* * *

### Data Members

#### what

`public uint32 what ;`

A coded constant that captures what the message is about.

* * *

### Constructor and Destructor

#### BMessage()

`BMessage(uint32 command);` `BMessage(const BMessage& message);` `BMessage();`

Creates a new `BMessage` object that has the given `command` constant, or that's a copy of another `BMessage`. If it's a copy, the new object contains the same command constant and data fields as `message`.

See also: [`BLooper::DetachCurrentMessage()`](BLooper.md#BLooper_DetachCurrentMessage)

#### ~BMessage()

`virtual ~BMessage();`

Frees all memory allocated to hold message data. If the message sender is expecting a reply but hasn't received one, a default reply (with `B_NO_REPLY` as the `what` data member) is sent before the message is destroyed.

The system retains ownership of the messages it delivers to you. Each message loop routinely deletes delivered `BMessage`s after the application is finished responding to them.

* * *

### Member Functions

####  AddData(), AddBool(), AddInt8(), AddInt16(), AddInt32(), AddInt64(), AddFloat(), AddDouble(), AddString(), AddPoint(), AddRect(), AddRef(), AddMessage(), AddMessenger(), AddPointer(), AddFlat()

`status_t AddData(const char* name,  
type_code type,  
const void* data,  
ssize_t numBytes,  
bool fixedSize = true,  
int32 numItems = 1);``status_t AddBool(const char* name,  
bool aBool);``status_t AddInt8(const char* name,  
int8 anInt8);``status_t AddInt16(const char* name,  
int16 anInt16);``status_t AddInt32(const char* name,  
int32 anInt32);``status_t AddInt64(const char* name,  
int64 anInt64);``status_t AddFloat(const char* name,  
float aFloat);``status_t AddDouble(const char* name,  
double aDouble);`

`status_t AddString(const char* name,  
const char* string);` `status_t AddString(const char* name,  
const BString string);`

`status_t AddPoint(const char* name,  
BPoint point);``status_t AddRect(const char* name,  
BRect rect);``status_t AddRef(const char* name,  
const entry_ref* ref);``status_t AddMessage(const char* name,  
const BMessage* message);``status_t AddMessenger(const char* name,  
BMessenger messenger);``status_t AddPointer(const char* name,  
const void* pointer);``status_t AddFlat(const char* name,  
BFlattenable* object,  
int32 numItems = 1);`

These functions add data to the field named `name` and assign a data type to the field. Field names can be no longer than 255 characters. If more than one item of data is added under the same name, the `BMessage` creates an array of data for that name. Each time you add another value (to the same name), the value is added to the end of the array—you can't add a value at a specific index. A given field can only store one type of data.

`AddData()` copies `numBytes` of `data` into the field, and assigns the data a `type` code. It copies whatever the `data` pointer points to. For example, if you want to add a string of characters to the message, `data` should be the string pointer (char *). If you want to add only the string pointer, not the characters themselves, `data` should be a pointer to the pointer (char **). The assigned `type` must be a specific data type; it should not be `B_ANY_TYPE`.

When you call `AddData()` to place the first item in an array under a new name, you can provide it with two arguments, `fixedSize` and `numItems`, that will improve the object's efficiency. If the `fixedSize` flag is `true`, each item in the array must have the same number of bytes; if the flag is `false`, items can vary in size. `numItems` tells the object to pre-allocate storage for some number of items. This isn't a limit, you can add more than `numItems` to the field.

Most of the other functions are variants of `AddData()` that hard-code the field's type. For example, `AddFloat()` assigns the type `B_FLOAT_TYPE`; `AddBool()` assigns `B_BOOL_TYPE`, and so on.

`AddString()`, like `AddData()`, takes a pointer to the data it adds, or you can use a [`BString`](BString.md "BString") object. The `string` must be null-terminated; the null character is counted and copied into the message. Similarly, `AddRef()` adds the pointed to entry_ref structure to the message (and the variable-length name that's one of the elements of the structure); `AddMessage()` adds one `BMessage` to another.

The other functions are simply passed the data directly. For example, `AddInt32()` takes an int32 or uint32 and `AddMessenger()` takes a [`BMessenger`](BMessenger.md "BMessenger") object, whereas `AddData()` would be passed a pointer to an int32 and a pointer to a [`BMessenger`](BMessenger.md "BMessenger"). `AddPointer()` adds only the pointer it's passed, not the data it points to. To accomplish the same thing, `AddData()` would take a pointer to the pointer. (The pointer will be valid only locally; it won't be useful to a remote destination.)

`AddFlat()` flattens an `object` (by calling its `Flatten()` function) and adds the flat data to the message. It calls the object's [`TypeCode()`](BFlattenable.md#BFlattenable_TypeCode "TypeCode\(\) , AllowsTypeCode\(\)") function to learn the type code it should associate with the data. Objects that are added through `AddFlat()` must inherit from [`BFlattenable`](BFlattenable.md "BFlattenable") (defined in the [Support Kit](TheSupportKit.md "The Support Kit")).

You can also provide a `numItems` hint to `AddFlat()` when you call it to set up a new array. `AddFlat()` calls the object's [`IsFixedSize()`](BFlattenable.md#BFlattenable_IsFixedSize "IsFixedSize\(\)") function to discover whether all items in the array will be the same size.

These functions return `B_ERROR` if the data is too massive to be added to the message, `B_BAD_TYPE` if the data can't be added to an existing array because it's the wrong type, `B_NO_MEMORY` if the `BMessage` can't get enough memory to hold the data, and `B_BAD_VALUE` if the proposed `name` for the data is longer than 255 bytes. If all goes well, they return `B_OK`.

There's no limit on the number of named fields a message can contain or on the size of a field's data. However, since the search is linear, combing through a very long list of names to find a particular piece of data may be inefficient. Also, because of the amount of data that must be moved, an extremely large message can slow the delivery mechanism. It's sometimes better to put some of the information in a common location (a file, a private clipboard, a shared area of memory) and just refer to it in the message.

See also: [`FindData()`](BMessage.md#BMessage_FindData "FindData\(\), FindBool\(\), FintInt8\(\), FindInt16\(\), FindInt32\(\), FindInt64\(\), FindFloat\(\), FindDouble\(\), FindString\(\), FindPoint\(\), FindRect\(\), FindRef\(\), FindMessage\(\), FindMessenger\(\), FindPointer\(\), FindFlat\(\)"), [`GetInfo()`](BMessage.md#BMessage_GetInfo "GetInfo\(\)")

#### AddSpecifier()

`status_t AddSpecifier(const BMessage* message);` `status_t AddSpecifier(const char* property);` `status_t AddSpecifier(const char* property,  
int32 index);` `status_t AddSpecifier(const char* property,  
int32 index,  
int32 range);` `status_t AddSpecifier(const char* property,  
const char* name);`

Adds a specifier to the specifier stack. There are several variations of this method. The first adds the specifier `message` to the specifier stack. The other methods add a specifier targeting the property `property`, with specifier constants `B_DIRECT_SPECIFIER`, `B_INDEX_SPECIFIER`, `B_RANGE_SPECIFIER`, and `B_NAME_SPECIFIER`, respectively. For all other specifiers, you must construct the specifier separately and then call `AddSpecifier()` on the message. For more information about specifiers, see the "Scripting" section near the beginning of this chapter.

Specifiers are stored in a data array named "specifiers". However, since `AddSpecifier()` also sets the notion of the current specifier, specifiers should always be added to a scripting message with this method rather than with [`AddMessage()`](BMessage.md#BMessage_AddData "AddData\(\), AddBool\(\), AddInt8\(\), AddInt16\(\), AddInt32\(\), AddInt64\(\), AddFloat\(\), AddDouble\(\), AddString\(\), AddPoint\(\), AddRect\(\), AddRef\(\), AddMessage\(\), AddMessenger\(\), AddPointer\(\), AddFlat\(\)").

`AddSpecifier()` returns `B_OK` if it's able to add the specifier to the `BMessage` and an error code, generally only `B_NO_MEMORY` to indicate that it has run out of memory, if not.

See also: [`GetCurrentSpecifier()`](BMessage.md#BMessage_GetCurrentSpecifier "GetCurrentSpecifier\(\), PopSpecifier\(\)"), [`HasSpecifiers()`](BMessage.md#BMessage_HasSpecifiers "HasSpecifiers\(\)"), [`PopSpecifier()`](BMessage.md#BMessage_PopSpecifier)

#### CountNames()

`int32 CountNames(type_code type) const;`

Returns the number of named data fields in the `BMessage` that store data of the specified `type`. An array of information held under a single name counts as one field; each name is counted only once, no matter how many data items are stored under that name.

If `type` is `B_ANY_TYPE`, this function counts all named fields. If `type` is a specific type, it counts only fields that store data registered as that type.

See also: [`GetInfo()`](BMessage.md#BMessage_GetInfo "GetInfo\(\)")

####  FindData(), FindBool(), FintInt8(), FindInt16(), FindInt32(), FindInt64(), FindFloat(), FindDouble(), FindString(), FindPoint(), FindRect(), FindRef(), FindMessage(), FindMessenger(), FindPointer(), FindFlat()

`status_t FindData(const char* name,  
type_code type,  
int32 index,  
const void** data,  
ssize_t* numBytes) const;` `status_t FindData(const char* name,  
type_code type,  
const void** data,  
ssize_t* numBytes) const;`

`status_t FindBool(const char* name,  
int32 index,  
bool* aBool) const;` `status_t FindBool(const char* name,  
bool* aBool) const;`

`status_t FindInt8(const char* name,  
int32 index,  
int8* anInt8) const;` `status_t FindInt8(const char* name,  
int8* anInt8) const;`

`status_t FindInt16(const char* name,  
int32 index,  
int16* anInt16) const;` `status_t FindInt16(const char* name,  
int16* anInt16) const;`

`status_t FindInt32(const char* name,  
int32 index,  
int32* anInt32) const;` `status_t FindInt32(const char* name,  
int32* anInt32) const;`

`status_t FindInt64(const char* name,  
int32 index,  
int64* anInt64) const;` `status_t FindInt64(const char* name,  
int64* anInt64) const;`

`status_t FindFloat(const char* name,  
int32 index,  
float* aFloat) const;` `status_t FindFloat(const char* name,  
float* aFloat) const;`

`status_t FindDouble(const char* name,  
int32 index,  
double* aDouble) const;` `status_t FindDouble(const char* name,  
double* aDouble) const;`

`status_t FindString(const char* name,  
int32 index,  
const char** string) const;` `status_t FindString(const char* name,  
const char** string) const;` `status_t FindString(const char* name,  
int32 index,  
BString* string) const;` `status_t FindString(const char* name,  
BString* string) const;`

`status_t FindPoint(const char* name,  
int32 index,  
BPoint* point) const;` `status_t FindPoint(const char* name,  
BPoint* point) const;`

`status_t FindRect(const char* name,  
int32 index,  
BRect* rect) const;` `status_t FindRect(const char* name,  
BRect* rect) const;`

`status_t FindRef(const char* name,  
int32 index,  
entry_ref* ref) const;` `status_t FindRef(const char* name,  
entry_ref* ref) const;`

`status_t FindMessage(const char* name,  
int32 index,  
BMessage* message) const;` `status_t FindMessage(const char* name,  
BMessage* message) const;`

`status_t FindMessenger(const char* name,  
int32 index,  
BMessenger* messenger) const;` `status_t FindMessenger(const char* name,  
BMessenger* messenger) const;`

`status_t FindPointer(const char* name,  
int32 index,  
void** pointer) const;` `status_t FindPointer(const char* name,  
void** pointer) const;`

`status_t FindFlat(const char* name,  
int32 index,  
BFlattenable* object) const;` `status_t FindFlat(const char* name,  
BFlattenable* object) const;`

These functions retrieve data from the `BMessage`. Each looks for data stored under the specified `name`. If more than one data item has the same name, an `index` can be provided to tell the function which item in the `name` array it should find. Indices begin at 0. If an `index` isn't provided, the function will find the first, or only, item in the array.

Important

![Important](./images/admonitions/Warning_32.png)

In all cases except `FindData()` and `FindString()`, the data that's retrieved from the `BMessage` is copied into the reference argument; the caller is responsible for freeing the copied data. For `FindData()` and the non-[`BString`](BString.md "BString") version of `FindString()`, a pointer to the data is returned; the `BMessage` retains ownership of the actual data and will delete the data when the object itself is deleted.

`FindData()` places, in `*data`, a pointer to the requested data item. The size of the item in bytes is written to `numBytes`. If `type` is `B_ANY_TYPE`, it provides a pointer to the data no matter what type it actually is. But if `type` is a specific data type, it provides the pointer only if the `name` field holds data of that particular type.

The other functions are specialized versions of `FindData()`. They match the corresponding [Add…()](BMessage.md#BMessage_AddData "AddData\(\), AddBool\(\), AddInt8\(\), AddInt16\(\), AddInt32\(\), AddInt64\(\), AddFloat\(\), AddDouble\(\), AddString\(\), AddPoint\(\), AddRect\(\), AddRef\(\), AddMessage\(\), AddMessenger\(\), AddPointer\(\), AddFlat\(\)") methods and search for named data of a particular type, as described below:

Function| Finds data| Registered as type  
---|---|---  
`FindBool()`| a bool| `B_BOOL_TYPE`  
`FindInt8()`| an int8 or uint8| `B_INT8_TYPE`  
`FindInt16()`| an int16 or uint16| `B_INT16_TYPE`  
`FindInt32()`| an int32 or uint32| `B_INT32_TYPE`  
`FindInt64()`| an int64 or uint64| `B_INT64_TYPE`  
`FindFloat()`| a float| `B_FLOAT_TYPE`  
`FindDouble()`| a double| `B_DOUBLE_TYPE`  
`FindString()`| a character string| `B_STRING_TYPE`  
`FindPoint()`| a [`BPoint`](BPoint.md "BPoint") object| `B_POINT_TYPE`  
`FindRect()`| a [`BRect`](BRect.md "BRect") object| `B_RECT_TYPE`  
`FindRef()`| an entry_ref| `B_REF_TYPE`  
`FindMessage()`| a `BMessage` object| `B_MESSAGE_TYPE`  
`FindMessenger()`| a [`BMessenger`](BMessenger.md "BMessenger") object| `B_MESSENGER_TYPE`  
`FindPointer()`| a pointer to anything| `B_POINTER_TYPE`  
  
The other type-specific functions retrieve the requested data item from the message by copying it to the variable referred to by the last argument; you get the data, not just a pointer to it. For example, `FindMessenger()` assigns the [`BMessenger`](BMessenger.md "BMessenger") it finds in the message to the `messenger` object, whereas `FindData()` would provide only a pointer to a [`BMessenger`](BMessenger.md "BMessenger"). `FindPointer()` puts the found pointer in the void* variable that `pointer` refers to; `FindData()`, as illustrated above, would provide a pointer to the pointer. (If the message was delivered from a remote source, pointers retrieved from the message won't be valid.)

`FindRef()` retrieves an entry_ref structure; the data that's used to reconstitute the structure may have been added as an entry_ref (through `AddRef()`), or as a flattened [`BPath`](BPath.md "BPath") object (`AddFlat()`).

`FindFlat()` assigns the object stored in the `BMessage` to the `object` passed as an argument, it calls the `object`'s [`Unflatten()`](BMessage.md#BMessage_Unflatten) function and passes it the flat data from the message, provided that the two objects have compatible types. The argument object's `AllowsTypeCode()` function must return `true` when tested with the type code stored in the message; if not, `FindFlat()` fails and returns `B_BAD_VALUE`.

If these functions can't find any data associated with `name`, they return a `B_NAME_NOT_FOUND` error. If they can't find `name` data of the requested `type` (or the type the function returns), they return `B_BAD_TYPE`. If the `index` is out of range, they return `B_BAD_INDEX`. You can rely on the values they retrieve only if they return `B_OK` and the data was correctly recorded when it was added to the message.

When they fail, `FindData()` and `FindString()` provide `NULL` pointers. `FindRect()` hands you an invalid rectangle and `FindMessenger()` an invalid [`BMessenger`](BMessenger.md "BMessenger"). Most of the other functions set the data values to 0, which may be indistinguishable from valid values.

Finding a data item doesn't remove it from the `BMessage`.

(Several functions, such as `FindRect()` and `FindInt32()`, have versions that return the found value directly. These versions don't report errors and may not be supported in the future.)

See also: [`GetInfo()`](BMessage.md#BMessage_GetInfo "GetInfo\(\)"), [`AddData()`](BMessage.md#BMessage_AddData "AddData\(\), AddBool\(\), AddInt8\(\), AddInt16\(\), AddInt32\(\), AddInt64\(\), AddFloat\(\), AddDouble\(\), AddString\(\), AddPoint\(\), AddRect\(\), AddRef\(\), AddMessage\(\), AddMessenger\(\), AddPointer\(\), AddFlat\(\)")

####  Flatten(), Unflatten(), FlattenedSize()

`status_t Flatten(BDataIO* object,  
ssize_t* numBytes = NULL) const;` `status_t Flatten(char* address,  
ssize_t numBytes = NULL) const;`

`status_t Unflatten(BDataIO* object);` `status_t Unflatten(const char* address);`

`ssize_t FlattenedSize() const;`

These functions write the `BMessage` and the data it contains to a "flat" (untyped) buffer of bytes, and reconstruct a `BMessage` object from such a buffer.

If passed a [`BDataIO`](BDataIO.md "BDataIO") `object` (including a [`BFile`](BFile.md "BFile")), `Flatten()` calls the object's `Write()` function to write the message data. If passed the `address` of a buffer, it begins writing at the start of the buffer. `FlattenedSize()` returns the number of bytes you must provide in the buffer to hold the flattened object. `Flatten()` places the number of bytes actually written in the variable that its `numBytes` argument refers to.

`Unflatten()` empties the `BMessage` of any information it may happen to contain, then initializes the object from data read from the buffer. If passed a [`BDataIO`](BDataIO.md "BDataIO") `object`, it calls the object's `Read()` function to read the message data. If passed a buffer `address`, it begins reading at the start of the buffer. It's up to the caller to make sure that `Unflatten()` reads data that `Flatten()` wrote and that pointers are positioned correctly.

`Flatten()` returns any errors encountered when writing the data, or `B_OK` if there is no error.

If it doesn't recognize the data in the buffer as being a flattened object or there's a failure in reading the data, `Unflatten()` returns `B_BAD_VALUE`. If it doesn't have adequate memory to recreate the whole message, it returns `B_NO_MEMORY`. Otherwise, it returns `B_OK`.

See also: the [`BDataIO`](BDataIO.md "BDataIO") class in the [Support Kit](TheSupportKit.md "The Support Kit")

####  GetCurrentSpecifier(), PopSpecifier()

`status_t GetCurrentSpecifier(int32* index,  
BMessage* specifier = NULL,  
int32* what = NULL,  
const char** property = NULL) const;``status_t PopSpecifier();`

`GetCurrentSpecifier()` unpacks the current specifier in the `BMessage`, the one at the top of the specifier stack; `PopSpecifier()` changes the notion of which specifier is current, by popping the current one from the stack.

These functions aid in implementing a class-specific version of [`BHandler`](BHandler.md "BHandler")'s [`ResolveSpecifier()`](BHandler.md#BHandler_ResolveSpecifier "ResolveSpecifier\(\)") function—the first gets the specifier that needs to be resolved, and the second pops it from the stack after it is resolved. You can also call them to examine relevant specifiers when handling a message that targets an object property (such as `B_GET_PROPERTY`).

A scripting `BMessage` keeps specifiers in a data array named "specifiers"; each specifier is itself a `BMessage`, but one with a special structure and purpose in the scripting system. See the "Scripting" section near the beginning of this chapter for an overview of the system and the place of specifiers in it.

The specifiers in a message are ordered and, until `PopSpecifier()` is called, the one that was added last, the one with the greatest index, is the current specifier. `PopSpecifier()` merely decrements the index that picks the current specifier; it doesn't delete anything from the `BMessage`.

`GetCurrentSpecifier()` puts the index of the current specifier in the variable that its first argument, `index`, refers to. If other arguments are provided, it makes the `specifier` `BMessage` a copy of the current specifier. It also extracts two pieces of information from the `specifier`: It places the `what` data member of the specifier in the `what` variable and a pointer to the property name in the `property` variable. These last two output arguments won't be valid if the `specifier` argument is `NULL`.

Both functions fail if the `BMessage` doesn't contain specifiers. In addition, `GetCurrentSpecifier()` fails if it can't find data in the `BMessage` for its `specifier` and `property` arguments, and `PopSpecifier()` fails if the `BMessage` isn't one that has been delivered to you after being processed through a message loop. When it fails, `GetCurrentSpecifier()` returns `B_BAD_SCRIPT_SYNTAX`, but `PopSpecifier()` returns `B_BAD_VALUE`. On success, both functions return `B_OK`.

See also: [`AddSpecifier()`](BMessage.md#BMessage_AddSpecifier "AddSpecifier\(\)"), [`HasSpecifiers()`](BMessage.md#BMessage_HasSpecifiers "HasSpecifiers\(\)"), [`BHandler::ResolveSpecifier()`](BHandler.md#BHandler_ResolveSpecifier "ResolveSpecifier\(\)")

#### GetInfo()

`status_t GetInfo(const char* name,  
type_code* typeFound,  
int32* countFound = NULL) const;` `status_t GetInfo(const char* name,  
type_code* typeFound,  
bool* fixedSize) const;` `status_t GetInfo(type_code type,  
int32 index,  
char** nameFound,  
type_code* typeFound,  
int32* countFound = NULL) const;`

Provides information about the data fields stored in the `BMessage`.

When passed a `name` that matches a name within the `BMessage`, `GetInfo()` places the type code for data stored under that name in the variable referred to by `typeFound` and writes the number of data items with that name into the variable referred to by `countFound`. It then returns `B_OK`. If it can't find a `name` field within the `BMessage`, it sets the `countFound` variable to 0, and returns `B_NAME_NOT_FOUND` (without modifying the `typeFound` variable).

When the `fixedSize` argument is specified, the bool referenced by `fixedSize` is set to `true` if all items in the array specified by `name` must be the same size, and `false` if the items can be of different sizes (see [`AddData()`](BMessage.md#BMessage_AddData "AddData\(\), AddBool\(\), AddInt8\(\), AddInt16\(\), AddInt32\(\), AddInt64\(\), AddFloat\(\), AddDouble\(\), AddString\(\), AddPoint\(\), AddRect\(\), AddRef\(\), AddMessage\(\), AddMessenger\(\), AddPointer\(\), AddFlat\(\)")).

When passed a `type` and an `index`, `GetInfo()` looks only at fields that store data of the requested `type` and provides information about the field at the requested `index`. Indices begin at 0 and are type specific. For example, if the requested `type` is `B_DOUBLE_TYPE` and the `BMessage` contains a total of three named fields that store double data, the first field would be at index 0, the second at 1, and the third at 2, no matter what other types of data actually separate them in the `BMessage`, and no matter how many data items each field contains. (Note that the `index` in this case ranges over fields, each with a different name, not over the data items within a particular named field.) If the requested type is `B_ANY_TYPE`, this function looks at all fields and gets information about the one at `index` whatever its type.

If successful in finding data of the `type` requested at `index`, `GetInfo()` returns `B_OK` and provides information about the data through the last three arguments:

  * It places a pointer to the name of the data field in the variable referred to by `nameFound`.

  * It puts the code for the type of data the field contains in the variable referred to by `typeFound`. This will be the same as the `type` requested, unless the requested type is `B_ANY_TYPE`, in which case `typeFound` will be the actual type stored under the name.

  * It records the number of data items stored within the field in the variable referred to by `countFound`.

If `GetInfo()` can't find data of the requested `type` at `index`, it sets the `countFound` variable to 0, and returns `B_BAD_TYPE`. If the index is out of range, it returns `B_BAD_INDEX`.

This version of `GetInfo()` can be used to iterate through all the `BMessage`'s data. For example:
    
    
    char *name;
    uint32 type;
    int32 count;
    
    for ( int32 i = 0;
          msg->GetInfo(B_ANY_TYPE, i, &name, &type, &count) == B_OK;
          i++ ) {
       . . .
    }

If the index is incremented from 0 in this way, all data of the requested type will have been read when `GetInfo()` returns `B_NAME_NOT_FOUND`. If the requested type is `B_ANY_TYPE`, as shown above, it will reveal the name and type of every field in the `BMessage`.

See also: [`AddData()`](BMessage.md#BMessage_AddData "AddData\(\), AddBool\(\), AddInt8\(\), AddInt16\(\), AddInt32\(\), AddInt64\(\), AddFloat\(\), AddDouble\(\), AddString\(\), AddPoint\(\), AddRect\(\), AddRef\(\), AddMessage\(\), AddMessenger\(\), AddPointer\(\), AddFlat\(\)"), [`FindData()`](BMessage.md#BMessage_FindData "FindData\(\), FindBool\(\), FintInt8\(\), FindInt16\(\), FindInt32\(\), FindInt64\(\), FindFloat\(\), FindDouble\(\), FindString\(\), FindPoint\(\), FindRect\(\), FindRef\(\), FindMessage\(\), FindMessenger\(\), FindPointer\(\), FindFlat\(\)")

#### HasSpecifiers()

`bool HasSpecifiers() const;`

Returns `true` if the `BMessage` has specifiers added by an [`AddSpecifier()`](BMessage.md#BMessage_AddSpecifier "AddSpecifier\(\)") function, and `false` if not.

See also: [`AddSpecifier()`](BMessage.md#BMessage_AddSpecifier "AddSpecifier\(\)"), [`GetCurrentSpecifier()`](BMessage.md#BMessage_GetCurrentSpecifier "GetCurrentSpecifier\(\), PopSpecifier\(\)")

#### IsSystem()

`bool IsSystem() const;`

Returns `true` if the `what` data member of the `BMessage` object identifies it as a system-defined message, and `false` if not.

####  MakeEmpty(), IsEmpty()

`status_t MakeEmpty();``bool IsEmpty() const;`

`MakeEmpty()` removes and frees all data that has been added to the `BMessage`, without altering the `what` constant. It returns `B_OK`, unless the message can't be altered (as it can't if it's being dragged), in which case it returns `B_ERROR`.

`IsEmpty` returns `true` if the `BMessage` has no data (whether or not it was emptied by `MakeEmpty()`), and `false` if it has some.

See also: [`RemoveName()`](BMessage.md#BMessage_RemoveName "RemoveName\(\), RemoveData\(\)")

#### PrintToStream()

`void PrintToStream() const;`

Prints information about the `BMessage` to the standard output stream (stdout). Each field of named data is reported in the following format,
    
    
    #entry _name_ , type = _type_ , count = _count_
    

where _`name`_ is the name that the data is registered under, _`type`_ is the constant that indicates what type of data it is, and _`count`_ is the number of data items in the named array.

####  RemoveName(), RemoveData()

`status_t RemoveName(const char* name);``status_t RemoveData(const char* name,  
int32 index = 0);`

`RemoveName()` removes all data entered in the `BMessage` under `name` and the name itself. `RemoveData()` removes the single item of data at `index` in the `name` array. If the array has just one data item, it removes the array and `name` just as `RemoveName()` would.

Both functions free the memory that was allocated to hold the data, and return `B_OK` when successful. However, if there's no data in the `BMessage` under `name`, they return a `B_NAME_NOT_FOUND` error. If message data can be read but can't be changed (as it can't for a message that's being dragged), they both return `B_ERROR`. If the `index` is out of range, `RemoveData()` returns `B_BAD_INDEX` (the index is too high) or `B_BAD_VALUE` (the value passed is a negative number).

See also: [`MakeEmpty()`](BMessage.md#BMessage_MakeEmpty "MakeEmpty\(\), IsEmpty\(\)")

####  ReplaceData(), ReplaceBool(), ReplaceInt8(), ReplaceInt16(), ReplaceInt32(), ReplaceInt64(), ReplaceFloat(), ReplaceDouble(), ReplaceString(), ReplacePoint(), ReplaceRect(), ReplaceRef(), ReplaceMessage(), ReplaceMessenger(), ReplacePointer(), ReplaceFlat()

`status_t ReplaceData(const char* name,  
type_code type,  
const void* data,  
ssize_t numBytes);` `status_t ReplaceData(const char* name,  
type_code type,  
int32 index,  
const void* data,  
ssize_t numBytes);`

`status_t ReplaceBool(const char* name,  
bool aBool);` `status_t ReplaceBool(const char* name,  
int32 index,  
bool aBool);`

`status_t ReplaceInt8(const char* name,  
int8 anInt8);` `status_t ReplaceInt8(const char* name,  
int32 index,  
int8 anInt8);`

`status_t ReplaceInt16(const char* name,  
int16 anInt16);` `status_t ReplaceInt16(const char* name,  
int32 index,  
int16 anInt16);`

`status_t ReplaceInt32(const char* name,  
int32 anInt32);` `status_t ReplaceInt32(const char* name,  
int32 index,  
int32 anInt32);`

`status_t ReplaceInt64(const char* name,  
int64 anInt64);` `status_t ReplaceInt64(const char* name,  
int32 index,  
int64 anInt64);`

`status_t ReplaceFloat(const char* name,  
float aFloat);` `status_t ReplaceFloat(const char* name,  
int32 index,  
float aFloat);`

`status_t ReplaceDouble(const char* name,  
double aDouble);` `status_t ReplaceDouble(const char* name,  
int32 index,  
double aDouble);`

`status_t ReplaceString(const char* name,  
const char* aString);` `status_t ReplaceString(const char* name,  
int32 index,  
const char* aString);` `status_t ReplaceString(const char* name,  
BString& aString);` `status_t ReplaceString(const char* name,  
int32 index,  
BString& aString);`

`status_t ReplacePoint(const char* name,  
BPoint point);` `status_t ReplacePoint(const char* name,  
int32 index,  
BPoint point);`

`status_t ReplaceRect(const char* name,  
BRect rect);` `status_t ReplaceRect(const char* name,  
int32 index,  
BRect rect);`

`status_t ReplaceRef(const char* name,  
entry_ref* ref);` `status_t ReplaceRef(const char* name,  
int32 index,  
entry_ref* ref);`

`status_t ReplaceMessage(const char* name,  
BMessage* message);` `status_t ReplaceMessage(const char* name,  
int32 index,  
BMessage* message);`

`status_t ReplaceMessenger(const char* name,  
BMessenger messenger);` `status_t ReplaceMessenger(const char* name,  
int32 index,  
BMessenger messenger);`

`status_t ReplacePointer(const char* name,  
const void* pointer);` `status_t ReplacePointer(const char* name,  
int32 index,  
const void* pointer);`

`status_t ReplaceFlat(const char* name,  
BFlattenable* object);` `status_t ReplaceFlat(const char* name,  
int32 index,  
BFlattenable* object);`

These functions replace a data item in the `name` field with another item passed as an argument. If an `index` is provided, they replace the item in the `name` array at that index; if an `index` isn't mentioned, they replace the first (or only) item stored under `name`. If an `index` is provided but it's out of range, the replacement fails.

`ReplaceData()` replaces an item in the `name` field with `numBytes` of data, but only if the `type` code that's specified for the data matches the type of data that's already stored in the field. The `type` must be specific; it can't be `B_ANY_TYPE`.

`FindFlat()` replaces a flattened object with another `object`, provided that the type reported by the argument `object` (by its [`TypeCode()`](BFlattenable.md#BFlattenable_TypeCode "TypeCode\(\) , AllowsTypeCode\(\)") function) matches the type recorded for the item in the message. If not, it returns `B_BAD_VALUE`.

The other functions are simplified versions of `ReplaceData()`. They each handle the specific type of data declared for their last arguments. They succeed if this `type` matches the type of data already in the `name` field, and fail if it does not. The new data is added precisely as the counterpart [Add…()](BMessage.md#BMessage_AddData "AddData\(\), AddBool\(\), AddInt8\(\), AddInt16\(\), AddInt32\(\), AddInt64\(\), AddFloat\(\), AddDouble\(\), AddString\(\), AddPoint\(\), AddRect\(\), AddRef\(\), AddMessage\(\), AddMessenger\(\), AddPointer\(\), AddFlat\(\)") function would add it.

If successful, all these functions return `B_OK`. If unsuccessful, they return an error code`B_ERROR` if the message is read-only (as it is while the message is being dragged), `B_BAD_INDEX` if the `index` is out of range, `B_NAME_NOT_FOUND` if the `name` field doesn't exist, or `B_BAD_TYPE` if the field doesn't contain data of the specified type.

See also: [`AddData()`](BMessage.md#BMessage_AddData "AddData\(\), AddBool\(\), AddInt8\(\), AddInt16\(\), AddInt32\(\), AddInt64\(\), AddFloat\(\), AddDouble\(\), AddString\(\), AddPoint\(\), AddRect\(\), AddRef\(\), AddMessage\(\), AddMessenger\(\), AddPointer\(\), AddFlat\(\)")

#### ReturnAddress()

`BMessenger ReturnAddress();`

Returns a [`BMessenger`](BMessenger.md "BMessenger") object that can be used to reply to the `BMessage`. Calling the [`BMessenger`](BMessenger.md "BMessenger")'s [`SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)") function is equivalent to calling [`SendReply()`](BMessage.md#BMessage_SendReply "SendReply\(\)"), except that the return message won't be marked as a reply. If a reply isn't allowed (if the `BMessage` wasn't delivered), the returned [`BMessenger`](BMessenger.md "BMessenger") will be invalid.

If you want to use the `ReturnAddress()` [`BMessenger`](BMessenger.md "BMessenger") to send a synchronous reply, you must do so before the `BMessage` is deleted and a default reply is sent.

See also: [`SendReply()`](BMessage.md#BMessage_SendReply "SendReply\(\)"), [`WasDelivered()`](BMessage.md#BMessage_WasDelivered "WasDelivered\(\), IsSourceRemote\(\), IsSourceWaiting\(\), IsReply\(\), Previous\(\)")

#### SendReply()

`status_t SendReply(BMessage* message,  
BMessage* reply,  
bigtime_t sendTimeout = B_INFINITE_TIMEOUT,  
bigtime_t replyTimeout = B_INFINITE_TIMEOUT);` `status_t SendReply(BMessage* message,  
BHandler* replyHandler = NULL,  
bigtime_t sendTimeout = B_INFINITE_TIMEOUT);` `status_t SendReply(uint32 command,  
BMessage* reply);` `status_t SendReply(uint32 command,  
BHandler* replyHandler = NULL);`

Sends a reply `message` back to the sender of the `BMessage` (in the case of a synchronous reply) or to a target [`BHandler`](BHandler.md "BHandler") (in the case of an asynchronous reply). Whether the reply is synchronous or asynchronous depends on how the `BMessage` that's sending the reply was itself sent:

  * The reply is delivered synchronously if the message sender is waiting for one to arrive. The function that sent the `BMessage` doesn't return until it receives the reply (or a timeout expires). If an expected reply has not been sent by the time the `BMessage` object is deleted, a default `B_NO_REPLY` message is returned to the sender. If a reply is sent after the sender gave up waiting for it to arrive, the reply `message` disappears into the bowels of the system.

  * The reply is delivered asynchronously if the message sender isn't waiting for a reply. In this case, the sending function designates a target [`BHandler`](BHandler.md "BHandler") and [`BLooper`](BLooper.md "BLooper") for any replies that might be sent, then returns immediately after putting the `BMessage` in the pipeline. Posted messages and messages that are dragged and dropped are also eligible for asynchronous replies.

`SendReply()` works only for `BMessage` objects that have been processed through a message loop and delivered to you. The caller retains ownership of the reply `message` passed to `SendReply()`; it can be deleted (or left to die on the stack) after the function returns.

`SendReply()` sends a message—a reply message, to be sure, but a message nonetheless. It behaves exactly like the other message-sending function, [`BMessenger`](BMessenger.md "BMessenger")'s [`SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)"):

  * By passing it a `reply` argument, you can ask for a synchronous reply to the reply message it sends. It won't return until it receives the reply.

  * By supplying a `replyHandler` argument, you can arrange for an expected asynchronous reply. If a specific target isn't specified, the [`BApplication`](BApplication.md "BApplication") object will handle the reply if one is sent.

By default, `SendReply()` doesn't return until the reply message is delivered (placed in the [`BLooper`](BLooper.md "BLooper")'s port queue). It's possible, in some circumstances, for the receiving port queue to be full, in which case `SendReply()` will block until a slot becomes free. However, you can limit how long `SendReply()` will wait to deliver the message before it gives up and returns. The `sendTimeout` argument is the number of microseconds you give the function to do its work. If the time limit is exceeded, the function fails and returns an error (`B_TIMED_OUT`).

When asking for a synchronous reply, separate `sendTimeout` and `replyTimeout` limits can be set for sending the message and receiving the reply. There is no time limit if a timeout value is set to `B_INFINITE_TIMEOUT`—as it is by default. The function won't block at all if the timeout is set to 0.

If a `command` is passed rather than a `message`, `SendReply()` constructs the reply `BMessage`, initializes its `what` data member with the `command` constant, and sends it just like any other reply. The `command` versions of this function have infinite timeouts; they block until the message is delivered and, if requested, a synchronous reply is received.

This function returns `B_OK` if the reply is successfully sent. If there's a problem in sending the message, it returns the same sort of error code as [`BMessenger`](BMessenger.md "BMessenger")'s [`SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)"). It may also report a reply-specific problem. The more informative return values are as follows:

Constant| Description  
---|---  
`B_BAD_REPLY`| Attempting to reply to a message that hasn't been delivered yet  
`B_DUPLICATE_REPLY`| Sending a reply after one has already been sent and delivered.  
`B_BAD_THREAD_ID`| Sending a reply to a destination thread that no longer exists.  
`B_BAD_PORT_ID`| Sending a reply to a [`BLooper`](BLooper.md "BLooper") and port that no longer exist.  
`B_TIMED_OUT`| Taking longer than the specified time limit to deliver a reply message or to receive a synchronous reply to the reply.  
  
If you want to delay sending a reply and keep the `BMessage` object beyond the time it's scheduled to be deleted, you may be able to detach it from the message loop. See [`DetachCurrentMessage()`](BLooper.md#BLooper_DetachCurrentMessage) in the [`BLooper`](BLooper.md "BLooper") class.

See also: [`BMessenger::SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)"), [`BLooper::DetachCurrentMessage()`](BLooper.md#BLooper_DetachCurrentMessage), [`ReturnAddress()`](BMessage.md#BMessage_ReturnAddress "ReturnAddress\(\)")

####  WasDelivered(), IsSourceRemote(), IsSourceWaiting(), IsReply(), Previous()

`bool WasDelivered() const;``bool IsSourceRemote() const;``bool IsSourceWaiting() const;``bool IsReply() const;``const BMessage* Previous() const;`

These functions can help if you're engaged in an exchange of messages or managing an ongoing communication.

`WasDelivered()` indicates whether it's possible to send a reply to a message. It returns `true` for a `BMessage` that was posted, sent, or dropped – that is, one that has been processed through a message loop—and `false` for a message that has not yet been delivered by any means.

`IsSourceRemote()` returns `true` if the message had its source in another application, and `false` if the source is local or the message hasn't been delivered yet.

`IsSourceWaiting()` returns `true` if the message source is waiting for a synchronous reply, and `false` if not. The source thread can request and wait for a reply when calling either [`BMessenger`](BMessenger.md "BMessenger")'s [`SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)") or `BMessage`'s [`SendReply()`](BMessage.md#BMessage_SendReply "SendReply\(\)") function.

`IsReply()` returns `true` if the `BMessage` is a reply to a previous message (if it was sent by the [`SendReply()`](BMessage.md#BMessage_SendReply "SendReply\(\)") function), and `false` if not.

`Previous()` returns the previous message – the message to which the current `BMessage` is a reply. It works only for a `BMessage` that's received as an asynchronous reply to a previous message. A synchronous reply is received in the context of the previous message, so it's not necessary to call a function to get it. But when an asynchronous reply is received, the context of the original message is lost; this function can provide it. `Previous()` returns `NULL` if the `BMessage` isn't an asynchronous reply to another message.

See also: [`BMessenger::SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)"), [`SendReply()`](BMessage.md#BMessage_SendReply "SendReply\(\)"), [`ReturnAddress()`](BMessage.md#BMessage_ReturnAddress "ReturnAddress\(\)")

####  WasDropped(), DropPoint()

`bool WasDropped() const;``BPoint DropPoint(BPoint* offset = NULL) const;`

`WasDropped()` returns `true` if the user delivered the `BMessage` by dragging and dropping it, and `false` if the message was posted or sent in application code or if it hasn't yet been delivered at all.

`DropPoint()` reports the point where the cursor was located when the message was dropped (when the user released the mouse button). It directly returns the point in the screen coordinate system and, if an `offset` argument is provided, returns it by reference in coordinates based on the image or rectangle the user dragged. The `offset` assumes a coordinate system with (0.0, 0.0) at the left top corner of the dragged rectangle or image.

Since any value can be a valid coordinate, `DropPoint()` produces reliable results only if `WasDropped()` returns `true`.

See also: [`BView::DragMessage()`](BView.md#BView_DragMessage "DragMessage\(\)")

* * *

### Operators

#### = (assignment)

`BMessage operator=(const BMessage&);`

Assigns one `BMessage` object to another. After the assignment, the two objects are duplicates of each other without shared data.

#### new

`void* operator new(size_t numBytes);`

Allocates memory for a `BMessage` object, or takes the memory from a previously allocated cache. The caching mechanism is an efficient way of managing memory for objects that are created frequently and used for short periods of time, as `BMessage`s typically are.

#### delete

`void operator delete(void* memory,  
size_t numBytes);`

Frees memory allocated by the `BMessage` version of `new`, which may mean restoring the memory to the cache.

* * *

### Constants

####  Message Specifiers
    
    
    B_NO_SPECIFIER
    B_DIRECT_SPECIFIER
    B_INDEX_SPECIFIER
    B_REVERSE_INDEX_SPECIFIER
    B_RANGE_SPECIFIER
    B_REVERSE_RANGE_SPECIFIER
    B_NAME_SPECIFIER
    B_ID_SPECIFIER
    B_SPECIFIERS_END = 128

These constants fill the `what` slot of specifier `BMessage`s. Each constant indicates what other information the specifer contains and how it should be interpreted. For example, a `B_REVERSE_INDEX_SPECIFIER` message has an `index` field with an index that counts backwards from the end of a list. A `B_NAME_SPECIFIER` message includes a `name` field that names the requested item.

See Also: [`AddSpecifier()`](BMessage.md#BMessage_AddSpecifier "AddSpecifier\(\)"), the [Scripting](TheApplicationKit_Scripting.md "Scripting") chapter.

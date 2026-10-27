<!-- Classes And Methods - The Application Kit - BPropertyInfo -->

## BPropertyInfo

| Derived From:| [`BFlattenable`](BFlattenable.md "BFlattenable")  
---|---  
Mix-in Classes:| –  
Declared In:| `app/PropertyInfo.h`  
Library:| `libbe.so`  
Allocation:| Constructor only  
[Class Overview](BPropertyInfo_Overview.md "BPropertyInfo")

  * [Constructor and Destructor](BPropertyInfo.md#BPropertyInfo_ConstructorDestructor)
  * [Member Functions](BPropertyInfo.md#BPropertyInfo_MemberFunctions)
  * [Defined Types](BPropertyInfo.md#BPropertyInfo_DefinedTypes)

  
  
* * *

### Constructor and Destructor

#### BPropertyInfo()

`BPropertyInfo(property_info* p = NULL,  
bool free_on_delete = false);`

Initializes the object with the specified zero-terminated array `p` of [property_info](BPropertyInfo.md#property_info "property_info"). Passing `true` in `free_on_delete` instructs the object to free the memory associated with the [property_info](BPropertyInfo.md#property_info "property_info") when the object is destroyed. `BPropertyInfo` does not copy the array, so it is important that the array is not deleted or otherwise destroyed while the `BPropertyInfo` is in use.

#### ~BPropertyInfo()

`~BPropertyInfo();`

If `free_on_delete` is set to `true` in the constructor, the destructor frees all memory associated with the [property_info](BPropertyInfo.md#property_info "property_info"). Otherwise, it does nothing.

* * *

### Member Functions

#### AllowsTypeCode()

See [`BFlattenable::AllowsTypeCode()`](BFlattenable.md#BFlattenable_AllowsTypeCode).

#### FindMatch()

`int32 FindMatch([`BMessage`](BMessage.md "BMessage")* msg,  
int32 index,  
[`BMessage`](BMessage.md "BMessage")* spec,  
int32 form,  
const char* prop,  
void* data = NULL) const;`

Passed a property name in `prop`, a specifier in `form`, and a command in `msg`->`what`, searches the [property_info](BPropertyInfo.md#property_info "property_info") array for an item supporting the specified scripting request. If `index` is nonzero, then `FindMatch()` only searches those [property_info](BPropertyInfo.md#property_info "property_info") structures with the wildcard command (first element of command array equal to 0). Otherwise, it searches through all available [property_info](BPropertyInfo.md#property_info "property_info") structures for a match. If a match is found, it fills the memory at `data` with the contents of the `extra_data` field of the match and returns the index of the match in the array. Otherwise, it returns `B_ERROR`.

#### Flatten()

See [`BFlattenable::Flatten()`](BFlattenable.md#BFlattenable_Flatten "Flatten\(\), Unflatten\(\)").

#### FlattenedSize()

See [`BFlattenable::FlattenedSize()`](BFlattenable.md#BFlattenable_FlattenedSize "FlattenedSize\(\)").

#### IsFixedSize()

See [`BFlattenable::IsFixedSize()`](BFlattenable.md#BFlattenable_IsFixedSize "IsFixedSize\(\)").

#### TypeCode()

See [`BFlattenable::TypeCode()`](BFlattenable.md#BFlattenable_TypeCode "TypeCode\(\) , AllowsTypeCode\(\)").

#### PrintToStream()

`void PrintToStream() const;`

Prints information about the `BPropertyInfo` to standard output.

#### PropertyInfo()

`const property_info* PropertyInfo() const;`

Returns the [property_info](BPropertyInfo.md#property_info "property_info") list associated with the object.

#### Unflatten()

See [`BFlattenable::Unflatten()`](BFlattenable.md#BFlattenable_Unflatten).

* * *

### Defined Types

#### property_info
    
    
    struct property_info {
        char*  name;
        uint32 commands[10];
        uint32 specifiers[10];
        char*  usage;
        uint32 extra_data;
    };

Field| Description  
---|---  
`name`| Provides the name of the property this structure describes.  
`commands`| Is a zero-terminated array of commands understood by the property, i.e. `B_GET_PROPERTY`. If the first element is 0, it represents a wildcard matching all possible commands.  
`specifiers`| Is a zero-terminated array of the specifiers understood by the property, i.e. `B_DIRECT_SPECIFIER`. If the first element is 0, it represents a wildcard matching all possible specifiers.  
`usage`| Gives a human-readable string describing the property and its allowable commands and specifiers.  
`extra_data`| Is an area free for general use; the operating system does not touch its contents.

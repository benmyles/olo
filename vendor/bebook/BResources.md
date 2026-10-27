<!-- Classes And Methods - The Storage Kit - BResources -->

## BResources

| Derived From:| None  
---|---  
Mix-in Classes:| None  
Declared In:| `storage/Resources.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BResources_Overview.md "BResources")

  * [Constructor and Destructor](BResources.md#BResources_ConstructorDestructor)
  * [Member Functions](BResources.md#BResources_Member_Functions)

  
  
* * *

### Constructor and Destructor

#### BResources()

`BResources();``BResources([`BFile`](BFile.md "BFile")* file,  
bool clobber = false);`

Creates a new `BResources` object. You can initialize the object by passing a pointer to a valid [`BFile`](BFile.md "BFile"); without the argument, the object won't refer to a file until [`SetTo()`](BResources.md#BResources_SetTo "SetTo\(\)") is called.

If `clobber` is `true`, the file that's referred to by [`BFile`](BFile.md "BFile") is truncated (it's data is erased), and a new resource file header is written to the file. If `clobber` is `false` and the file doesn't otherwise doesn't have a resource header, the initialization fails.

`BResources` copies the [`BFile`](BFile.md "BFile") argument; after the constructor returns, you can, for example, delete the [`BFile`](BFile.md "BFile") that you passed in.

#### ~BResources()

`virtual ~BResources();`

Destroys the BResources object.

* * *

### Member Functions

#### AddResource()

`status_t AddResource(type_code type,  
int32 id,  
const void* data,  
size_t length,  
const char* name = NULL);`

Adds a new resource to the file. For this function to have an effect, the file must be open for writing. The arguments are:

  * `type` is one of the type_code constants defined in `support/TypeConstants.h`.

  * `id` is the ID number that you want to assign to the resource. The value of the ID has no meaning other than that which your application gives it; the only restriction on the ID is that the combination of it and the data type constant must be unique across all resources in this resource file.

  * `data` is a pointer to the data that you want the resource to hold.

  * `length` is the length of the data buffer, in bytes.

  * `name` is optional, and needn't be unique. Or even interesting.

Ownership of the data pointer isn't assigned to the `BResources` object by this function; after `AddResource()` returns, your application can free or otherwise manipulate the buffer that data points to without affecting the data that was written to the file.

Return Code| Description  
---|---  
`B_OK`.| The resource was successfully added.  
`B_NOT_ALLOWED`.| The is open read-only.  
`B_FILE_ERROR`.| The file's resource map doesn't exist or is invalid.  
`B_BAD_VALUE`.| The `data` pointer is `NULL`.  
`B_NO_MEMORY`.| Not enough memory to add the resource to the file.  
  
#### File()

`const [`BFile`](BFile.md "BFile")& File() const;`

Returns the [`BFile`](BFile.md "BFile") the `BResource` object references.

#### GetResourceInfo()

`bool GetResourceInfo(int32 byIndex,  
type_code* typeFound,  
int32* idFound,  
const char** nameFound,  
size_t* lengthFound);` `bool GetResourceInfo(type_code byType,  
int32 andIndex,  
int32* idFound,  
const char** nameFound,  
size_t* lengthFound);` `bool GetResourceInfo(type_code byType,  
int32 andId,  
const char** nameFound,  
size_t* lengthFound);` `bool GetResourceInfo(type_code byType,  
const char* andName,  
int32* idFound,  
size_t* lengthFound);` `bool GetResourceInfo(const void* byPointer,  
type_code* typeFound,  
int32* idFound,  
size_t* lengthFound,  
const char** nameFound);`

These functions return information about a specific resource, as identified by the first one or two arguments:

  * The first version (`byIndex`) searches for the `byIndex`'th resource in the file.

  * The second (`byType`/`andIndex`) searches for the `byIndex`'th resource that has the given type.

  * The third (`byType`/`andId`) looks for the resource with the unique combination of type and ID.

  * The fourth (`byType`/`andName`) looks for the first resource that has the given type and name.

  * The last (`byPointer`) returns information about the resource whose data is pointed to by `byPointer`. This can be used to trace a resource that's already been loaded with [`LoadResource()`](BResources.md#BResources_LoadResource "LoadResource\(\)") back to its information.

The other arguments return the other statistics about the resource (if found).

The pointer that's returned in `*foundName` belongs to the `BResources`. Don't free it.

The functions return `true` if a resource was found and `false` otherwise.

#### HasResource()

`bool HasResource(type_code type,  
int32 id);` `bool HasResource(type_code type,  
const char* name);`

Returns `true` if the resource file contains a resource as identified by the arguments, otherwise it returns `false`.

Keep in mind that there may be more than one resource in the file with the same name and type combination. The type and id combo, on the other hand, is unique. See "[Identifying a Resource within a Resource File](BResources_Overview.md#BResources_Identifying_A_Resource_Within_A_Resource_File "Identifying a Resource within a Resource File")."

#### LoadResource()

`const void* LoadResource(type_code type,  
int32 id,  
size_t* outSize);` `const void* LoadResource(type_code type,  
const char* name,  
size_t* outSize);`

Loads the specified resource into memory. The resource can be identified by either type code and ID or by type code and name. See "[Identifying a Resource within a Resource File](BResources_Overview.md#BResources_Identifying_A_Resource_Within_A_Resource_File "Identifying a Resource within a Resource File")."

The returned pointer belongs to the resource file; it's valid until the resource gets changed. If an error occurs while trying to load the resource, `NULL` is returned.

#### MergeFrom()

`status_t MergeFrom([`BFile`](BFile.md "BFile")* fromFile);`

Copies all the resource from the file specified by `fromFile` into the file targeted by the `BResources` object. The original file isn't changed. You can do this to a file that's opened read-only, but the changes won't have any effect.

Return Code| Description  
---|---  
`B_OK`.| The resources were copied without error.  
`B_BAD_FILE`.| The resource map is empty.  
`B_FILE_ERROR`.| A file error occurred, or the resource map is nonexistent.  
`B_IO_ERROR`.| An error occurred while writing the data.  
`B_ERROR`.| Something else went wrong.  
  
#### PreloadResourceType()

`status_t PreloadResourceType(type_code type = 0);`

If you know you're going to need to access all resources of a particular type, you can preload them all into memory in one shot using this function. If you specify a type of 0, all resources of all types are preloaded.

Return Code| Description  
---|---  
`B_OK`.| The resources were preloaded without error.  
`B_BAD_FILE`.| The resource map is empty.  
Other values.| The returned value is the negative of the number of errors that occurred while preloading resources; for example, if five errors occurred, the result is -5.  
  
#### RemoveResource()

`status_t RemoveResource(type_code type,  
int32 id);` `status_t RemoveResource(const void* resource);`

Removes the resource identified by the arguments. See "[Identifying a Resource within a Resource File](BResources_Overview.md#BResources_Identifying_A_Resource_Within_A_Resource_File "Identifying a Resource within a Resource File")."

Return Code| Description  
---|---  
`B_OK`.| The resource was removed.  
`B_FILE_ERROR`.| The file's resource map doesn't exist.  
`B_NOT_ALLOWED`.| The file is opened read-only.  
`B_NOT_ALLOWED`.| Couldn't find the specified resource, or an error occurred trying to remove it (first form of [`RemoveResource()`](BResources.md#BResources_RemoveResource "RemoveResource\(\)")).  
`B_ERROR`.| Couldn't find the specified resource, the pointer doesn't indicate a valid resource, or an error occurred while removing the resource (second form).  
  
#### SetTo()

`status_t SetTo([`BFile`](BFile.md "BFile")* file,  
bool clobber = false);`

Unlocks and closes the object's previous [`BFile`](BFile.md "BFile"), and re-initializes it to refer to a copy of the argument. If the new BFile is open for writing, the `BResources`' copy of the [`BFile`](BFile.md "BFile") is locked.

If `clobber` is `true`, the file that's referred to by [`BFile`](BFile.md "BFile") is truncated (it's data is erased), and a new resource file header is written to the file. If `clobber` is `false` and the file doesn't otherwise doesn't have a resource header, the initialization fails.

Return Code| Description  
---|---  
`B_OK`.| The resource was removed.  
`B_BAD_VALUE`.| The argument [`BFile`](BFile.md "BFile") is invalid (uninitialized).  
`B_ERROR`.| The `BResources` couldn't be initialized (for whatever reason).  
  
#### Sync()

`status_t Sync();`

Updates all changed resources on disk. This actually rewrites the entire resource file, so be aware of this when designing your code. This is a very good reason not to use `BResources` for anything other than permanent, nonchanging application data, and only developer tools should write to resource files.

Return Code| Description  
---|---  
`B_OK`.| The resources were updated without error.  
`B_BAD_FILE`.| The resource map is empty.  
`B_NOT_ALLOWED`.| The file is opened read-only.  
`B_FILE_ERROR`.| A file error occurred.  
`B_IO_ERROR`.| An error occurred while writing the data.  
  
#### WriteTo()

`status_t WriteTo([`BFile`](BFile.md "BFile")* newFile);`

Writes all the file's resources into a new file. After this function returns, the `BResources` object is targeted on the new [`BFile`](BFile.md "BFile"); all future operations will be done in the new file.

Return Code| Description  
---|---  
`B_OK`.| The resources were written without error.  
Other errors.| File errors indicating problems copying the data.

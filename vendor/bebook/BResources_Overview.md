<!-- System Overview - The Storage Kit - BResources -->

## BResources  
  
Note

![Note](./images/admonitions/Info_32.png)

You may not want to be here… The `BResources` class was designed for a specific purpose: To provide a means to bundle application "resources" (icons, in particular) within the application executable itself. If you want to add new resources to your own application (resources that you want to have "stick" to the executable), then you've come to the right place. But you shouldn't use `BResources` to add data to a regular data file—use attributes instead.

The data that a file contains is either "flat," or it's "structured." To read a flat file, you simply open it (through a [`BFile`](BFile.md "BFile") object) and start [`Read()`](BFile.md#BFile_Read "Read\(\), ReadAt\(\), Write\(\), WriteAt\(\)")'ing. Structured data requires that you understand the structure. Typically, an application understands the structure either because it's a well-known format, or because the application itself wrote the file in the first place.

The `BResources` class defines a simple design for storing structured data. The structure is a series of "resources," where each resource is key/value pair. A single "resource file" can hold an unlimited number of resources; a single resource within a resource file can contain an unlimited amount of data.

Resources are sort of like attributes in that they store chunks of data that are looked up through the use of a key. But note these differences:

  * Resources are stored in the file itself, such that if you copy the file, you copy the resources, as well.

  * Resources can't be queried.

  * Only plain files can have resources. (In other words, directories and symbolic links can't have resources.)

* * *

### Initializing a BResources Object

The `BResources` class provides the means for reading and writing a file's resources, but it doesn't let you access the file directly. Instead, you must initialize the `BResources` object by passing it a valid [`BFile`](BFile.md "BFile") object, either in the constructor or the [`SetTo()`](BResources.md#BResources_SetTo "SetTo\(\)") function. Note the following:

  * The [`BFile`](BFile.md "BFile") that you pass in is copied by the `BResources` object. Thus, initializing a `BResources` object opens a new file descriptor into the file. You can delete the "original" [`BFile`](BFile.md "BFile") immediately after you use it to initialize the `BResources` object.

  * Care must be taken to avoid writing to a [`BFile`](BFile.md "BFile") that other applications have open for reading. `BResources` can't enforce this rule, but if you're not careful to abide by it, problems can (and will) occur. Likewise, multiple applications mustn't open the same file for writing at the same time.

  * If you want to write resources, the [`BFile`](BFile.md "BFile") must not be locked when you pass it in. The `BResources` needs to be able to lock its copy of your object.

  * The [`BFile`](BFile.md "BFile") must be open for reading (at least).

  * Unfortunately, `BResources` lacks an `InitCheck()` function. If you want to check initialization errors, you should always initialize through [`SetTo()`](BResources.md#BResources_SetTo "SetTo\(\)"), rather than through the constructor.

#### Identifying and Creating Resource Files

You can't use just any old file as a `BResources` initializer: The file must be an actual resource file. Simply initializing a `BResources` object with an existing non-resource file will not transform the file into a resource file—unless you tell the initializer to clobber the existing file.

For example, this initialization fails:
    
    
    /* "fido" exists, but isn't a resource file. */
    BFile file("/boot/home/fido", B_READ_WRITE);
    BResources res;
    status_t err;
    
    if ((err = res.SetTo(&file)) != B_OK)
    ...

And this one succeeds…
    
    
    /* The second arg to SetTo() is the "clobber?" flag. */
    if ((err = res.SetTo(&file, true)) != B_OK)
    ...

…but at a price: fido's existing data is destroyed (truncated to 0 bytes), and a new "resource header" is written to the file. Having gained a resource header, fido can thereafter be used to initialize a `BResources` object.

Clobber-setting a resource file is possible, but, as mentioned at the top of this class description, you'll probably never create resource files directly yourself

So where do resource files come from if you don't create them yourself? Step right up…

#### Executables as Resource Files

The only files that are naturally resource-ful are application executables. For example, here we initialize a `BResources` object with the IconWorld executable:
    
    
    BPath path;
    BFile file;
    BResources res;
    
    find_directory(B_APPS_DIRECTORY, &path);
    path.Append("IconWorld");
    file.SetTo(&path, B_READ_ONLY);
    
    if (res.SetTo(&file) != B_OK)
       ...

The `BResources` object is now primed to look at IconWorld's resources. But be aware that an application's "app-like" resources (its icons, signature, app flags) should be accessed through the [`BAppFileInfo`](BAppFileInfo.md "BAppFileInfo") class.

* * *

### Resource Data

After you've initialized your `BResources` object, you use the `FiddleResource()` functions to examine and manipulate the file's resources:

#### Generative Functions

  * [`AddResource()`](BResources.md#BResources_AddResource "AddResource\(\)") adds a new resource to the file.

  * [`RemoveResource()`](BResources.md#BResources_RemoveResource "RemoveResource\(\)") removes an existing resource from the file.

#### Data Function

  * [`LoadResource()`](BResources.md#BResources_LoadResource "LoadResource\(\)") loads a resource from disk and returns a pointer to it.

#### Info Functions

  * [`HasResource()`](BResources.md#BResources_HasResource "HasResource\(\)") tells you if the file contains a specified resource.

  * [`GetResourceInfo()`](BResources.md#BResources_GetResourceInfo "GetResourceInfo\(\)") returns information about a resource.

As mentioned earlier, the [`BFile`](BFile.md "BFile") that you use to initialize a `BResources` object must be open for reading. If you also want to modify the resources (by adding, removing, or writing) the [`BFile`](BFile.md "BFile") must also be open for writing.

#### Identifying a Resource within a Resource File

A single resource within a resource file is tagged with a data type, an ID, and a name:

  * The data type is one of the type_code types (`B_INT32_TYPE`, `B_STRING_TYPE`, and so on) that characterize different types of data. The data type that you assign to a resource doesn't restrict the type of data that the resource can contain, it simply serves as a way to label the type of data that you're putting into the resource so you'll know how to cast it when you retrieve it.

  * The ID is an arbitrary integer that you invent yourself. It need only be meaningful to the application that uses the resource file.

  * The name is optional, but can be useful: You can look up a resource by its name, if it has one.

Taken singly, none of these tags needs to be unique: Any number of resources (within the same file) can have the same data type, ID, or name. It's the combination of the data type constant and the ID that uniquely identifies a resource within a file. The name, on the other hand, is more of a convenience; it never needs to be unique when combined with the data type or with the ID.

Some functions also provide the option to use a pointer to a resource's data to identify the resource; once a resource has been loaded into memory by calling [`LoadResource()`](BResources.md#BResources_LoadResource "LoadResource\(\)"), you can use the resulting pointer to identify it.

#### Data Format

All resource data is assumed to be "raw": If you want to store a `NULL`-terminated string in a resource, for example, you have to write the `NULL` as part of the string data, or the application that reads the resource from the resource must apply the `NULL` itself. Put more generally, the data in a resource doesn't assume any particular structure or format, it's simply a vector of bytes.

#### Data Ownership

Resource data that you retrieve from a `BResources` object belongs to the `BResources` object. You mustn't `free()` these pointers.

Individual changes that you make to the resource file are cached in memory until you call the [`Sync()`](BResources.md#BResources_Sync "Sync\(\)") function. Other applications won't see the changes until then.

* * *

### Reading and Writing a Resource File as a Plain File

Just because a file is a resource file, that doesn't mean that you're prevented from reading and writing it as a plain file (through the [`BFile`](BFile.md "BFile") object). For example, it's possible to create a resource file, add some resources to it, and then use a [`BFile`](BFile.md "BFile") object to seek to the end of the file and write some flat data. But you have to keep track of the "data map" yourself—if you go back and add more resources to the file (or extend the size of the existing ones), your flat data will be overwritten: The `BResources` object doesn't preserve non-resource data that lives in the file that it's operating on.

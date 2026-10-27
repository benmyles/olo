<!-- Classes And Methods - The Storage Kit - BAppFileInfo -->

## BAppFileInfo

| Derived From:| [`BNodeInfo`](BNodeInfo.md "BNodeInfo")  
---|---  
Mix-in Classes:| None  
Declared In:| `storage/AppFileInfo.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BAppFileInfo_Overview.md "BAppFileInfo")

  * [Constructor and Destructor](BAppFileInfo.md#BAppFileInfo_ConstructorDestructor)
  * [Member Functions](BAppFileInfo.md#BAppFileInfo_MemberFunctions)
  * [Constants](BAppFileInfo.md#BAppFileInfo_Constants)
  * [Defined Types](BAppFileInfo.md#BAppFileInfo_DefinedTypes)

  
  
* * *

### Constructor and Destructor

#### BAppFileInfo()

`BAppFileInfo();` `BAppFileInfo([`BFile`](BFile.md "BFile")* file);`

The default constructor creates a new, uninitialized `BAppFileInfo` object. To initialize you have to follow this construction with a call to [`SetTo()`](BAppFileInfo.md#BAppFileInfo_SetTo "SetTo\(\)").

The [`BFile`](BFile.md "BFile") version intializes the `BAppFileInfo` by passing the argument to [`SetTo()`](BAppFileInfo.md#BAppFileInfo_SetTo "SetTo\(\)").

#### ~BAppFileInfo()

`~BAppFileInfo();`

Destroys the object. The [`BFile`](BFile.md "BFile") that was used to initialize this object isn't touched.

* * *

### Member Functions

####  GetAppFlags(), SetAppFlags()

`status_t GetAppFlags(uint32* flags) const;``status_t SetAppFlags(uint32 flags);`

These functions get and set the executable's "app flags." These are the constants that determine whether an executable can only be launched once, whether it runs in the background, and so on. The app flag constants are defined in `app/Roster.h`; the flags must include one of the following…

  * `B_SINGLE_LAUNCH`

  * `B_MULTIPLE_LAUNCH`

  * `B_EXCLUSIVE_LAUNCH`

…plus either of these two:

  * `B_BACKGROUND_APP`

  * `B_ARGV_ONLY`

See the [`BApplication`](BApplication.md "BApplication") class for details on the meanings of these constants.

Return Code| Description  
---|---  
`B_OK`| Everything went fine.  
`B_NO_INIT`| The object is not properly initialized.  
`B_BAD_VALUE`| `NULL` flags.  
`B_BAD_TYPE`| The attribute/resources the flags are stored in have the wrong type.  
`B_ENTRY_NOT_FOUND`| No application flags are set on the file.  
  
####  GetIcon(), SetIcon(), GetIconForType(), SetIconForType()

`status_t GetIcon([`BBitmap`](BBitmap.md "BBitmap")* icon,  
icon_size which) const;``status_t SetIcon([`BBitmap`](BBitmap.md "BBitmap")* icon,  
icon_size which);``status_t GetIconForType(const char* file_type,  
[`BBitmap`](BBitmap.md "BBitmap")* icon,  
icon_size which) const;``status_t SetIconForType(const char* file_type,  
[`BBitmap`](BBitmap.md "BBitmap")* icon,  
icon_size which);`

`GetIcon()` and `SetIcon()` get and set the icons that are represent the executable. `GetIconForType()` and `SetIconForType()` get and set the icons that the executable uses when it writes (or otherwise "takes possession of") a file of the given type, identified by `file_type`. You specify which icon you want (large or small) by passing `B_LARGE_ICON` or `B_MINI_ICON` as the `which` argument.

The icon is passed in or returned through the icon argument. The bitmap must be the proper size: 32x32 for the large icon, 16x16 for the small one. In [`BRect`](BRect.md "BRect") lingo, that's `BRect`(0, 0, 31, 31) and `BRect`(0, 0, 15, 15). The bitmap's color space must be `B_CMAP8`. For example:
    
    
    BBitmap* bitmap = new BBitmap(BRect(0,0,31,31), B_CMAP8);
    appFileInfo.GetIcon(bitmap, B_LARGE_ICON);

You can remove an icon by passing `NULL` as the `icon` argument to `SetIcon()`.

Note

![Note](./images/admonitions/Info_32.png)

To create a [`BBitmap`](BBitmap.md "BBitmap") you must have a [`be_app`](BApplication.md#BApplication_be_app "be_app") object ; the object needn't be running.

Return Code| Description  
---|---  
`B_OK`.| Success  
`B_BAD_VALUE`.| (`Get…()`) `NULL` `BBitmap`, or invalid `file_type`.  
`B_BAD_VALUE`.| (`Set…()`) The bitmap data isn't the proper size.  
`B_NO_INIT`.| The object is not properly initialized.  
  
####  GetPreferredApp(), SetPreferredApp()

An application's preferred app must be itself; an add-on is more flexible. For syntax, see [`BNodeInfo::SetPreferredApp()`](BNodeInfo.md#BNodeInfo_SetPreferredApp).

####  GetSignature() , SetSignature()

`status_t GetSignature(char* signature) const;``status_t SetSignature(const char* signature);`

These functions get and set the executable's MIME type signature. The `signature` buffer you pass to `GetSignature()` should be at least `B_MIME_TYPE_LENGTH` characters long; the `SetSignature()` buffer must be no longer than that.

When you set an executable's signature, the signature is installed in the File Type database if it's not there already. The old signature isn't removed from the database.

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_ENTRY_NOT_FOUND`.| (`Get…()`) The executable doesn't have a signature.  
`B_BAD_VALUE`.| (`Set…()`) `signature` is too long.  
  
####  GetSupportedTypes(), SetSupportedTypes()

`status_t GetSupportedTypes([`BMessage`](BMessage.md "BMessage")* types) const;`

`status_t SetSupportedTypes(const [`BMessage`](BMessage.md "BMessage")* types);` `status_t SetSupportedTypes(const [`BMessage`](BMessage.md "BMessage")* types,  
bool sync_all);`

These functions get and set the MIME file types that this executable can read and/or write. The `types` [`BMessage`](BMessage.md "BMessage") that you pass in looks like this:

Field| Type| Description  
---|---|---  
`types` (array)| `B_STRING_TYPE`| An array of MIME strings.  
  
`GetSupportedTypes()` copies the types into the `types` field; `SetSupportedTypes()` reads them from the field. The message's command constants (its `what` value) is ignored.

Here we print an executable's supported types:
    
    
    BMessage msg;
    uint32 i=0;
    char* ptr;
    
    if (appFileInfo.GetSupportedTypes(&msg) != B_OK)
       /* Handle the error. */
    
    while (msg.FindString("types", i++, &ptr) == B_OK)
       printf("> Supported Type: %sn", ptr);

If `SetSupportedTypes()` names a type that doesn't already appear in the File Type database, the new type is added to the database and its preferred handler is set to the executable that this `BAppFileInfo` object represents.

Warning

![Warning](./images/admonitions/Stop_32.png)

`SetSupportedTypes()` clobbers an executable's existing set of supported types. If you want to augment an executable's supported types, you should retrieve the existing set, add the new ones, and _then_ call `SetSupportedTypes()`.

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_NO_MEMORY`.| Insufficient memory to copy the types.  
  
See Also: [`Supports()`](BAppFileInfo.md#BAppFileInfo_Supports)

####  GetType(), SetType()

Implementation detail; see "[Functions Inherited From BNodeInfo](BAppFileInfo_Overview.md#BAppFileInfo_Functions_Inherited_From_BNodeInfo "Functions Inherited From BNodeInfo")". An executable's default file type (for both applications and add-ons) is `B_APP_MIME_TYPE`. This value mustn't be changed. For syntax, see [`BNodeInfo::SetType()`](BNodeInfo.md#BNodeInfo_SetType).

####  GetVersionInfo(), SetVersionInfo()

`status_t GetVersionInfo(version_info* info,  
version_kind kind) const;``status_t SetVersionInfo(const version_info* info,  
version_kind kind);`

The functions get and set the application's "version info." The information is recorded in the version_info structure:
    
    
    struct version_info {
       uint32 major;
       uint32 middle;
       uint32 minor;
       uint32 variety;
       uint32 internal;
       char short_info[64];
       char long_info[256];
    }

The field names (and types) provide suggestions for the type of info they want to store.

There are two kinds of version info, as specified by the `kind` argument:

Constant| Description  
---|---  
`B_APP_VERSION_KIND`| Provides information about this specific app.  
`B_SYSTEM_VERSION_KIND`| Provides information about the "suite," or other grouping of apps, that this app belongs to.  
  
Again, the uses of the two kinds is up to the app developer—currently, nothing in the BeOS depends on any information being stored in either version_info structure.

Return Code| Description  
---|---  
`B_OK`.| The version_info was found or set.  
`B_ENTRY_NOT_FOUND`.| (`Get…()`) the app doesn't have the requested version info.  
  
#### InitCheck()

`status_t InitCheck() const;`

Returns the status of the most recent initialization.

Return Code| Description  
---|---  
`B_OK`.| The object was successfully initialized.  
`B_BAD_VALUE`.| The object is uninitialized.  
  
####  IsSupportedType(), Supports()

`bool IsSupportedType(const char* type) const;``bool Supports([`BMimeType`](BMimeType.md "BMimeType")* type) const;`

Returns `true` if the app supports the given MIME type and `false` if it doesn't. `IsSupportedType()` always returns `true` if the application supports "application/octet-stream", while `Supports()` returns `true` only if `type` is explicitly supported.

See Also: [`GetSupportedTypes()`](BAppFileInfo.md#BAppFileInfo_GetSupportedTypes "GetSupportedTypes\(\), SetSupportedTypes\(\)"), [`SetSupportedTypes()`](BAppFileInfo.md#BAppFileInfo_SetSupportedTypes)

####  SetInfoLocation(), IsUsingAttributes(), IsUsingResources()

`void SetInfoLocation(info_location loc);``bool IsUsingAttributes() const;``bool IsUsingResources() const;`

`SetInfoLocation()` sets the location where the `BAppFileInfo` object stores its information. It can store them as either attributes, resources, or both. `loc` takes the following values:

  * `B_USE_ATTRIBUTES`

  * `B_USE_RESOURCES`

  * `B_USE_BOTH`

`IsUsingAttributes()` and `IsUsingResources()` return `true` if the `BAppFileInfo` object is storing information in the designated location and `false` otherwise.

#### SetTo()

`status_t SetTo([`BFile`](BFile.md "BFile")* file);`

Initializes the `BAppFileInfo` object by pointing it to `file`, which must be a valid (initialized) [`BFile`](BFile.md "BFile") object, and must not be locked. The `BFile` is not copied, or re-opened by `BAppFileInfo`. In particular, the `BAppFileInfo` uses `file`'s file descriptor, and doesn't destroy the `BFile` object when it (the `BAppFileInfo`) is destroyed or reinitialized.

Warning

![Warning](./images/admonitions/Stop_32.png)

The `BAppFileInfo` object doesn't check to make sure that the file that you pass in really is an executable. Passing in a non-executable (a plain file, a directory, etc.) could corrupt the file.

Return Code| Description  
---|---  
`B_OK`.| The object was successfully initialized.  
`B_BAD_VALUE`.| `file` is invalid (uninitialized).  
  
* * *

### Constants

####  Version Kinds

Declared in: `storage/AppFileInfo.h`

Constant| Description  
---|---  
`B_APP_VERSION_KIND`| Records information about a specific application.  
`B_SYSTEM_VERSION_KIND`| Records information about a "suite," or other grouping of applications, that the application belongs to.  
  
These constants are used when setting or retrieving the version information attached to an application. There are two version information records for each application, and these two constants select which one you wish to reference. Although there is no prescribed use for these structures or their constants, it is suggested that `B_APP_VERSION_KIND` be used for application-specific version information, and `B_SYSTEM_VERSION_KIND` be used for information about the suite of applications to which the application belongs.

* * *

### Defined Types

#### version_info

Declared in: `storage/AppFileInfo.h`
    
    
    struct version_info {
        uint32    major;
        uint32    middle;
        uint32    minor;
        uint32    variety;
        uint32    internal;
        char      short_info[64];
        char      long_info[256];
    }

The version_info structure is used to contain version information about an application. Although none of these fields have prescribed uses, and you can use them for anything you want, their names do hint at their suggested uses.

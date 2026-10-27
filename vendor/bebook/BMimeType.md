<!-- Classes And Methods - The Storage Kit - BMimeType -->

## BMimeType

| Derived From:| None  
---|---  
Mix-in Classes:| None  
Declared In:| `storage/Mime.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BMimeType_Overview.md "BMimeType")

  * [Constructor and Destructor](BMimeType.md#BMimeType_ConstructorDestructor)
  * [Member Functions](BMimeType.md#BMimeType_Member_Functions)
  * [Operators](BMimeType.md#BMimeType_Operators)

  
  
* * *

### Constructor and Destructor

#### BMimeType()

`BMimeType();` `BMimeType(const char* MIME_string);`

Constructs a new `BMimeType` object and initializes its MIME type to a copy of `MIME_string` (if the argument is given). The rules of validity apply (see "[Valid MIME Strings](BMimeType_Overview.md#BMimeType_Valid_MIME_Strings "Valid MIME Strings")"). To see if the initialization was successful, call [`InitCheck()`](BMimeType.md#BMimeType_InitCheck "InitCheck\(\)") after you construct a new `BMimeType` object.

You can also set the MIME type through the [`SetTo()`](BMimeType.md#BMimeType_SetTo "SetTo\(\), Unset\(\)") function.

#### ~BMimeType()

`virtual ~BMimeType();`

Frees the object's MIME string and destroys the object.

* * *

### Member Functions

#### Contains()

`bool Contains(const BMimeType* other) const;`

Compares the MIME string with `other`, returning `true` if they are identical. If the object is a supertype and it's the supertype of `other`, then the method returns `true`. Otherwise, it returns `false`.

####  GetAppHint(), SetAppHint()

`status_t GetAppHint(entry_ref* app_ref) const;``status_t SetAppHint(const entry_ref* app_ref);`

These functions get and set the "app hint" for the object's application signature. The app hint is a path that identifies the executable that should be used when launching an application that has this signature. For example, when the Tracker needs to launch an app of type "application/YourAppHere", it asks the database for the application hint. This hint is converted to an entry_ref before it is passed to the caller. Of course, the path may not point to an application, or it might point to an application with the wrong signature (and so on)—that's why this is merely a hint.

`GetAppHint()` function initializes the entry_ref to the hint recorded in the database; the argument must be allocated before it's passed in.

`SetAppHint()` copies the path corresponding to the entry_ref into the database. `app_ref` should point to an executable file that has the same signature as this object's MIME type.

Return Code| Description  
---|---  
`B_OK`.| The ref was successfully retrieved or set.  
`B_NO_INIT`.| The `BMimeType` is uninitialized.  
`B_BAD_VALUE`.| (Set) The ref is uninitialized.  
  
See Also: [`BNodeInfo::GetAppHint()`](BNodeInfo.md#BNodeInfo_GetAppHint "GetAppHint\(\), SetAppHint\(\)")

####  GetAttrInfo(), SetAttrInfo()

`status_t GetAttrInfo([`BMessage`](BMessage.md "BMessage")* info) const;``status_t SetAttrInfo([`BMessage`](BMessage.md "BMessage")* info);`

These functions use a [`BMessage`](BMessage.md "BMessage") to get and set the list of attributes that are typically associated with files of the MIME type. The [`BMessage`](BMessage.md "BMessage") must have the following fields:

Field Name| Type| element[0..n]  
---|---|---  
`attr:name`| `B_STRING_TYPE`| Each element is the name of one attribute.  
`attr:public_name`| `B_STRING_TYPE`| Each element is the human-readable name of one attribute.  
`attr:type`| `B_INT32_TYPE`| Each element is the type code for the corresponding attribute.  
`attr:public`| `B_BOOL_TYPE`| `true` if the attribute is public, `false` if it's private.  
`attr:editable`| `B_BOOL_TYPE`| `true` if the attribute should be user-editable, `false` if not.  
  
You can actually have any fields you want; it's up to applications to determine which attributes they recognize and which they don't.

Each element in each field describes the next attribute. If a file has three attributes, there should be three elements in each field, one per attribute.

Return Code| Description  
---|---  
`B_OK`.| No error.  
`B_BAD_VALUE`.| Invalid file.  
  
####  GetFileExtensions(), SetFileExtensions()

`status_t GetFileExtensions([`BMessage`](BMessage.md "BMessage")* msg) const;``status_t SetFileExtensions(const [`BMessage`](BMessage.md "BMessage")* msg);`

The database associates a list of file extensions (`.xxx` filename appendages) with each file type. If a file is otherwise untyped, clients of the database can figure out its type by matching the file's extension to the lists in the database.

These functions get and set the file extensions that are associated with the object's MIME type.

  * If you're getting the extensions, you'll find them copied into your [`BMessage`](BMessage.md "BMessage")'s `extensions` field (the [`BMessage`](BMessage.md "BMessage") must be allocated). They're given as an indexed array of strings (`B_STRING_TYPE`).

  * Similarly, you pass in the extensions by adding strings to the message's `extensions` field.

  * The [`BMessage`](BMessage.md "BMessage")'s `what` field is unimportant.

For example, to retrieve all the extensions that correspond to this object's MIME type, you would do the following:
    
    
    BMessage msg();
    uint32 i=0;
    char *ptr;
    
    if (mime.GetFileExtensions(&msg) != B_OK)
       /* Handle the error. */
    
    while (true) {
       if (msg.FindString("extensions", i++, &ptr) != B_OK)
          break;
       printf("> Extension: %sn", ptr);
    }

A given extension can be associated with more than one MIME type.

A `NULL` msg to `SetFileExtensions()` clears the type's extension list.

Warning

![Warning](./images/admonitions/Stop_32.png)

`SetFileExtensions()` clobbers the existing set of extensions. If you want to augment a type's extensions, you should retrieve the existing set, add the new ones, and then call `SetFileExtensions()`.

Also, there's no way to ask the database to give you a set of file types that map to a given extension. To find a type for an extension, you have to get all the installed types with [`GetInstalledTypes()`](BMimeType.md#BMimeType_GetInstalledTypes "GetInstalledTypes\(\), GetInstalledSupertypes\(\)") and ask each one for its set of extensions.

Return Code| Description  
---|---  
`B_OK`.| The extensions were found or set.  
`B_NO_INIT`.| The `BMimeType` is uninitialized.  
`B_NO_MEMORY`.| Insufficient memory to copy the extensions.  
  
####  GetIcon(), SetIcon()

`status_t GetIcon([`BBitmap`](BBitmap.md "BBitmap")* icon,  
icon_size which) const;``status_t SetIcon([`BBitmap`](BBitmap.md "BBitmap")* icon,  
icon_size which);`

`GetIcon()` and `SetIcon()` get and set the icons that are associated (in the database) with this object's MIME type. You specify which icon you want (large or small) by passing `B_LARGE_ICON` or `B_MINI_ICON` as the `which` argument. The icon is passed in or returned through the `icon` argument. The icon data is copied out of or into the [`BBitmap`](BBitmap.md "BBitmap") object.

The bitmap (if you're calling `SetIcon()`) or icon (if you're calling `GetIcon()`) must be the proper size: 32x32 for the large icon, 16x16 for the small one. Additionally, the bitmap must be in the `B_CMAP8` color space (8-bit color), or the application will crash.

If you want to erase the node's icon, pass `NULL` as the `icon` argument to `SetIcon()`.

Return Code| Description  
---|---  
`B_OK`.| The icon was found or set.  
`B_NO_INIT`.| The `BMimeType` is uninitialized.  
`B_BAD_VALUE`.| The bitmap or icon wasn't the proper size.  
  
####  GetIconForType(), SetIconForType()

`status_t GetIconForType(const char* file_type,  
[`BBitmap`](BBitmap.md "BBitmap")* icon,  
icon_size which) const;``status_t SetIconForType(const char* file_type,  
[`BBitmap`](BBitmap.md "BBitmap")* icon,  
icon_size which);`

These functions get and set the icons that an application that has this object's MIME type as a signature uses to display the given file type. `file_type` must be a valid MIME string.

The icon is passed in or returned through the `icon` argument:

  * If you're getting the icon, the [`BBitmap`](BBitmap.md "BBitmap") must be allocated; the icon data is copied into your [`BBitmap`](BBitmap.md "BBitmap") object.

  * If you're setting the icon, the bitmap must be the proper size: 32x32 for the large icon, 16x16 for the small one. In [`BRect`](BRect.md "BRect") lingo, that's `BRect`(0, 0, 31, 31) and `BRect`(0, 0, 15, 15).

  * If you're setting the icon, the bitmap must be in the `B_CMAP8` color space (8-bit color), or the application will crash.

  * You can remove an icon by passing `NULL` as the icon argument to `SetIconForType()`.

Return Code| Description  
---|---  
`B_OK`| The icon was found or set.  
`B_NO_INIT`.| The `BMimeType` is uninitialized.  
`B_BAD_VALUE`.| (`Get`) `NULL` [`BBitmap`](BBitmap.md "BBitmap") pointer, or `file_type` is invalid.  
`B_BAD_VALUE`.| (`Set`) The bitmap data isn't the proper size, or `file_type` is invalid.  
  
####  GetInstalledTypes(), GetInstalledSupertypes()

`static status_t GetInstalledTypes([`BMessage`](BMessage.md "BMessage")* types);` `static status_t GetInstalledTypes(const char* supertype,  
[`BMessage`](BMessage.md "BMessage")* subtypes);`

`static status_t GetInstalledSupertypes([`BMessage`](BMessage.md "BMessage")* supertypes);`

These static functions retrieve all the file types that are currently installed in the database, all the installed subtypes for a given supertype, and all the installed supertypes. The types are copied into the `types` field of the passed-in [`BMessage`](BMessage.md "BMessage") (which must be allocated).

Return Code| Description  
---|---  
`B_OK`.| The types were found.  
`B_BAD_VALUE`.| The `supertype` string isn't valid.  
`B_NO_MEMORY`.| Insufficient memory to copy the types.  
  
####  GetLongDescription(), SetLongDescription(), GetShortDescription(), SetShortDescription()

`status_t GetLongDescription(char* description) const;``status_t SetLongDescription(const char* description);``status_t GetShortDescription(char* description) const;``status_t SetShortDescription(const char* description);`

Each file type has a couple of human-readable description strings associated with it. Neither description string may be longer than `B_MIME_TYPE_LENGTH` characters.

These functions get and set the long and short description strings. The `Get` functions copy the string into the argument (which must be allocated). The `Set` functions copy the string that the argument points to.

Return Code| Description  
---|---  
`B_OK`.| The description was found or set.  
`B_NO_INIT`.| The `BMimeType` is uninitialized.  
`B_BAD_VALUE`.| (`Set`) `description` is too long.  
`B_NO_MEMORY`.| Insufficient memory to copy the description.  
  
####  GetPreferredApp(), SetPreferredApp()

`status_t GetPreferredApp(char* signature,  
app_verb verb = B_OPEN) const;``status_t SetPreferredApp(const char* signature,  
app_verb verb = B_OPEN);`

These functions get and set the "preferred app" for this object's MIME type. The preferred app is the application that's used to access a file when, for example, the user double-clicks the file in a Tracker window: Unless the file identifies (in its attributes) a "custom" preferred app, the Tracker will ask the File Type database for the preferred app that's associated with the file's type.

  * The preferred app is identified by `signature`, a MIME string. A value of `NULL` indicates that there is no preferred app for the MIME type.

  * The app_verb argument specifies the type of access; currently, the only app_verb is `B_OPEN`.

Return Code| Description  
---|---  
`B_OK`.| The preferred app was found or set.  
`B_NO_INIT`.| The `BMimeType` is uninitialized.  
`B_BAD_VALUE`| (`Set…()` only). The `signature` argument is too long (greater than `B_MIME_TYPE_LENGTH`).  
  
####  GetSupportingApps(), GetWildcardApps()

`status_t GetSupportingApps([`BMessage`](BMessage.md "BMessage")* msg) const;``static status_t GetWildcardApps([`BMessage`](BMessage.md "BMessage")* msg);`

These functions retrieve a list of applications (identified by signature) that know how to handle the object's MIME type (for `GetSupportingApps()`) or all MIME types (`GetWildCardApps()`). The information is returned in `msg`, which must be allocated by the caller. The `msg` format is:

Field name| Type| Description  
---|---|---  
`applications`| `B_STRING_TYPE` (array)| The signatures of the application that know how to handle the MIME type. The first _n_ applications (where _n_ is defined by `be:sub`, below) can handle the full type (supertype _and_ subtype). The rest of the applications in the array handle the supertype only.  
`be:sub`| `B_INT32_TYPE`| The number of applications in the "applications" array that can handle the object's full MIME type. These applications are listed first in the array. This field is omitted if the object represents a supertype only.  
`be:super`| `B_INT32_TYPE`| The number of applications in the `applications` array that can handle the object's supertype (not counting those that can handle the full type). These applications are listed after the full-MIME type supporters. By definition, the `GetWildcardApps()` function never returns supertype-only apps.  
  
For example, here we print the signatures of the apps that can handle "text/plain" and "text" (without checking for errors):
    
    
    BMessage msg();
    BMimeType mime("text/plain");
    int32 subs=0, supers=0, n, hold;
    char *ptr;
    
    mime.GetSupportingApps(&msg);
    msg.FindInt32("be:subs", &subs);
    msg.FindInt32("be:supers", &supers);
    
    for (n = 0; n < subs; n++) {
       msg.FindString("applications", n, &ptr);
       printf("Full support: %sn", ptr);
    }
    hold = n;
    for (n = 0; n < supers; n++) {
       msg.FindString("applications", n+hold, &ptr);
       printf("Supertype support: %sn", ptr);
    }

If an application supports both the full type and the supertype, it will be listed only once in the "applications" array (as a full supporter).

To set the types that an application supports, use [`BAppFileInfo::SetSupportedTypes()`](BAppFileInfo.md#BAppFileInfo_SetSupportedTypes). To tell an app to support all types, add "application/octet-stream" to its supported-types list.

Return Code| Description  
---|---  
`B_OK`.| The signatures were found.  
`B_BAD_PORT`.| No [`be_app`](BApplication.md#BApplication_be_app "be_app") found.  
`B_NO_INIT`.| The `BMimeType` is uninitialized.  
`B_NO_MEMORY`.| Insufficient memory to copy the signatures.  
  
#### InitCheck()

`status_t InitCheck() const;`

Returns the status of the most recent construction or [`SetTo()`](BMimeType.md#BMimeType_SetTo "SetTo\(\), Unset\(\)") call.

See [`SetTo()`](BMimeType.md#BMimeType_SetTo "SetTo\(\), Unset\(\)").

####  Install(), Delete(), IsInstalled()

`status_t Install();``status_t Delete();``bool IsInstalled() const;`

`Install()` adds the object's MIME type to the File Type database. `Delete()` removes the type from the database. `IsInstalled()` tells you if the type is currently installed.

None of these functions affect the object's copy of the MIME type; for instance, deleting a MIME type from the database doesn't uninitialize the object.

Warning

![Warning](./images/admonitions/Stop_32.png)

Currently, `Install()` may return a random value if the object is already installed. To avoid confusion, you should call `IsInstalled()` first:
    
    
    if (!mime.IsInstalled())
       mime.Install();

Return Code| Description  
---|---  
`B_OK`.| The type was successfully added or deleted.  
`B_BAD_VALUE`.| The object is uninitialized.  
  
####  IsValid(), IsSupertypeOnly()

`static bool IsValid(const char* MIME_string);` `bool IsValid() const;`

`bool IsSupertypeOnly() const;`

The static `IsValid()` tests its argument for MIME validity. See "[Valid MIME Strings](BMimeType_Overview.md#BMimeType_Valid_MIME_Strings "Valid MIME Strings")" for the rules. The non-static version checks the validity of the object's MIME string.

`IsSupertypeOnly()` returns `true` if the object's MIME string doesn't include a subtype.

####  SetTo(), Unset()

`status_t SetTo(const char* MIME_string);``void Unset();`

`SetTo()` initializes this `BMimeType` object to represent `MIME_string`. The object's previous MIME string is freed; the argument is then copied. The argument can be a full supertype/subtype string, or simply a supertype. In any case, it must pass the validity test described in "[Valid MIME Strings](BMimeType_Overview.md#BMimeType_Valid_MIME_Strings "Valid MIME Strings")".

`Unset()` frees the object's current MIME string, and sets the object's status to `B_NO_INIT`.

These return codes are also returned by the [`InitCheck()`](BMimeType.md#BMimeType_InitCheck "InitCheck\(\)") function.

Return Code| Description  
---|---  
`B_OK`.| The initialization was successful.  
`B_NO_INIT`.| `MIME_string` is `NULL` or invalid.  
`B_NO_MEMORY`.| Not enough memory to allocate a copy of the argument.  
  
####  StartWatching(), StopWatching()

`static status_t StartWatching([`BMessenger`](BMessenger.md "BMessenger") target);``static status_t StopWatching([`BMessenger`](BMessenger.md "BMessenger") target);`

`StartWatching()` initiates the MIME monitor, which is used for keeping track of changes to the File Types database. Change notifications will be sent via the [`BMessenger`](BMessenger.md "BMessenger") `target` in a [`BMessage`](BMessage.md "BMessage") with the `what` field set to `B_META_MIME_CHANGED`.

Notification messages have the following fields:

Field name| Type| Description  
---|---|---  
`be:which`| int32| Change bitmap (see below for a list)  
`be:type`| string| MIME type whose database information was changed  
`be:extra_type`| string| Extra MIME field used for some notifications  
`be:large_icon`| bool| For notifications involving icon changes, `true` if the large icon was changed; `false` otherwise  
  
`be:which` is a bitmask describing the changes made to the database for MIME type `be:type`. The following masks are defined along with the `BMimeType` methods used to effect the changes they signal:

Constant| Method  
---|---  
`B_ICON_CHANGED`| [`SetIcon()`](BMimeType.md#BMimeType_SetIcon)  
`B_PREFERRED_APP_CHANGED`| [`SetPreferredApp()`](BMimeType.md#BMimeType_SetPreferredApp)  
`B_ATTR_INFO_CHANGED`| [`SetAttrInfo()`](BMimeType.md#BMimeType_SetAttrInfo)  
`B_FILE_EXTENSIONS_CHANGED`| [`SetFileExtensions()`](BMimeType.md#BMimeType_SetFileExtensions)  
`B_SHORT_DESCRIPTION_CHANGED`| [`SetShortDescription()`](BMimeType.md#BMimeType_SetShortDescription)  
`B_LONG_DESCRIPTION_CHANGED`| [`SetLongDescription()`](BMimeType.md#BMimeType_SetLongDescription)  
`B_ICON_FOR_TYPE_CHANGED`| [`SetIconForType()`](BMimeType.md#BMimeType_SetIconForType)  
`B_APP_HINT_CHANGED`| [`SetAppHint()`](BMimeType.md#BMimeType_SetAppHint)  
  
The `BMimeType` methods are given for illustrative purposes only —anything that alters the database for a MIME type will also trigger a notification message. The `be:extra_type` field is used only in the `B_ICON_FOR_TYPE_CHANGED` message and indicates the application signature for which the change is valid.

`StopWatching()` terminates the MIME monitor previously initiated for the given [`BMessenger`](BMessenger.md "BMessenger").

####  Type(), GetSupertype()

`const char* Type() const;``status_t GetSupertype([`BMimeType`](BMimeType.md "BMimeType")* super) const;`

`Type()` returns a pointer to the object's MIME string. If the object isn't initialized, this returns a pointer to `NULL`.

`GetSupertype()` initializes the argument with this object's supertype. (You can then call `Type()` on the argument to see the supertype.) `super` must be allocated before it's passed in. If this object isn't initialized, `super` is uninitialized.

The errors apply to `GetSupertype()` only.

Return Code| Description  
---|---  
`B_OK`.| Everything's fine.  
`B_BAD_VALUE`.| This object isn't initialized.  
  
* * *

### Operators

#### == (comparison)

`bool operator==(const BMimeType& type) const;` `bool operator==(const char* type) const;`

Two MIME types are equal if they are both initialized to the same string (without regard to case).

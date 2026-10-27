<!-- Classes And Methods - The Storage Kit - BNodeInfo -->

## BNodeInfo

| Derived From:| None  
---|---  
Mix-in Classes:| None  
Declared In:| `storage/NodeInfo.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BNodeInfo_Overview.md "BNodeInfo")

  * [Constructor and Destructor](BNodeInfo.md#BNodeInfo_ConstructorDestructor)
  * [Member Functions](BNodeInfo.md#BNodeInfo_Member_Functions)

  
  
* * *

### Constructor and Destructor

#### BNodeInfo()

`BNodeInfo();` `BNodeInfo([`BNode`](BNode.md "BNode")* node);`

The default constructor creates a new, uninitialized `BNodeInfo` object. To initialize you have to follow this construction with a call to [`SetTo()`](BNodeInfo.md#BNodeInfo_SetTo "SetTo\(\)").

The [`BNode`](BNode.md "BNode") version intializes the `BNodeInfo` by passing the argument to [`SetTo()`](BNodeInfo.md#BNodeInfo_SetTo "SetTo\(\)"); see [`SetTo()`](BNodeInfo.md#BNodeInfo_SetTo "SetTo\(\)") for details (and error codes).

#### ~BNodeInfo()

`~BNodeInfo();`

Destroys the object. The [`BNode`](BNode.md "BNode") object that was used to initialize the object isn't touched.

* * *

### Member Functions

####  GetAppHint(), SetAppHint()

`status_t GetAppHint(entry_ref* app_ref) const;``status_t SetAppHint(const entry_ref app_ref);`

These functions get and set the "app hint" for the node. The app hint is an entry_ref that identifies the executable that should be used when launching the node. Of course, the entry_ref may not point to an application, or it might point to an application with the wrong signature (and so on)—that's why this is merely a hint.

`GetAppHint()` function initializes the entry_ref to the hint recorded in the "BEOS:PPATH" attribute of the node; the argument must be allocated before it's passed in.

`SetAppHint()` stores the path corresponding to the entry_ref in the "BEOS:PPATH" attribute of the node. Since the path to the application is stored instead of its entry_ref, the hint will break if the application is subsequently moved.

Return Code| Description  
---|---  
`B_OK`.| The ref was successfully retrieved or set.  
`B_ENTRY_NOT_FOUND`.| The attribute "BEOS:PPATH" doesn't exist. (`Get`)  
`B_NOT_ALLOWED`.| The node is on a read-only volume. (`Set`)  
  
See Also: [`BMimeType::GetAppHint()`](BMimeType.md#BMimeType_GetAppHint "GetAppHint\(\), SetAppHint\(\)")

####  GetIcon(), SetIcon(), GetTrackerIcon()

`virtual status_t GetIcon([`BBitmap`](BBitmap.md "BBitmap")* icon,  
icon_size which = B_LARGE_ICON) const;``virtual status_t SetIcon(const [`BBitmap`](BBitmap.md "BBitmap")* icon,  
icon_size which = B_LARGE_ICON);`

`status_t GetTrackerIcon([`BBitmap`](BBitmap.md "BBitmap")* icon,  
icon_size which = B_LARGE_ICON);` `static status_t GetTrackerIcon(entry_ref* ref,  
[`BBitmap`](BBitmap.md "BBitmap")* icon,  
icon_size which = B_LARGE_ICON);`

`GetIcon()` and `SetIcon()` get and set the icon data that's stored in the node's attributes. You specify which icon you want (large or small) by passing `B_LARGE_ICON` or `B_MINI_ICON` as the `which` argument. The icon is passed in or returned through the `icon` argument. The icon data is copied out of or into the [`BBitmap`](BBitmap.md "BBitmap") object.

The bitmap (if you're calling `SetIcon()`) or icon (for `GetIcon()` and `GetTrackerIcon()`) must be the proper size: 32x32 for the large icon, 16x16 for the small one.

If you want to erase the node's icon, pass `NULL` as the `icon` argument to `SetIcon()`.

Note

![Note](./images/admonitions/Info_32.png)

The icon attributes are stored as "BEOS: L:STD_ICON" (large icon) and "BEOS: M:STD_ICON" (small, or "mini" icon).

`GetTrackerIcon()` finds the icon that the Tracker uses to display the node. The static version lets you identify the node as an entry_ref. Both versions follow the same ordered path in trying to find the icon:

  1. First, it looks in the node's attributes. If the attribute doesn't exist, it…

  2. …gets the node's preferred app (as a signature), and asks the File Type database if that signature declares an icon for this node's file type. If the node doesn't have a preferred app, or if the app doesn't designate an icon for the node's type, the function…

  3. …asks the File Type database for the icon based on the node's file type. If still empty-handed, the function…

  4. …asks the File Type database for the preferred app based on the node's file type, and then asks that app for the icon it uses to display this node's file type. If still nothing, we…

  5. …quit.

The function doesn't tell you which branch of the path it found the icon in.

Return Code| Description  
---|---  
`B_OK`.| The icon was found or set.  
`B_NO_INIT`.| The `BNodeInfo` is uninitialized.  
`B_BAD_VALUE`.| The bitmap or icon wasn't the proper size.  
Attribute errors.| See the error codes for [`BNode::ReadAttr()`](BNode.md#BNode_ReadAttr "ReadAttr\(\), WriteAttr\(\), RemoveAttr\(\)") and [`BNode::WriteAttr()`](BNode.md#BNode_WriteAttr).  
  
####  GetPreferredApp(), SetPreferredApp()

`status_t GetPreferredApp(char* signature,  
app_verb verb = B_OPEN) const;``status_t SetPreferredApp(const char* signature,  
app_verb verb = B_OPEN);`

These functions get and set the node's "preferred app." This is the application that's used to access the node when, for example, the user double-clicks the node in a Tracker window.

  * The preferred app is identified by `signature`, a MIME string.

  * The `app_verb` argument specifies the type of access; currently, the only `app_verb` is `B_OPEN`.

If a node doesn't have a preferred app, the Tracker looks in the File Type database for an app that can open the node's file type.

Note

![Note](./images/admonitions/Info_32.png)

The attribute that stores the preferred app is named "BEOS:PREF_APP".

Return Code| Description  
---|---  
`B_OK`.| The preferred app was found or set.  
`B_NO_INIT`.| The `BNodeInfo` is uninitialized.  
`B_BAD_VALUE`| (`Set…()` only). The `signature` argument is too long (greater than `B_MIME_TYPE_LENGTH`).  
Attribute errors.| See the error codes for [`BNode::ReadAttr()`](BNode.md#BNode_ReadAttr "ReadAttr\(\), WriteAttr\(\), RemoveAttr\(\)") and [`BNode::WriteAttr()`](BNode.md#BNode_WriteAttr).  
  
####  GetType(), SetType()

`virtual status_t GetType(char* type) const;``virtual status_t SetType(const char* type);`

These functions get and set the node's file type. The file type, passed in or returned through `type`, is a MIME string.

Note

![Note](./images/admonitions/Info_32.png)

The attribute that stores the file type is named "BEOS:TYPE".

Return Code| Description  
---|---  
`B_OK`.| The type was found or set.  
`B_NO_INIT`.| The `BNodeInfo` is uninitialized.  
Attribute errors.| See the error codes for [`BNode::ReadAttr()`](BNode.md#BNode_ReadAttr "ReadAttr\(\), WriteAttr\(\), RemoveAttr\(\)") and [`BNode::WriteAttr()`](BNode.md#BNode_WriteAttr).  
  
#### InitCheck()

`status_t InitCheck() const;`

Returns the status of the most recent initialization.

Return Code| Description  
---|---  
`B_OK`.| The object was successfully initialized.  
`B_NO_INIT`.| The `BNodeInfo` is uninitialized.  
  
See [`SetTo()`](BNodeInfo.md#BNodeInfo_SetTo "SetTo\(\)") for more error codes.

#### SetTo()

`status_t SetTo([`BNode`](BNode.md "BNode")* node);`

Initializes the `BNodeInfo` object by pointing it to `node`, which must be a valid (initialized) [`BNode`](BNode.md "BNode") object. The `BNodeInfo` maintains its own [`BNode`](BNode.md "BNode") pointer: You shouldn't delete `node` while the `BNodeInfo` is accessing it; other changes to the [`BNode`](BNode.md "BNode") are permitted, but you may want to avoid such antics. Re-initializing a `BNodeInfo` doesn't affect the previous [`BNode`](BNode.md "BNode") object.

Return Code| Description  
---|---  
`B_OK`.| The object was successfully initialized.  
`B_BAD_VALUE`.| `node` is uninitialized.

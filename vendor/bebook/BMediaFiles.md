<!-- Classes And Methods - The Media Kit - BMediaFiles -->

## BMediaFiles

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `media/MediaFiles.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BMediaFiles_Overview.md "BMediaFiles")

  * [Constructor and Destructor](BMediaFiles.md#BMediaFiles_ConstructorDestructor)
  * [Member Functions](BMediaFiles.md#BMediaFiles_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BMediaFiles()

`BMediaFiles();`

Constructor.

#### ~BMediaFiles()

`virtual ~BMediaFiles()();`

Destructor.

* * *

### Member Functions

#### GetNextRef()

`virtual status_t GetNextRef(BString* outType,  
entry_ref outRef = NULL);`

Returns the next media type in `outType`. These types include things like "sound" and "bitmap."

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_BAD_INDEX`| No more entry_refs to get.  
`B_BAD_VALUE`| `outType` is `NULL`.  
  
#### GetNextType()

`virtual status_t GetNextType(BString* outType) const;`

Returns the next media type in `outType`. These types include things like "sound" and "bitmap."

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_BAD_INDEX`| No more types to get.  
  
####  GetRefFor(), SetRefFor(), RemoveRefFor()

`virtual status_t GetRefFor(const char* type,  
const char* item,  
entry_ref* outRef);``virtual status_t SetRefFor(const char* type,  
const char* item,  
entry_ref& outRef);``virtual status_t RemoveRefFor(const char* type,  
const char* item,  
entry_ref& outRef);`

`GetRefFor()` returns the entry_ref for the media file that's been assigned to the specified `type` and `item` name pair. For example, if you wanted to retrieve the sound file that's been assigned as the system startup sound, you might specify type as "sound" and item as "startup sound."

`SetRefFor()` sets, by entry_ref, the file to be used for the specified `type`/`item` pairing.

`RemoveRefFor()` removes the specified entry_ref for the given `type`/`item` pairing.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_ENTRY_NOT_FOUND`| (`GetRefFor()` only) the specified `type`/`item` pair isn't assigned.  
  
#### RewindRefs()

`virtual status_t RewindRefs(const char* type);`

Rewinds the entry_ref list for the specified media type, so you can start scanning from the beginning of the list.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_BAD_VALUE`| `type` is `NULL`.  
Other errors.| Unable to rewind; the Media Server may not be responding.  
  
#### RemoveItem()

`virtual status_t RemoveItem(const char* type);`

Removes an entrire entry from the media files database. Unlike `RemoveRefFor()`, which removes an entry_ref from an entry, this function actually deletes the entire entry.

Return Code| Description  
---|---  
`B_OK`| No error.  
Other errors.| Unable to remove the item; the Media Server may not be responding.  
  
#### RewindTypes()

`virtual status_t RewindTypes();`

Rewinds to the beginning of the media type list. If you've been using a `BMediaFiles` object and want to begin scanning again from the beginning of the media type list, you should call this function.

Return Code| Description  
---|---  
`B_OK`| No error.  
Other errors.| Unable to rewind; the Media Server may not be responding.

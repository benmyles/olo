<!-- Classes And Methods - The Interface Kit - BBitmap -->

## BBitmap

| Derived From:| –  
---|---  
Mix-in Classes:| [BArchivable](BArchivable.md "BArchivable")  
Declared In:| `interface/Bitmap.h`  
Library:| `libbe.so`  
Allocation:| Constructor only  
[Class Overview](BBitmap_Overview.md "BBitmap")

  * [Constructor and Destructor](BBitmap.md#BBitmap_ConstructorDestructor)
  * [Member Functions](BBitmap.md#BBitmap_MemberFunctions)
  * [Static Functions](BBitmap.md#BBitmap_StaticFunctions)
  * [Archived Fields](BBitmap.md#BBitmap_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BBitmap()

`BBitmap([`BRect`](BRect.md "BRect") bounds,  
color_space space,  
bool acceptsViews = false,  
bool needsContiguousMemory = false);` `BBitmap([`BMessage`](BMessage.md "BMessage")* archive);`

Creates a new `BBitmap` object that can hold a bitmap whose size and depth are described by `bounds` and `space`. The bitmap data is uninitialized; you set the data through [`Bits()`](BBitmap.md#BBitmap_Bits "Bits\(\)") / [`SetBits()`](BBitmap.md#BBitmap_SetBits "SetBits\(\)"), or by drawing into an attached [`BView`](BView.md "BView") (see "[Using a View to Draw into a Bitmap](BBitmap_Overview.md#BBitmap_Overview_Using_A_View "Using a View to Draw into a Bitmap")").

Warning

![Warning](./images/admonitions/Stop_32.png)

The `BBitmap` class insists that a [`BApplication`](BApplication.md "BApplication") object be present (but not necessarily running).

If [`BView`](BView.md "BView")s are to be used, the `acceptsViews` argument must be set to `true`. Furthermore (in this case), the origin of the `bounds` rectangle must be 0.0

If the `needsContiguousMemory` flag is `true`, the `BBitmap` will make sure that the (physical) memory it allocates is one contiguous physical chunk. This should matter only to drivers doing direct DMA into physical memory.

The possible color spaces are enumerated in the section "[Color Spaces](TheInterfaceKit_Drawing.md#TheInterfaceKit_Drawing_Color_Spaces "Color Spaces")".

#### ~BBitmap()

`virtual ~BBitmap();`

Frees all memory allocated to hold image data, deletes any [`BView`](BView.md "BView")s used to create the image, gets rid of the off-screen window that held the views, and severs the `BBitmap`'s connection to the Application Server.

* * *

### Member Functions

#### AddChild()

`virtual void AddChild([`BView`](BView.md "BView")* aView);`

Adds `aView` (and all its children) to this `BBitmap`'s view hierarchy, and causes [`AttachedToWindow()`](BView.md#BView_AttachedToWindow "AttachedToWindow\(\), AllAttached\(\)") to be sent to the newly add children.

Warning

![Warning](./images/admonitions/Stop_32.png)

If `aView` already has a parent, the application may crash. Be sure to remove the view from a previous parent before trying to add it to a bitmap.

`AddChild()` fails if the `BBitmap` was not constructed to accept views.

See also: [`BWindow::AddChild()`](BWindow.md#BWindow_AddChild "AddChild\(\), RemoveChild\(\), ChildAt\(\), CountChildren\(\)"), [`BView::AttachedToWindow()`](BView.md#BView_AttachedToWindow "AttachedToWindow\(\), AllAttached\(\)"), [`RemoveChild()`](BBitmap.md#BBitmap_RemoveChild "RemoveChild"), the `BBitmap` [constructor](BBitmap.md#BBitmap_Constructor "BBitmap\(\)")

#### Archive()

`virtual status_t Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Calls the inherited version of [`Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)") and stores the `BBitmap` in the [`BMessage`](BMessage.md "BMessage") archive.

See also: [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)"), [`Instantiate()`](BBitmap.md#BBitmap_Instantiate "Instantiate\(\)") static function

#### Bits()

`void* Bits() const;`

Returns a pointer to the bitmap data. The length of the data can be obtained by calling [`BitsLength()`](BBitmap.md#BBitmap_BitsLength "BitsLength\(\)")—or it can be calculated from the height of the bitmap (the number of rows) and [`BytesPerRow()`](BBitmap.md#BBitmap_BytesPerRow "BytesPerRow\(\)").

The data is in the format specified by [`ColorSpace()`](BBitmap.md#BBitmap_ColorSpace "ColorSpace\(\)").

This pointer is valid throughout the entire lifespan of the object.

See also: [`Bounds()`](BBitmap.md#BBitmap_Bounds "Bounds\(\)"), [`BytesPerRow()`](BBitmap.md#BBitmap_BytesPerRow "BytesPerRow\(\)"), [`BitsLength()`](BBitmap.md#BBitmap_BitsLength "BitsLength\(\)")

#### BitsLength()

`int32 BitsLength() const;`

Returns the number of bytes that were allocated to store the bitmap data.

See also: [`Bits()`](BBitmap.md#BBitmap_Bits "Bits\(\)"), [`BytesPerRow()`](BBitmap.md#BBitmap_BytesPerRow "BytesPerRow\(\)")

#### Bounds()

`[`BRect`](BRect.md "BRect") Bounds() const;`

Returns the bounds rectangle that defines the size and coordinate system of the bitmap. This should be identical to the rectangle used in constructing the object.

#### BytesPerRow()

`int32 BytesPerRow() const;`

Returns how many bytes of data are required to specify a row of pixels. This may include slop space required by the graphics hardware; you should always use this call to determine the width of a row of pixels in bytes instead of assuming that it will be the number of pixels multiplied by the size of a pixel in bytes.

####  ChildAt(), CountChildren()

`[`BView`](BView.md "BView")* ChildAt(int32 index) const;``int32 CountChildren() const;`

`ChildAt()` returns the child [`BView`](BView.md "BView") at `index`, or `NULL` if there's no child at `index`. Indices begin at 0 and count only [`BView`](BView.md "BView")s that were added to the `BBitmap` (added as children of the top view of the `BBitmap`'s off-screen window) and not subsequently removed.

`CountChildren()` returns the number of [`BView`](BView.md "BView")s the `BBitmap` currently has. (It counts only [`BView`](BView.md "BView")s that were added directly to the `BBitmap`, not [`BView`](BView.md "BView")s farther down the view hierarchy.)

These functions fail if the `BBitmap` wasn't constructed to accept views.

#### ColorSpace()

`color_space ColorSpace() const;`

Returns the color space of the data being stored (not necessarily the color space of the data passed to the [`SetBits()`](BBitmap.md#BBitmap_SetBits "SetBits\(\)") function). Once set by the `BBitmap` constructor, the color space doesn't change.

#### FindView()

`[`BView`](BView.md "BView")* FindView([`BPoint`](BPoint.md "BPoint") point) const;` `[`BView`](BView.md "BView")* FindView(const char* name) const;`

Returns the [`BView`](BView.md "BView") at `point` within the bitmap or the [`BView`](BView.md "BView") tagged with `name`. The point must be somewhere within the `BBitmap`'s bounds rectangle, which must have the coordinate origin, (0.0, 0.0), at its left top corner.

If the `BBitmap` doesn't accept views, this function fails. If no view draws at the `point` given, or no view associated with the `BBitmap` has the `name` given, it returns `NULL`.

#### IsValid()

`bool IsValid() const;`

Returns `true` if there's memory for the bitmap (if the address returned by [`Bits()`](BBitmap.md#BBitmap_Bits "Bits\(\)") is valid), and `false` if not.

####  Lock(), Unlock(), IsLocked()

`bool Lock();``void Unlock();``bool IsLocked() const;`

These functions lock and unlock the off-screen window where [`BView`](BView.md "BView")s associated with the `BBitmap` draw. Locking works for this window and its views just as it does for ordinary on-screen windows.

`Lock()` returns `false` if the `BBitmap` doesn't accept views or if its off-screen window is unlockable (and therefore unusable) for some reason. Otherwise, it doesn't return until it has the window locked and can return `true`.

`IsLocked()` returns `false` if the `BBitmap` doesn't accept views. Otherwise, it returns the lock status of its off-screen window.

#### RemoveChild

`virtual bool RemoveChild([`BView`](BView.md "BView")* aView);`

Removes `aView` from the hierarchy of views associated with the `BBitmap`, but only if `aView` was added to the hierarchy by calling `BBitmap`'s version of the [`AddChild()`](BView.md#BView_AddChild "AddChild\(\), RemoveChild\(\)") function.

If `aView` is successfully removed, `RemoveChild()` returns `true`. If not, it returns `false`.

#### SetBits()

`void SetBits(const void* data,  
int32 length,  
int32 offset,  
color_space mode);`

Assigns `length` bytes of `data` to the `BBitmap` object. The new data is copied into the bitmap beginning `offset` bytes (not pixels) from the start of allocated memory. To set data beginning with the first (left top) pixel in the image, the `offset` should be 0; to set data beginning with, for example, the sixth pixel in the first row of a `B_RGB32` image, the offset should be 20. The offset counts any padding required to align rows of data.

This function is intended to be used for importing existing data from a different format rather than for setting individual pixels in the bitmap. If you're interested in coloring individual pixels, use [`Bits()`](BBitmap.md#BBitmap_Bits "Bits\(\)") to obtain direct access to the bitmap data.

The source data is specified in the `mode` color space, which may or may not be the same as the color space that the `BBitmap` uses to store the data. If not, the following conversions are automatically made:

  * `B_GRAY1` and `B_RGB32` to `B_CMAP8`.

  * `B_CMAP8` and `B_GRAY1` to `B_RGB32`.

Note

![Note](./images/admonitions/Info_32.png)

These are the only color conversions `SetBits()` understands; all other conversions must be performed manually.

Colors may be dithered in a conversion to `B_CMAP8` so that the resulting image will match the original as closely as possible, despite the lost information.

If the color space `mode` is `B_RGB32`, the `data` should be triplets of three 8-bit components—red, green, and blue, in that order—without an alpha component. Although stored as 32-bit quantities with the components in BGRA order, the input data is only 24 bits in RGB order. Rows of source data do not need to be aligned.

However, if the source data is in any `mode` other than `B_RGB32`, padding must be added so that each row is aligned on a int32 word boundary.

Warning

![Warning](./images/admonitions/Stop_32.png)

`SetBits()` works only on `BBitmap`s in `B_GRAY1`, `B_CMAP8`, and `B_RGB32` color spaces; all other conversions must be carried out manually.

This function works for all `BBitmap`s, whether or not [`BView`](BView.md "BView")s are also enlisted to produce the image.

* * *

### Static Functions

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Returns a new `BBitmap` object—or `NULL`, if the `archive` message doesn't contain data for a `BBitmap` object. The new object is allocated by new and created with the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive.

See also: [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"), [`Archive()`](BBitmap.md#BBitmap_Archive "Archive\(\)")

* * *

### Archived Fields

The [`Archive()`](BBitmap.md#BBitmap_Archive "Archive\(\)") function adds the following fields to its [`BMessage`](BMessage.md "BMessage") argument:

Field| Type code| Description  
---|---|---  
`_frame`| `B_RECT_TYPE`| The `BBitmap`'s bounds rectangle.  
`_cspace`| `B_INT32_TYPE`| The color_space of the data.  
`_view_ok`| `B_BOOL_TYPE`| Always `true`, indicating the `BBitmap` accepts views (only present in deep copy archives of `BBitmap`s accepting views).  
`_data`| `B_RAW_TYPE`| The bitmap data (present only if `_view_ok` not present).  
`_continguous`| `B_BOOL_TYPE`| Whether the `BBitmap` requires memory in one contiguous chunk.  
  
If the `_view_ok` field is present, the child views of the BBitmap are additionally archived in the `_views` array of [`BMessages`](BMessage.md "BMessage"). See the description of the [`BView`](BView.md "BView") [Archived Fields](BView.md#BView_ArchivedFields "Archived Fields") for more information on those fields.

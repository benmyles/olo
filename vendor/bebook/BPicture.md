<!-- Classes And Methods - The Interface Kit - BPicture -->

## BPicture

| Derived From:| –  
---|---  
Mix-in Classes:| [`BArchivable`](BArchivable.md "BArchivable")  
Declared In:| `interface/Picture.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BPicture_Overview.md "BPicture")

  * [Constructor and Destructor](BPicture.md#BPicture_ConstructorDestructor)
  * [Member Functions](BPicture.md#BPicture_MemberFunctions)
  * [Static Functions](BPicture.md#BPicture_StaticFunctions)
  * [Archived Fields](BPicture.md#BPicture_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BPicture()

`BPicture();` `BPicture(const BPicture& picture);` `BPicture([`BMessage`](BMessage.md "BMessage")* archive);`

Initializes the `BPicture` object by ensuring that it's empty, or by copying data from another picture or archive of a `BPicture` object.

#### ~BPicture()

`virtual ~BPicture();`

Destroys the Application Server's record of the `BPicture` object and deletes all its picture data.

* * *

### Member Functions

#### Archive()

`virtual status_t Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Archives the `BPicture` by recording its current settings in the [`BMessage`](BMessage.md "BMessage") `archive`.

See also: [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)"), [`Instantiate()`](BPicture.md#BPicture_Instantiate "Instantiate\(\)") static function

#### Play()

`virtual status_t* Play(void** callBackTable,  
int32 tableEntries,  
void* user);`

Plays back a picture using a user's rendering functions. The functions are passed in `callBackTable`, an array of function pointers. `tableEntries` contains the number of functions in the table. The functions perform various tasks such as drawing lines and text. `user` is passed to each function, providing a hook for passing additional data to the call back functions. The functions, along with their positions in the call back table, are detailed below.

Index| Function prototype  
---|---  
0| no operation  
1| `MovePenBy`(void* `user`, [`BPoint`](BPoint.md "BPoint") delta)  
2| `StrokeLine`(void* `user`, [`BPoint`](BPoint.md "BPoint") `start`, [`BPoint`](BPoint.md "BPoint") `end`)  
3| `StrokeRect`(void* `user`, BRect `rect`)  
4| `FillRect`(void* `user`, BRect `rect`)  
5| `StrokeRoundRect`(void* `user`, BRect `rect`, [`BPoint`](BPoint.md "BPoint") `radii`)  
6| `FillRoundRect`(void* `user`, BRect `rect`, [`BPoint`](BPoint.md "BPoint") `radii`)  
7| `StrokeBezier`(void* `user`, [`BPoint`](BPoint.md "BPoint")* `control`)  
8| `FillBezier`(void* `user`, [`BPoint`](BPoint.md "BPoint")* `control`)  
9| `StrokeArc`(void* `user`, [`BPoint`](BPoint.md "BPoint") `center`, [`BPoint`](BPoint.md "BPoint") `radii`, float `startTheta`, float `arcTheta`)  
10| `FillArc`(void* `user`, [`BPoint`](BPoint.md "BPoint") `center`, [`BPoint`](BPoint.md "BPoint") `radii`, float `startTheta`, float `arcTheta`)  
11| `StrokeEllipse`(void* `user`, [`BPoint`](BPoint.md "BPoint") `center`, [`BPoint`](BPoint.md "BPoint") `radii`)  
12| `FillEllipse`(void* `user`, [`BPoint`](BPoint.md "BPoint") `center`, [`BPoint`](BPoint.md "BPoint") `radii`)  
13| `StrokePolygon`(void* `user`, int32 `numPoints`, [`BPoint`](BPoint.md "BPoint")* `points`, bool `isClosed`)  
14| `FillPolygon`(void* `user`, int32 `numPoints`, [`BPoint`](BPoint.md "BPoint")* `points`, bool `isClosed`)  
15| Reserved  
16| Reserved  
17| `DrawString`(void* `user`, char* `string`, float `deltax`, float `deltay`)  
18| `DrawPixels`(void* `user`, BRect `src`, BRect `dest`, int32 `width`, int32 `height`, int32 `bytesPerRow`, int32 `pixelFormat`, int32 `flags`, void* `data`)  
19| Reserved  
20| `SetClippingRects`(void* `user`, BRect* `rects`, uint32 `numRects`)  
21| Reserved  
22| `PushState`(void* `user`)  
23| `PopState`(void* `user`)  
24| `EnterStateChange`(void* `user`)  
25| `ExitStateChange`(void* `user`)  
26| `EnterFontState`(void* `user`)  
27| `ExitFontState`(void* `user`)  
28| `SetOrigin`(void* `user`, [`BPoint`](BPoint.md "BPoint") `pt`)  
29| `SetPenLocation`(void* `user`, [`BPoint`](BPoint.md "BPoint") `pt`)  
30| `SetDrawingMode`(void* `user`, drawing_mode `mode`)  
31| `SetLineMode`(void* `user`, cap_mode `capMode`, join_mode `joinMode`, float `miterLimit`)  
32| `SetPenSize`(void* `user`, float `size`)  
33| `SetForeColor`(void* `user`, rgb_color `color`)  
34| `SetBackColor`(void* `user`, rgb_color `color`)  
35| `SetStipplePattern`(void* `user`, pattern `p`)  
36| `SetScale`(void* `user`, float `scale`)  
37| `SetFontFamily`(void* `user`, char* `family`)  
38| `SetFontStyle`(void* `user`, char* `style`)  
39| `SetFontSpacing`(void* `user`, int32 `spacing`)  
40| `SetFontSize`(void* `user`, float `size`)  
41| `SetFontRotate`(void* `user`, float `rotation`)  
42| `SetFontEncoding`(void* `user`, int32 `encoding`)  
43| `SetFontFlags`(void* `user`, int32 `flags`)  
44| `SetFontShear`(void* `user`, float `shear`)  
45| Reserved  
46| `SetFontFace`(void* `user`, int32 `flags`)  
  
While many of these functions are similar to those found in [`BView`](BView.md "BView"), there are some important differences:

  * The return value of the functions is ignored.

  * The `Fill…()` and `Stroke…()` functions do not explicitly specify patterns. Instead, they should be drawn in the current pattern, as set by the `SetStipplePattern()` (callback #35). Note that there is no equivalent to `SetStipplePattern()` in [`BView`](BView.md "BView").

  * The `deltax` and `deltay` arguments passed to `DrawString()` are escapement delta values; the string should be drawn at the current pen position.

  * `MovePenBy()` uses a [`BPoint`](BPoint.md "BPoint") to specify the amount to move the pen. The x component of the [`BPoint`](BPoint.md "BPoint") gives the x offset and the y component the y offset.

  * Similarly, `…RoundRect()` and `…Ellipse()` use a [`BPoint`](BPoint.md "BPoint") to specify the two separate radius components. The `x` component gives the x radius and the `y` component the y radius.

  * `DrawPixels()` is a `BPicture`-specific primitive for rendering bitmaps. It is a request to copy the `src` rectangle from the raw color information in `data` to the `dest` rectangle of the current rendering area. `width`, `height`, `bytesPerRow`, and `pixelFormat` provide all the information necessary to interpret data. `flags` is currently always zero and should be ignored. `src` and `dest` need not have the same dimensions; in these cases, the function should scale the bitmap appropriately.

  * `SetClippingRects()` is a `BPicture`-specific primitive approximating `ConstrainClippingRegion()`. It instructs the renderer to replace the current clipping region with the union of the rectangles passed to SetClippingRects().

  * Changes in the graphics state are sandwiched between calls to `EnterStateChange()` and `ExitStateChange()`. State change functions will only be called between these functions. No other call backs will be called between these functions. State change functions are all `Set…()` functions in addition to `EnterFontState()` and `ExitFontState()`.

  * Similarly, changes to the font state are sandwiched between calls to `EnterFontState()` and `ExitFontState()`. Font state change functions will only be called between these functions. No other call backs will be called between these functions. Font state change functions are all `SetFont…()` functions. Many of the font state functions are found in BFont rather than [`BView`](BView.md "BView").

  * `SetFontRotate()` sets the rotation of the font. Unlike the [`BFont::SetRotation()`](BFont.md#BFont_SetRotation "SetRotation\(\) , Rotation\(\)") function, the angle here is specified in radians, rather than in degrees. You can convert the value into degrees by using the forumla: 
        
        degrees = (rotation*180.0) / 3.14159265369);

* * *

### Static Functions

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Returns a new `BPicture` object, allocated by new and created with the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive. However, if the archive message doesn't contain data for a `BPicture` object, this function returns `NULL`.

See also: [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"), [`Archive()`](BPicture.md#BPicture_Archive "Archive\(\)")

* * *

### Archived Fields

The [`Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)") function adds the following fields to its [`BMessage`](BMessage.md "BMessage") argument:

Field| Type code| Description  
---|---|---  
`_ver`| `B_INT32_TYPE`| Always 1.  
`_endian`| `B_INT8_TYPE`| Endianness of the data. Always `B_HOST_IS_BENDIAN`.  
`_data`| `B_RAW_TYPE`| The `BPicture` data.

<!-- Classes And Methods - The Interface Kit - BShape -->

## BShape

| Derived From:| –  
---|---  
Mix-in Classes:| [`BArchivable`](BArchivable.md "BArchivable")  
Declared In:| `interface/Shape.h`  
Library:| `libbe.so`  
Allocation:| Constructor only  
[Class Overview](BShape_Overview.md "BShape")

  * [Constructor and Destructor](BShape.md#BShape_ConstructorDestructor)
  * [Member Functions](BShape.md#BShape_MemberFunctions)
  * [Static Functions](BShape.md#BShape_StaticFunctions)
  * [Archived Fields](BShape.md#BShape_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BShape()

`BShape();` `BShape(const BShape& copyFrom);` `BShape([`BMessage`](BMessage.md "BMessage")* archive);`

Creates a new `BShape` object. The first form of the `BShape` constructor creates an empty shape. The second form, which accepts another `BShape` as an argument, copies that shape into the new one. The third form reconstructs a `BShape` from an archive.

#### ~BShape()

`virtual ~BShape();`

Releases memory occupied by the `BShape` object.

* * *

### Member Functions

#### AddShape()

`status_t AddShape(const BShape* otherShape);`

`AddShape()` adds the lines and curves that comprise the `otherShape` to the shape.

Return Code| Description  
---|---  
`B_OK`.| No error.  
Other errors.| None are defined at this time, but you should always check for errors returned by this function.  
  
#### Archive()

`virtual Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Stores the `BShape` in the [`BMessage`](BMessage.md "BMessage") archive.

See also: [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)"), [`Instantiate()`](BShape.md#BShape_Instantiate "Instantiate\(\)") static function

#### BezierTo()

`status_t BezierTo([`BPoint`](BPoint.md "BPoint") controlPoints[3]);`

Adds a command to the `BShape` that represents a Beziér curve that begins at the current coordinates and is constructed using the specified control points.

Return Code| Description  
---|---  
`B_OK`.| No error.  
Other errors.| None are defined at this time, but you should always check for errors returned by this function.  
  
#### Bounds()

`[`BRect`](BRect.md "BRect") Bounds() const;`

`Bounds()` returns the bounds rectangle of the entire shape; this rectangle encloses all the lines and Beziér curves that comprise the shape.

#### Clear()

`void Clear();`

`Clear()` deletes all the lines and Beziér curves that comprise the shape, leaving it empty.

#### Close()

`void Close();`

`Close()` should be called when the shape has been fully-constructed by calls to the [`BezierTo()`](BShape.md#BShape_BezierTo "BezierTo\(\)"), [`LineTo()`](BShape.md#BShape_LineTo "LineTo\(\)"), and [`MoveTo()`](BShape.md#BShape_MoveTo "MoveTo\(\)") functions.

Return Code| Description  
---|---  
`B_OK`.| No error.  
Other errors.| None are defined at this time, but you should always check for errors returned by this function.  
  
#### LineTo()

`status_t LineTo([`BPoint`](BPoint.md "BPoint") point);`

Adds a "draw line" command to the `BShape`. A line will be drawn from the previous coordinates to the specified coordinates.

Return Code| Description  
---|---  
`B_OK`.| No error.  
Other errors.| None are defined at this time, but you should always check for errors returned by this function.  
  
#### MoveTo()

`status_t MoveTo([`BPoint`](BPoint.md "BPoint") point);`

Adds a "move to" command to the `BShape`. The next [`LineTo()`](BShape.md#BShape_LineTo "LineTo\(\)") or [`BezierTo()`](BShape.md#BShape_BezierTo "BezierTo\(\)") will begin at this point. This lets you create noncontiguous shapes.

Return Code| Description  
---|---  
`B_OK`.| No error.  
Other errors.| None are defined at this time, but you should always check for errors returned by this function.  
  
* * *

### Static Functions

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Returns a new `BShape` object, allocated by new and created with the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive. However, if the archive message doesn't contain data for a `BShape` object, the return value will be `NULL`.

[`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"), [`Archive()`](BShape.md#BShape_Archive "Archive\(\)")

* * *

### Archived Fields

The [`Archive()`](BShape.md#BShape_Archive "Archive\(\)") function adds the following fields to its [`BMessage`](BMessage.md "BMessage") argument:

Field| Type code| Description  
---|---|---  
`pts`| `B_POINT_TYPE`| The list of points used by the shape's commands.  
`ops`| `B_INT32_TYPE`| The list of command opcodes indicating the shape's commands.

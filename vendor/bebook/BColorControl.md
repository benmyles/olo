<!-- Classes And Methods - The Interface Kit - BColorControl -->

## BColorControl

| Derived From:| [BControl](BControl.md "BControl") > [BView](BView.md "BView") > [BHandler](BHandler.md "BHandler")  
---|---  
Mix-in Classes:| [`BArchivable`](BArchivable.md "BArchivable"), [`BInvoker`](BInvoker.md "BInvoker")  
Declared In:| `interface/ColorControl.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BColorControl_Overview.md "BColorControl")

  * [Constructor and Destructor](BColorControl.md#BColorControl_ConstructorDestructor)
  * [Hook Functions](BColorControl.md#BColorControl_HookFunctions)
  * [Member Functions](BColorControl.md#BColorControl_MemberFunctions)
  * [Static Functions](BColorControl.md#BColorControl_StaticFunctions)
  * [Defined Types](BColorControl.md#BColorControl_DefinedTypes)
  * [Archived Fields](BColorControl.md#BColorControl_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BColorControl()

`BColorControl([`BPoint`](BPoint.md "BPoint") leftTop,  
color_control_layout matrix,  
float cellSide,  
const char* name,  
[`BMessage`](BMessage.md "BMessage")* message = NULL,  
bool bufferedDrawing = false);` `BColorControl([`BMessage`](BMessage.md "BMessage")* archive = NULL);`

Initializes the `BColorControl` so that the left top corner of its frame rectangle will be located at the stated `leftTop` point in the coordinate system of its parent view. The frame rectangle will be large enough to display 256 color cells arranged in the specified `matrix`, which can be any of the following [color_control_layout](BColorControl.md#BColorControl_color_control_layout "color_control_layout") constants:

  * `B_CELLS_4x64`
  * `B_CELLS_8x32`
  * `B_CELLS_16x16`
  * `B_CELLS_64x4`
  * `B_CELLS_32x8`

For example, `B_CELLS_4x64` lays out a matrix with four cell columns and 64 rows; `B_CELLS_32x8` specifies 32 columns and 8 rows. Each cell is a square `cellSide` coordinate units on a side; since the number of units translates directly to screen pixels, `cellSide` should be a whole number.

When the screen is 15, 16, or 32 bits deep, the same frame rectangle will display four color ramps, one each for the red, green, and blue components, plus a disabled ramp for the alpha component. You might choose `matrix` and `cellSize` values with a view toward how the resulting bounds rectangle would be divided into four horizontal rows.

The `name` argument assigns a name to the object as a [`BHandler`](BHandler.md "BHandler"). It's the same as the argument declared by the [`BView`](BView.md "BView") constructor.

If a model `message` is supplied, the `BColorControl` will announce every change in color value by calling [`Invoke()`](BControl.md#BControl_Invoke "Invoke\(\)") (defined in the [`BControl`](BControl.md "BControl") class) to post a copy of the message to a designated target.

If the `bufferedDrawing` flag is `true`, all changes to the on-screen display will first be made in an off-screen bitmap and then copied to the screen. This makes the drawing smoother, but it requires more memory.

The initial value of the new object is 0, which when translated to an rgb_color structure, means black.

See also: [`BHandler::SetName()`](BHandler.md#BHandler_SetName "SetName\(\), Name\(\)")

#### ~BColorControl()

`virtual ~BColorControl();`

Gets rid of the off-screen bitmap, if one was requested when the object was constructed.

* * *

### Hook Functions

#### AttachedToWindow()

`virtual void AttachedToWindow();`

Augments the [`BControl`](BControl.md "BControl") [version](BControl.md#BControl_AttachedToWindow "AttachedToWindow\(\)") of this function to set the `BColorControl`'s view color and low color to be the same as its parent's view color and to set up the [`BTextControl`](BTextControl.md "BTextControl") objects where the user can type red, green, and blue color values. If the object uses buffered drawing, this function makes sure the offscreen images are displayed on-screen.

See also: [`BView::SetViewColor()`](BView.md#BView_SetViewColor "SetViewColor\(\), ViewColor\(\)")

#### Draw()

`virtual void Draw([`BRect`](BRect.md "BRect") updateRect);`

Overrides the [`BView`](BView.md "BView") [version](BView.md#BView_Draw "Draw\(\)") of this function to draw the color control.

#### GetPreferredSize()

`virtual void GetPreferredSize(float* width,  
float* height);`

Calculates how large the color control needs to be given its layout, cell size, and current font; the results are reported in the variables that the `width` and `height` arguments refer to.

See also: [`BView::GetPreferredSize()`](BView.md#BView_GetPreferredSize "GetPreferredSize\(\), ResizeToPreferred\(\)")

#### KeyDown()

`virtual void KeyDown(const char* bytes,  
int32 numBytes);`

Augments the [`BControl`](BControl.md "BControl") version of [`KeyDown()`](BControl.md#BControl_KeyDown "KeyDown\(\)") to allow the user to navigate within the color control using the arrow keys.

#### MessageReceived()

`virtual void MessageReceived([`BMessage`](BMessage.md "BMessage") message);`

Responds to internal messages that change the color.

See also: [`BHandler::MessageReceived()`](BHandler.md#BHandler_MessageReceived "MessageReceived\(\)")

#### MouseDown()

`virtual void MouseDown([`BPoint`](BPoint.md "BPoint") point);`

Overrides the [`BView`](BView.md "BView") version of [`MouseDown()`](BView.md#BView_MouseDown "MouseDown\(\)") to allow the user to operate the color control with the mouse.

* * *

### Member Functions

#### Archive()

`virtual status_t Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Calls the inherited version of [`Archive()`](BControl.md#BControl_Archive "Archive\(\)") , then adds the layout, cell size, and whether the object uses buffered drawing to the [`BMessage`](BMessage.md "BMessage") archive.

See also: [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)"), [`Instantiate()`](BColorControl.md#BColorControl_Instantiate "Instantiate\(\)") static function

####  SetCellSize(), CellSize()

`virtual void SetCellSize(float cellSide);``float CellSize() const;`

These functions set and return the size of a single cell in the `BColorControl`'s matrix of 256 colors. A cell is a square `cellSide` coordinate units on a side. The size is first set by the `BColorControl` [constructor](BColorControl.md#BColorControl_Constructor "BColorControl\(\)").

#### SetEnabled()

`virtual void SetEnabled(bool enabled);`

Auguments the [`BControl`](BControl.md "BControl") version of [`SetEnabled()`](BControl.md#BControl_SetEnabled "SetEnabled\(\), IsEnabled\(\)") to disable and re-enable the text fields for setting the color components as the `BColorControl` is disabled and re-enabled. The inherited [`IsEnabled()`](BControl.md#BControl_IsEnabled) function doesn't need augmenting and therefore isn't reimplemented.

####  SetLayout(), Layout()

`virtual void SetLayout(color_control_layout layout);``color_control_layout Layout() const;`

These functions set and return the layout of the matrix of 256 color cells. The matrix is first arranged by the constructor. See the [constructor](BColorControl.md#BColorControl_Constructor "BColorControl\(\)") for permissible layout values.

####  SetValue(), ValueAsColor()

`virtual void SetValue(int32 color);` `inline void SetValue(rgb_color color);`

`rgb_color ValueAsColor();`

These functions set and return the `BColorControl`'s current value—the last color that the user selected.

The virtual version of `SetValue()` takes an int32 argument and is essentially the same as the [`BControl`](BControl.md "BControl") version of the function, which it modifies only to take care of class-internal housekeeping details. The inline version, on the other hand, takes an rgb_color argument and is unique to this class. It packs color information from the structure into a 32-bit integer and passes it to the virtual version of the function. Like all other objects that derive from [`BControl`](BControl.md "BControl"), a `BColorControl` stores its current value as an int32; no information is lost in the translation from an rgb_color structure to an integer.

`SetValue()` is called to make every change to the [`BControl`](BControl.md "BControl")'s value. If you override this function to be notified of the changes, you should override the virtual version. (However, due to the peculiarities of C++, overriding any version of an overloaded function hides all versions of the function. For continued access to the rgb_color version of `SetValue()` without explicitly specifying the "`BColorControl`::" prefix, copy the inline code from `interface/ColorControl.h` to the derived class.)

`ValueAsColor()` is an alternative to the [`Value()`](BControl.md#BControl_Value) function inherited from the [`BControl`](BControl.md "BControl") class. It returns the object's current value as an rgb_color; `Value()` returns it as an int32.

See also: [`BControl::SetValue()`](BControl.md#BControl_SetValue "SetValue\(\), Value\(\)")

* * *

### Static Functions

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive = NULL);`

Returns a new `BColorControl` object, allocated by new and created with the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive. However, if the archive doesn't contain data for a `BColorControl` object, this function returns `NULL`.

See also: [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"), [`Archive()`](BColorControl.md#BColorControl_Archive "Archive\(\)")

* * *

### Defined Types

#### color_control_layout
    
    
    enum color_control_layout {}

Determines the layout of the `BColorControl`

* * *

### Archived Fields

The [`Archive()`](BColorControl.md#BColorControl_Archive "Archive\(\)") function adds the following fields to its [`BMessage`](BMessage.md "BMessage") argument:

Field| Type code| Description  
---|---|---  
`_layout`| `B_INT32_TYPE`| The layout of the matrix of 256 color cells.  
`_csize`| `B_FLOAT_TYPE`| Size of a single cell in the matrix of 256 color cells.  
`_use_off`| `B_BOOL_TYPE`| Whether the control uses buffered drawing.  
  
The following views are added to the "_views" field (deep copy only):

Name| Level| Description  
---|---|---  
`_red`| 0| [`BTextControl`](BTextControl.md "BTextControl") for the `red` field.  
`_green`| 0| [`BTextControl`](BTextControl.md "BTextControl") for the `green` field.  
`_blue`| 0| [`BTextControl`](BTextControl.md "BTextControl") for the `blue` field.

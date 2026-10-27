<!-- Classes And Methods - The Interface Kit - BMenuField -->

## BMenuField

| Derived From:| [`BView`](BView.md "BView")  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/MenuField.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BMenuField_Overview.md "BMenuField")

  * [Constructor and Destructor](BMenuField.md#BMenuField_ConstructorDestructor)
  * [Hook Functions](BMenuField.md#BMenuField_HookFunctions)
  * [Member Functions](BMenuField.md#BMenuField_MemberFunctions)
  * [Static Functions](BMenuField.md#BMenuField_StaticFunctions)
  * [Archived Fields](BMenuField.md#BMenuField_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BMenuField

`BMenuField([`BRect`](BRect.md "BRect") frame,  
const char* name,  
const char* label,  
[`BMenu`](BMenu.md "BMenu")* menu,  
uint32 resizingMode = B_FOLLOW_LEFT | B_FOLLOW_TOP,  
uint32 flags = B_WILL_DRAW | B_NAVIGABLE);` `BMenuField([`BRect`](BRect.md "BRect") frame,  
const char* name,  
const char* label,  
[`BMenu`](BMenu.md "BMenu")* menu,  
bool fixedSize,  
uint32 resizingMode = B_FOLLOW_LEFT | B_FOLLOW_TOP,  
uint32 flags = B_WILL_DRAW | B_NAVIGABLE);` `BMenuField([`BMessage`](BMessage.md "BMessage")* archive);`

Initializes the `BMenuField` object with the specified `frame` rectangle, `name`, `resizingMode`, and `flags`. These arguments are the same as for any [`BView`](BView.md "BView") object and are passed unchanged to the [`BView`](BView.md "BView") constructor. When the object is attached to a window, the height of its frame rectangle will be adjusted to fit the height of the text it displays, which depends on the user's preferred font for menus.

By default, the `frame` rectangle is divided horizontally in half, with the label displayed on the left and the menu on the right. This division can be changed with the [`SetDivider()`](BMenuField.md#BMenuField_SetDivider "SetDivider\(\), Divider\(\)") function. The menu is assigned to a [`BMenuBar`](BMenuBar.md "BMenuBar") object and will pop up under the user's control. For most uses, the menu should be a [`BPopUpMenu`](BPopUpMenu.md "BPopUpMenu") object.

The second form of the constructor accepts an added argument, `fixedSize`. If this is true, the `BMenuField` won't adjust its size based on the width of the items in the menu.

#### ~BMenuField

`virtual ~BMenuField();`

Frees the label, the [`BMenuBar`](BMenuBar.md "BMenuBar") object, and other memory allocated by the `BMenuField`.

* * *

### Hook Functions

####  AttachedToWindow(), AllAttached()

`virtual void AttachedToWindow();``virtual void AllAttached();`

These functions override their [`BView`](BView.md "BView") counterparts to make the `BMenuField`'s background color match the color of its parent view and to adjust the height of the view to the height of the [`BMenuBar`](BMenuBar.md "BMenuBar") child it contains. The height of the child depends on the size of the user's preferred font for menus.

See also: [`BView::AttachedToWindow()`](BView.md#BView_AttachedToWindow "AttachedToWindow\(\), AllAttached\(\)")

#### Draw()

`virtual void Draw([`BRect`](BRect.md "BRect") updateRect);`

Overrides the [`BView`](BView.md "BView") version of this function to draw the view's border and label. The way the menu field is drawn depends on whether it's enabled or disabled and whether or not it's the current focus for keyboard actions.

See also: [`BView::Draw()`](BView.md#BView_Draw "Draw\(\)")

#### KeyDown()

`virtual void KeyDown(const char* bytes,  
int32 numBytes);`

Augments the [`BView`](BView.md "BView") version of [`KeyDown()`](BView.md#BView_KeyDown "KeyDown\(\)") to permit keyboard navigation to and from the view and to allow users to open the menu by pressing the space bar.

#### MouseDown()

`virtual void MouseDown([`BPoint`](BPoint.md "BPoint") point);`

Overrides the [`BView`](BView.md "BView") version of [`MouseDown()`](BView.md#BView_MouseDown "MouseDown\(\)") to enable users to pop up the menu using the mouse, even if the cursor isn't directly over the menu portion of the bounds rectangle.

#### WindowActivated()

`virtual void WindowActivated(bool active);`

Makes sure that the `BMenuField` is redrawn when the window is activated and deactivated, provided that it's the current focus view.

See also: [`BView::WindowActivated()`](BView.md#BView_WindowActivated "WindowActivated\(\)")

* * *

### Member Functions

#### Archive()

`virtual status_t Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Calls the inherited version of `Archive()`, which will, in the normal course of things, archive the child [`BMenuBar`](BMenuBar.md "BMenuBar") and the [`BMenu`](BMenu.md "BMenu") it displays, provided the `deep` flag is `true`. This function then adds the label, divider, and current state of the `BMenuField` to the [`BMessage`](BMessage.md "BMessage") archive.

See also: [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)"), [`Instantiate()`](BMenuField.md#BMenuField_Instantiate "Instantiate\(\)") static function

#### MakeFocus()

`virtual void MakeFocus(bool focused);`

Augments the [`BView`](BView.md "BView") version of [`MakeFocus()`](BView.md#BView_MakeFocus "MakeFocus\(\)") to enable keyboard navigation. This function calls [`Draw()`](BMenuField.md#BMenuField_Draw "Draw\(\)") when the `BMenuField` becomes the focus view and when it loses that status.

####  Menu(), MenuBar(), MenuItem()

`[`BMenu`](BMenu.md "BMenu")* Menu() const;``[`BMenuBar`](BMenu.md "BMenu")* MenuBar() const;``[`BMenuItem`](BMenu.md "BMenu")* MenuItem() const;`

`Menu()` returns the [`BMenu`](BMenu.md "BMenu") object that pops up when the user operates the `BMenuField`; `MenuBar()` returns the [`BMenuBar`](BMenuBar.md "BMenuBar") object that contains the menu. The [`BMenuBar`](BMenuBar.md "BMenuBar") is created by the `BMenuField`; the menu is assigned to it during construction. `MenuItem()` returns the first [`BMenuItem`](BMenuItem.md "BMenuItem") assigned to the [`BMenuBar`](BMenuBar.md "BMenuBar") object containing the menu.

See also: The `BMenuField` [constructor](BMenuField.md#BMenuField_Constructor "BMenuField")

####  SetAlignment(), Alignment()

`virtual void SetAlignment(alignment label);``alignment Alignment() const;`

These functions set and return the alignment of the label in its portion of the frame rectangle.

Constant| Description  
---|---  
`B_ALIGN_LEFT`| The label is aligned at the left side of the bounds rectangle.  
`B_ALIGN_RIGHT`| The label is aligned at the right boundary of its portion of the bounds rectangle.  
`B_ALIGN_CENTER`| The label is centered in its portion of the bounds rectangle.  
  
The default is `B_ALIGN_LEFT`.

####  SetDivider(), Divider()

`virtual void SetDivider(float xCoordinate);``float Divider() const;`

These functions set and return the x coordinate value that divides the bounds rectangle between the label's portion on the left and the portion that holds the menu on the right. The coordinate is expressed in the `BMenuField`'s coordinate system.

The default divider splits the bounds rectangle in two equal sections. By resetting it, you can provide more or less room for the label or the menu.

####  SetEnabled(), IsEnabled()

`virtual void SetEnabled(bool enabled);``bool IsEnabled() const;`

`SetEnabled()` enables the `BMenuField` if the `enabled` flag is `true`, and disables it if the flag is `false`. `IsEnabled()` returns whether or not the object is currently enabled. When disabled, the `BMenuField` doesn't respond to mouse and keyboard manipulations.

If the `enabled` flag changes the current state of the object, `SetEnabled()` causes the view to be redrawn, so that its new state can be displayed to the user.

####  SetLabel(), Label()

`virtual void SetLabel(const char* string);``const char* Label() const;`

`SetLabel()` frees the current label and, if the argument it's passed is not `NULL`, replaces it with a copy of string. `Label()` returns the current label. The string it returns belongs to the `BMenuField` object.

See also: The `BMenuField` [constructor](BMenuField.md#BMenuField_Constructor "BMenuField")

* * *

### Static Functions

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Returns a new `BMenuField` object, allocated by new and created with the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive. However, if the archive message doesn't contain data for a `BMenuField` object, this function returns `NULL`.

See also: [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"), [`Archive()`](BMenuField.md#BMenuField_Archive "Archive\(\)")

* * *

### Archived Fields

The `BMenu` class implements the suite called "suite/vnd.Be-menu" consisting of the following messages:

Field| Type code| Description  
---|---|---  
`_label`| `B_STRING_TYPE`| `BMenuField` label.  
`_disable`| `B_BOOL_TYPE`| Exists and `true` only if menu is disabled.  
`_align`| `B_INT32_TYPE`| Alignment of label in the frame rectangle.  
`_divide`| `B_FLOAT_TYPE`| The x-coordinate dividing the bounds rectangle between the label and the menu.  
  
The following views are added to the `_views` field (deep copy only):

Name| Level| Description  
---|---|---  
`_mc_mb_`| 0| Private menu bar.

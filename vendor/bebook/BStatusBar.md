<!-- Classes And Methods - The Interface Kit - BStatusBar -->

## BStatusBar

| Derived From:| [`BView`](BView.md "BView")  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/StatusBar.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BStatusBar_Overview.md "BStatusBar")

  * [Constructor and Destructor](BStatusBar.md#BStatusBar_ConstructorDestructor)
  * [Hook Functions](BStatusBar.md#BStatusBar_HookFunctions)
  * [Member Functions](BStatusBar.md#BStatusBar_MemberFunctions)
  * [Static Functions](BStatusBar.md#BStatusBar_StaticFunctions)
  * [Archived Fields](BStatusBar.md#BStatusBar_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BStatusBar()

`BStatusBar(BRect frame,  
const char* name,  
const char* label = NULL,  
const char* trailingLabel = NULL);` `BStatusBar(BMessage* archive);`

Initializes the `BStatusBar` with a `label` and a `trailingLabel`, which can both be `NULL`. The `frame` rectangle is used for its origin and width only; when the `BStatusBar` is attached to a window, it will be resized to a height that precisely accommodates its parts. The `name` argument is passed on to the [`BView`](BView.md "BView") constructor.

The object is given a `B_WILL_DRAW` flag and a resizing mode that keeps it glued to the left and top sides of its parent.

The default font of the `BStatusBar` is the system plain font, and the default color of the bar is blue (50, 150, 255). It's initial value is 0.0, the minimum, and its default maximum value is 100.0.

See also: [`SetMaxValue()`](BStatusBar.md#BStatusBar_SetMaxValue "SetMaxValue\(\), MaxValue\(\), CurrentValue\(\)"), [`SetBarColor()`](BStatusBar.md#BStatusBar_SetBarColor "SetBarColor\(\), BarColor\(\)")

#### ~BStatusBar()

`virtual ~BStatusBar();`

Frees the labels and the text.

* * *

### Hook Functions

#### AttachedToWindow()

`virtual void AttachedToWindow();`

Resizes the frame rectangle to accommodate the object's graphical parts. The width isn't altered.

This function also sets the view and low colors of the `BStatusBar` to match the background view color of its new parent.

#### Draw()

`virtual void Draw(BRect updateRect);`

Draws the bar, labels, and text.

#### MessageReceived()

`virtual void MessageReceived([`BMessage`](BMessage.md "BMessage")* message);`

Responds to [`B_UPDATE_STATUS_BAR`](TheInterfaceKit_MessageConstants.md#B_UPDATE_STATUS_BAR "B_UPDATE_STATUS_BAR") and [`B_RESET_STATUS_BAR`](TheInterfaceKit_MessageConstants.md#B_RESET_STATUS_BAR "B_RESET_STATUS_BAR") messages by calling the [`Update()`](BStatusBar.md#BStatusBar_Update "Update\(\)") and [`Reset()`](BStatusBar.md#BStatusBar_Reset "Reset\(\)") functions. Each message contains data that can be passed as arguments to the functions.

See also: [`BView::MessageReceived()`](BView.md#BView_MessageReceived "MessageReceived\(\)"), "BStatusBar Messages" in the Message Protocols appendix

* * *

### Member Functions

#### Archive()

`virtual status_t Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Calls the inherited version of `Archive()`, then adds the bar color, bar height, current value, and maximum value to the [`BMessage`](BMessage.md "BMessage") archive, along with the current text, trailing text, label, and trailing label.

####  Label(), TrailingLabel()

`const char* Label() const;``const char* TrailingLabel() const;`

These functions return pointers to the object's label and trailing label. The returned strings belong to the `BStatusBar` object and should not be altered. They can be set only on construction or when all values are reset.

See also: [`Reset()`](BStatusBar.md#BStatusBar_Reset "Reset\(\)")

#### Reset()

`virtual void Reset(const char* label = NULL,  
const char* trailingLabel = NULL);`

Empties the status bar, sets its current value to 0.0 and its maximum value to 100.0, deletes and erases the text and trailing text, and replaces the label and trailing label with copies of the strings passed as arguments. If either argument is `NULL`, the label or trailing label will also be deleted and erased.

This gets the `BStatusBar` ready to be reused for another operation. For example, if several large files are being downloaded, the `BStatusBar` could be reset for each one.

See also: [`SetText()`](BStatusBar.md#BStatusBar_SetText "SetText\(\), SetTrailingText\(\), Text\(\), TrailingText\(\)"), [`Update()`](BStatusBar.md#BStatusBar_Update "Update\(\)"), the [`B_RESET_STATUS_BAR`](TheInterfaceKit_MessageConstants.md#B_RESET_STATUS_BAR "B_RESET_STATUS_BAR") message constant.

####  SetBarColor(), BarColor()

`virtual void SetBarColor(rgb_color color);``rgb_color BarColor() const;`

These functions set and return the color that fills the bar to show how much of an operation has been completed. The default bar color is blue (50, 150, 255).

####  SetBarHeight(), BarHeight()

`virtual void SetBarHeight(float height);``float BarHeight() const;`

These functions set and return the height of the bar itself, minus the text and labels. The default height is 16.0 coordinate units. The frame rectangle is adjusted to accommodate the new height, and the object is redrawn.

####  SetMaxValue(), MaxValue(), CurrentValue()

`virtual void SetMaxValue(float max);``float MaxValue() const;``float CurrentValue() const;`

`SetMaxValue()` sets the maximum value of the `BStatusBar`, which by default is 100.0. `MaxValue()` returns the current maximum. The minimum value is 0.0 and cannot be changed.

`CurrentValue()` returns the current value of the `BStatusBar`. The current value is set by [`Update()`](BStatusBar.md#BStatusBar_Update "Update\(\)") and reset to 0.0 by [`Reset()`](BStatusBar.md#BStatusBar_Reset "Reset\(\)").

The amount of "filling" in the status bar reflects the ratio `CurrentValue()`/`MaxValue()`.

####  SetText(), SetTrailingText(), Text(), TrailingText()

`virtual void SetText(const char* string);``virtual void SetTrailingText(const char* string);``const char* Text() const;``const char* TrailingText() const;`

These `SetText()` and `SetTrailingText()` functions erase the previous text or trailing text and replace them with copies of the string argument. string can be `NULL`. The view is automatically redrawn.

`Text()` and `TrailingText()` return pointers to the current text strings.

#### Update()

`virtual void Update(float delta,  
const char* text = NULL,  
const char* trailingText = NULL);`

`Update()` updates the `BStatusBar` by adding delta to its current value and resetting its text and trailing text. Passing `NULL` for the `text` or `trailingText` argument retains the existing string(s). The status bar is automatically redrawn.

See also: [`Reset()`](BStatusBar.md#BStatusBar_Reset "Reset\(\)"), the [`B_UPDATE_STATUS_BAR`](TheInterfaceKit_MessageConstants.md#B_UPDATE_STATUS_BAR "B_UPDATE_STATUS_BAR") message constant.

* * *

### Static Functions

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Returns a new `BStatusBar` object, allocated by new and created with the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive. However, if the archive doesn't contain data for an `BStatusBar` object, this function returns `NULL`.

See also [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"), [`Archive()`](BStatusBar.md#BStatusBar_Archive "Archive\(\)")

* * *

### Archived Fields

The [`Archive()`](BStatusBar.md#BStatusBar_Archive "Archive\(\)") function adds the following fields to its [`BMessage`](BMessage.md "BMessage") argument:

Field| Type code| Description  
---|---|---  
`_high`| `B_FLOAT_TYPE`| Status bar height.  
`_bcolor`| `B_INT32_TYPE`| Status bar color.  
`_val`| `B_FLOAT_TYPE`| Current value.  
`_max`| `B_FLOAT_TYPE`| Maximum value.  
`_text`| `B_STRING_TYPE`| Status bar text.  
`_ttext`| `B_STRING_TYPE`| Trailing text.  
`_label`| `B_STRING_TYPE`| Status bar label.  
`_tlabel`| `B_STRING_TYPE`| Trailing text label.  
  
Some of these fields may not be present if the setting they represent isn't used, or is the default value. For example, if there is no trailing text, the `_ttext` field won't be found in the archive.

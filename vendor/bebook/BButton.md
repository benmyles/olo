<!-- Classes And Methods - The Interface Kit - BButton -->

## BButton

| Derived From:| [`BControl`](BControl.md "BControl")  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/Button.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BButton_Overview.md "BButton")

  * [Constructor and Destructor](BButton.md#BButton_ConstructorDestructor)
  * [Hook Functions](BButton.md#BButton_HookFunctions)
  * [Member Functions](BButton.md#BButton_MemberFunctions)
  * [Static Functions](BButton.md#BButton_StaticFunctions)
  * [Archived Fields](BButton.md#BButton_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BButton()

`BButton([`BRect`](BRect.md "BRect") frame,  
const char* name,  
const char* label,  
[`BMessage`](BMessage.md "BMessage")* message,  
uint32 resizingMode = B_FOLLOW_LEFT | B_FOLLOW_TOP,  
uint32 flags = B_WILL_DRAW | B_NAVIGABLE);` `BButton([`BMessage`](BMessage.md "BMessage")* archive);`

Initializes the `BButton` by passing all arguments to the [`BControl`](BControl.md "BControl") constructor. [`BControl`](BControl.md "BControl") initializes the button's `label` and assigns it a model `message` that identifies the action that should be carried out when the button is invoked. 

The `frame`, `name`, `resizingMode`, and `flags` arguments are the same as those declared for the [`BView`](BView.md "BView") class and are passed up the inheritance hierarchy to the [`BView`](BView.md "BView") constructor without change.

When the button is attached to a window, it will be resized to its preferred height; the height of `BButton`'s `frame` rectangle will exactly accommodate the button border and label, given the `BButton`'s current font.

#### ~BButton()

`virtual ~BButton();`

Does nothing; a `BButton` has no data to free.

* * *

### Hook Functions

#### AttachedToWindow()

`virtual void AttachedToWindow();`

Augments the [`BControl`](BControl.md "BControl") [version](BControl.md#BControl_AttachedToWindow "AttachedToWindow\(\)") of this function to tell the [`BWindow`](BWindow.md "BWindow") that the button is the default button, if [`MakeDefault()`](BButton.md#BButton_MakeDefault "MakeDefault\(\)") has already been called.

See also: [`BView::AttachedToWindow()`](BView.md#BView_AttachedToWindow "AttachedToWindow\(\), AllAttached\(\)")

#### KeyDown()

`virtual void KeyDown(const char* bytes,  
int32 numBytes);`

Augments the inherited version of `KeyDown()` to respond to messages reporting that the user pressed the Enter key or the space bar. Its response is to:

  * Momentarily highlight the button and change its value.

  * Call [`Invoke()`](BControl.md#BControl_Invoke "Invoke\(\)") to deliver a copy of the model [`BMessage`](BMessage.md "BMessage") to the target receiver.

The `BButton` gets `KeyDown()` function calls when it's the focus view for the active window (which results when the user navigates to it) and also when it's the default button for the window and the character the user types is `B_ENTER`.

See also: [`BView::KeyDown()`](BView.md#BView_KeyDown "KeyDown\(\)"), [`MakeDefault()`](BButton.md#BButton_MakeDefault "MakeDefault\(\)")

#### MakeDefault()

`virtual void MakeDefault(bool flag);`

`MakeDefault()` makes the `BButton` the default button for its window when `flag` is `true`, and removes that status when `flag` is `false`. The default button is the button the user can operate by striking the Enter key when the window is the active window.

A window can have only one default button at a time. Setting a new default button, therefore, may deprive another button of that status. When `MakeDefault()` is called with an argument of `true`, it generates a `MakeDefault()` call with an argument of `false` for previous default button. Both buttons are redisplayed so that the user can see which one is currently the default.

The default button can also be set by calling [`BWindow`](BWindow.md "BWindow")'s [`SetDefaultButton()`](BWindow.md#BWindow_SetDefaultButton "SetDefaultButton\(\), DefaultButton\(\)") function. That function makes sure that the button that's forced to give up default status and the button that obtains it are both notified through `MakeDefault()` function calls.

`MakeDefault()` is therefore a hook function that can be augmented to take note each time the default status of the button changes. It's called once for each change in status, no matter which function initiated the change.

See also: [`BWindow::SetDefaultButton()`](BWindow.md#BWindow_SetDefaultButton "SetDefaultButton\(\), DefaultButton\(\)")

#### MouseDown()

`virtual void MouseDown([`BPoint`](BPoint.md "BPoint") point);`

Overrides the [`BView`](BView.md "BView") version of [`MouseDown()`](BView.md#BView_MouseDown "MouseDown\(\)") to track the cursor while the user holds the mouse button down. As the cursor moves in and out of the button, the `BButton`'s value is reset accordingly. The [`SetValue()`](BControl.md#BControl_SetValue "SetValue\(\), Value\(\)") virtual function is called to make the change each time.

If the cursor is inside the `BButton`'s bounds rectangle when the user releases the mouse button, this function posts a copy of the model message so that it will be dispatched to the target object.

See also: [`BView::MouseDown()`](BView.md#BView_MouseDown "MouseDown\(\)"), [`BControl::Invoke()`](BControl.md#BControl_Invoke "Invoke\(\)"), [`BInvoker::SetTarget()`](BInvoker.md#BInvoker_SetTarget "SetTarget\(\), Target\(\), IsTargetLocal\(\), Messenger\(\)")

* * *

### Member Functions

#### Archive()

`virtual Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Stores the `BButton` in the [`BMessage`](BMessage.md "BMessage") archive.

See also: [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)"), [`Instantiate()`](BButton.md#BButton_Instantiate "Instantiate\(\)") static function

#### Draw()

`virtual void Draw([`BRect`](BRect.md "BRect") updateRect);`

Draws the button and labels it. If the `BButton`'s value is anything but 0, the button is highlighted. If it's disabled, it drawn in muted shades of gray. Otherwise, it's drawn in its ordinary, enabled, unhighlighted state.

See also: [`BView::Draw()`](BView.md#BView_Draw "Draw\(\)")

#### GetPreferredSize()

`virtual void GetPreferredSize(float* width,  
float* height);`

Calculates how big the button needs to be to display its label in the current font, and writes the results into the variables that the `width` and `height` arguments refer to. [`ResizeToPreferred()`](BView.md#BView_ResizeToPreferred), defined in the [`BView`](BView.md "BView") class, resizes a view's frame rectangle to the preferred size, keeping its left and top sides constant. A button is automatically resized to its preferred height (but not to its preferred width) by [`AttachedToWindow()`](BButton.md#BButton_AttachedToWindow "AttachedToWindow\(\)").

#### IsDefault()

`bool IsDefault() const;`

`IsDefault()` returns whether the `BButton` is currently the default button.

See also: [`MakeDefault()`](BButton.md#BButton_MakeDefault "MakeDefault\(\)")

#### SetLabel()

`virtual void SetLabel(const char* string);`

Overrides the [`BControl`](BControl.md "BControl") version of this function to make sure that calculations based on the width of the label won't assume cached results for the previous label.

See also: [`BControl::SetLabel()`](BControl.md#BControl_SetLabel "SetLabel\(\), Label\(\)")

* * *

### Static Functions

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Returns a new `BButton` object, allocated by new and created with the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive. However, if the archive message doesn't contain data for a `BButton` object, this function returns `NULL`.

See also: [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"), [`Archive()`](BButton.md#BButton_Archive "Archive\(\)")

* * *

### Archived Fields

The [`Archive()`](BButton.md#BButton_Archive "Archive\(\)") function adds the following fields to its [`BMessage`](BMessage.md "BMessage") argument:

Field| Type code| Description  
---|---|---  
`_default`| `B_BOOL_TYPE`| Exists and is `true` if the button is the default.

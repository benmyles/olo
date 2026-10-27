<!-- Classes And Methods - The Interface Kit - BScrollBar -->

## BScrollBar

| Derived From:| [`BView`](BView.md "BView")  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/ScrollBar.h`  
Library:| `libbe.so`  
Allocation:| Constructor or on the stack  
[Class Overview](BScrollBar_Overview.md "BScrollBar")

  * [Constructor and Destructor](BScrollBar.md#BScrollBar_ConstructorDestructor)
  * [Hook Functions](BScrollBar.md#BScrollBar_HookFunctions)
  * [Member Functions](BScrollBar.md#BScrollBar_MemberFunctions)
  * [Static Functions](BScrollBar.md#BScrollBar_StaticFunctions)
  * [Archived Fields](BScrollBar.md#BScrollBar_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BScrollBar()

`BScrollBar([`BRect`](BRect.md "BRect") frame,  
const char* name,  
[`BView`](BView.md "BView")* target,  
float min,  
float max,  
orientation posture);` `BScrollBar([`BMessage`](BMessage.md "BMessage")* archive);`

Initializes the `BScrollBar` and connects it to the target view that it will scroll. It will be a horizontal scroll bar if posture is `B_HORIZONTAL` and a vertical scroll bar if posture is `B_VERTICAL`.

The range of values that the scroll bar can represent at the outset is set by `min` and `max`. These values should be calculated from the boundaries of a rectangle that encloses the entire contents of the target view—everything that it can draw. If `min` and `max` are both 0, the scroll bar is disabled and the knob is not drawn.

The object's initial value is 0 even if that falls outside the range set for the scroll bar.

The other arguments, `frame` and `name`, are the same as for other [`BView`](BView.md "BView")s:

  * The `frame` rectangle locates the scroll bar within its parent view. For consistency in the user interface, a horizontal scroll bar should be `B_H_SCROLL_BAR_HEIGHT` coordinate units high, and a vertical scroll bar should be `B_V_SCROLL_BAR_WIDTH` units wide.

  * The `BScrollBar`'s `name` identifies it and permits it to be located by the [`FindView()`](BView.md#BView_FindView "FindView\(\)") function. It can be `NULL`.

Unlike other [`BView`](BView.md "BView")s, the `BScrollBar` constructor doesn't set an automatic resizing mode. By default, scroll bars have the resizing behavior that befits their posture—horizontal scroll bars resize themselves horizontally (as if they had a resizing mode that combined `B_FOLLOW_LEFT_RIGHT` with `B_FOLLOW_BOTTOM`) and vertical scroll bars resize themselves vertically (as if their resizing mode combined `B_FOLLOW_TOP_BOTTOM` with `B_FOLLOW_RIGHT`).

#### ~BScrollBar()

`virtual ~BScrollBar();`

Disconnects the scroll bar from its target.

* * *

### Hook Functions

#### AttachedToWindow()

`virtual void AttachedToWindow();`

Makes sure that the Application Server is cognizant of the `BScrollBar`'s value, if a value was set before the object was attached to a window.

See also: [`BView::AttachedToWindow()`](BView.md#BView_AttachedToWindow "AttachedToWindow\(\), AllAttached\(\)")

#### ValueChanged()

`virtual void ValueChanged(float newValue);`

Responds to a notification that the value of the scroll bar has changed to `newValue`. For a horizontal scroll bar, this function interprets `newValue` as the coordinate value that should be at the left side of the target view's bounds rectangle. For a vertical scroll bar, it interprets `newValue` as the coordinate value that should be at the top of the rectangle. It calls [`ScrollTo()`](BView.md#BView_ScrollTo) to scroll the target's contents into position, unless they have already been scrolled.

`ValueChanged()` is called as the result both of user actions (`B_VALUE_CHANGED` messages received from the Application Server) and of programmatic ones. Programmatically, scrolling can be initiated by the target view (calling [`ScrollTo()`](BView.md#BView_ScrollTo)) or by the `BScrollBar` (calling [`SetValue()`](BScrollBar.md#BScrollBar_SetValue "SetValue\(\), Value\(\)") or [`SetRange()`](BScrollBar.md#BScrollBar_SetRange "SetRange\(\), GetRange\(\)")).

In all these cases, the target view and the scroll bars need to be kept in synch. This is done by a chain of function calls: `ValueChanged()` calls [`ScrollTo()`](BView.md#BView_ScrollTo), which in turn calls [`SetValue()`](BScrollBar.md#BScrollBar_SetValue "SetValue\(\), Value\(\)"), which then calls `ValueChanged()` again. It's up to `ValueChanged()` to get off this merry-go-round, which it does by checking the target view's bounds rectangle. If `newValue` already matches the left or top side of the bounds rectangle, if forgoes calling [`ScrollTo()`](BView.md#BView_ScrollTo).

`ValueChanged()` does nothing if a target [`BView`](BView.md "BView") hasn't been set—or if the target has been set by name, but the name doesn't correspond to an actual [`BView`](BView.md "BView") within the scroll bar's window.

Derived classes can override this function to interpret `newValue` differently, or to do something in addition to scrolling the target view.

See also: [`SetTarget()`](BScrollBar.md#BScrollBar_SetTarget "SetTarget\(\), Target\(\)"), [`SetValue()`](BScrollBar.md#BScrollBar_SetValue "SetValue\(\), Value\(\)"), [`BView::ScrollTo()`](BView.md#BView_ScrollTo)

* * *

### Member Functions

#### Archive()

`virtual status_t Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Calls the inherited version of [`Archive()`](BView.md#BView_Archive "Archive\(\)"), then adds the `BScrollBar`'s range, orientation, current value and proportion, and the size of its big and little steps to the [`BMessage`](BMessage.md "BMessage") `archive`.

See also: [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)"), [`Instantiate()`](BScrollBar.md#BScrollBar_Instantiate "Instantiate\(\)") static function

#### Orientation()

`orientation Orientation() const;`

Returns `B_HORIZONTAL` if the object represents a horizontal scroll bar and `B_VERTICAL` if it represents a vertical scroll bar.

See also: The `BScrollBar` [`constructor`](BScrollBar.md#BScrollBar_Constructor "BScrollBar\(\)")

####  SetProportion(), Proportion()

`void SetProportion(float ratio);``float Proportion() const;`

These functions set and return a value between 0.0 and 1.0 that represents the proportion of the entire document that can be displayed within the target view—the ratio of the width (or height) of the target's bounds rectangle to the width (or height) of its data rectangle. This ratio determines the size of a proportional scroll knob relative to the whole scroll bar. It's not adjusted to take into account the minimum size of the knob.

The proportion should be reset as the size of the data rectangle changes (as data is entered and removed from the document) and when the target view is resized.

####  SetRange(), GetRange()

`void SetRange(float min,  
float max);``void GetRange(float* min,  
float* max) const;`

These functions modify and return the range of the scroll bar. `SetRange()` sets the minimum and maximum values of the scroll bar to `min` and `max`. `GetRange()` places the current minimum and maximum in the variables that `min` and `max` refer to.

If the scroll bar's current value falls outside the new range, it will be reset to the closest value—either `min` or `max`—within range. [`ValueChanged()`](BScrollBar.md#BScrollBar_ValueChanged "ValueChanged\(\)") is called to inform the `BScrollBar` of the change whether or not it's attached to a window.

If the `BScrollBar` is attached to a window, any change in its range will be immediately reflected on-screen. The knob will move to the appropriate position to reflect the current value.

Setting both the minimum and maximum to 0 disables the scroll bar. It will be drawn without a knob.

See also: The `BScrollBar` [`constructor`](BScrollBar.md#BScrollBar_Constructor "BScrollBar\(\)")

####  SetSteps(), GetSteps()

`void SetSteps(float smallStep,  
float bigStep);``void GetSteps(float* smallStep,  
float* bigStep) const;`

`SetSteps()` sets how much a single user action should change the value of the scroll bar—and therefore how far the target view should scroll. `GetSteps()` provides the current settings.

When the user presses one of the scroll arrows at either end of the scroll bar, its value changes by a `smallStep` coordinate units. When the user clicks in the bar itself (other than on the knob), it changes by a `bigStep` units. For an application that displays text, the small step of a vertical scroll bar should be large enough to bring another line of text into view.

The default small step is 1.0, which should be too small for most purposes; the default large step is 10.0, which is also probably too small.

Note

![Note](./images/admonitions/Info_32.png)

Although the step values are specified using type float, only integral values should be specified; otherwise, the scroll bar won't behave as expected.

Currently, a `BScrollBar`'s steps can be successfully set only after it's attached to a window.

See also: [`ValueChanged()`](BScrollBar.md#BScrollBar_ValueChanged "ValueChanged\(\)")

####  SetTarget(), Target()

`void SetTarget([`BView`](BView.md "BView")* view);` `void SetTarget(const char* name);`

`[`BView`](BView.md "BView")* Target() const;`

These functions set and return the target of the `BScrollBar`, the view that the scroll bar scrolls. `SetTarget()` sets the target to `view`, or to the [`BView`](BView.md "BView") identified by `name`. `Target()` returns the current target view. The target can also be set when the `BScrollBar` is constructed.

`SetTarget()` can be called either before or after the `BScrollBar` is attached to a window. If the target is set by `name`, the named view must eventually be found within the same window as the scroll bar. Typically, the target and its scroll bars are children of a container view that serves to bind them together as a unit.

When the target is successfully set, a pointer to the `BScrollBar` object is passed to the target view. This lets the target update its scroll bars when its contents are scrolled.

See also: The `BScrollBar` [`constructor`](BScrollBar.md#BScrollBar_Constructor "BScrollBar\(\)"), [`ValueChanged()`](BScrollBar.md#BScrollBar_ValueChanged "ValueChanged\(\)"), [`BView::ScrollBar()`](BView.md#BView_ScrollBar "ScrollBar\(\)")

####  SetValue(), Value()

`void SetValue(float value);``float Value() const;`

These functions modify and return the value of the scroll bar. The value is usually set as the result of user actions; `SetValue()` provides a way to do it programmatically. `Value()` returns the current value, whether set by `SetValue()` or by the user.

`SetValue()` assigns a new `value` to the scroll bar and calls the [`ValueChanged()`](BScrollBar.md#BScrollBar_ValueChanged "ValueChanged\(\)") hook function, whether or not the new value is really a change from the old. If the value passed lies outside the range of the scroll bar, the `BScrollBar` is reset to the closest value within range—that is, to either the minimum or the maximum value previously specified.

If the scroll bar is attached to a window, changing its value updates its on-screen display. The call to [`ValueChanged()`](BScrollBar.md#BScrollBar_ValueChanged "ValueChanged\(\)") enables the object to scroll the target view so that it too is updated to conform to the new value.

The initial value of a scroll bar is 0.

See also: [`ValueChanged()`](BScrollBar.md#BScrollBar_ValueChanged "ValueChanged\(\)"), [`SetRange()`](BScrollBar.md#BScrollBar_SetRange "SetRange\(\), GetRange\(\)")

* * *

### Static Functions

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Returns a new `BScrollBar` object, allocated by new and created with the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive. However, if the `archive` message doesn't contain data for a `BScrollBar` object, the return value will be `NULL`.

See also: [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"), [`Archive()`](BScrollBar.md#BScrollBar_Archive "Archive\(\)")

* * *

### Archived Fields

The [`Archive()`](BScrollBar.md#BScrollBar_Archive "Archive\(\)") function adds the following fields to its [`BMessage`](BMessage.md "BMessage") argument:

Field| Type code| Description  
---|---|---  
`_range` (array)| `B_FLOAT_TYPE`| Minimum and maximum values of the scroll bar.  
`_steps` (array)| `B_FLOAT_TYPE`| Small and large steps of the scroll bar.  
`_val`| `B_FLOAT_TYPE`| Current value of the scroll bar  
`_orient`| `B_INT32_TYPE`| Orientation of the scroll bar.  
`_prop`| `B_FLOAT_TYPE`| Proportion of the document visible in the target view.

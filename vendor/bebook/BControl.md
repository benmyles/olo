<!-- Classes And Methods - The Interface Kit - BControl -->

## BControl

| Derived From:| [`BView`](BView.md "BView") > [`BHandler`](BHandler.md "BHandler"), [`BInvoker`](BInvoker.md "BInvoker")  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/Control.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BControl_Overview.md "BControl")

  * [Constructor and Destructor](BControl.md#BControl_ConstructorDestructor)
  * [Hook Functions](BControl.md#BControl_HookFunctions)
  * [Member Functions](BControl.md#BControl_MemberFunctions)
  * [Static Functions](BControl.md#BControl_StaticFunctions)
  * [Scripting Support](BControl.md#BControl_ScriptingSupport)
  * [Archived Fields](BControl.md#BControl_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BControl()

`BControl([`BRect`](BRect.md "BRect") frame,  
const char* name,  
const char* label,  
[`BMessage`](BMessage.md "BMessage")* message,  
uint32 resizingMode,  
uint32 flags);` `BControl([`BMessage`](BMessage.md "BMessage")* archive);`

Initializes the `BControl` by setting its initial value to 0 (`B_CONTROL_OFF`), assigning it a `label`, and registering a model `message` that captures what the control does—the command it gives when it's invoked and the information that accompanies the command. The `label` and the `message` can each be `NULL`.

The `label` is copied, but the `message` is not. The [`BMessage`](BMessage.md "BMessage") object becomes the property of the `BControl`; it should not be deleted, posted, assigned to another object, or otherwise used in application code. The label and message can be altered after construction with the [`SetLabel()`](BControl.md#BControl_SetLabel "SetLabel\(\), Label\(\)") and [`SetMessage()`](BInvoker.md#BInvoker_SetMessage "SetMessage\(\), Message\(\), Command\(\)") functions.

The `BControl` class doesn't define a [`Draw()`](BView.md#BView_Draw "Draw\(\)") function to draw the label or a [`MouseDown()`](BView.md#BView_MouseDown "MouseDown\(\)") function to post the message. (It does define [`KeyDown()`](BView.md#BView_KeyDown "KeyDown\(\)"), but only to enable keyboard navigation between controls.) It's up to derived classes to determine how the `label` is drawn and how the `message` is to be used. Typically, when a `BControl` object needs to take action (in response to a click, for example), it calls the [`Invoke()`](BControl.md#BControl_Invoke "Invoke\(\)") function, which copies the model message and delivers the copy to the designated target. By default, the target is the window where the control is located, but [`BInvoker::SetTarget()`](BInvoker.md#BInvoker_SetTarget "SetTarget\(\), Target\(\), IsTargetLocal\(\), Messenger\(\)") can designate another handler.

Before delivering the message, [`Invoke()`](BControl.md#BControl_Invoke "Invoke\(\)") adds two data field to it, under the names `when` and `source`. These names should not be used for data items in the model.

The `frame`, `name`, `resizingMode`, and `flags` arguments are identical to those declared for the [`BView`](BView.md "BView") class and are passed unchanged to the [`BView`](BView.md "BView") constructor.

The `BControl` begins life enabled, and the system plain font is made the default font for all control devices.

See also: the [`BView`](BView.md "BView") [`constructor`](BView.md#BView_Constructor "BView\(\)"), [`BLooper::PostMessage()`](BLooper.md#BLooper_PostMessage "PostMessage\(\)"), [`SetLabel()`](BControl.md#BControl_SetLabel "SetLabel\(\), Label\(\)"), [`SetMessage()`](BInvoker.md#BInvoker_SetMessage "SetMessage\(\), Message\(\), Command\(\)"), [`BInvoker::SetTarget()`](BInvoker.md#BInvoker_SetTarget "SetTarget\(\), Target\(\), IsTargetLocal\(\), Messenger\(\)"), [`Invoke()`](BControl.md#BControl_Invoke "Invoke\(\)")

#### ~BControl()

`virtual ~BControl();`

Frees the model message and all memory allocated by the `BControl`.

* * *

### Hook Functions

#### AttachedToWindow()

`virtual void AttachedToWindow();`

Overrides [`BView`](BView.md "BView")'s version of this function to set the `BControl`'s low color and view color so that it matches the view color of its new parent. It also makes the [`BWindow`](BWindow.md "BWindow") to which the `BControl` has become attached the default target for the [`Invoke()`](BControl.md#BControl_Invoke "Invoke\(\)") function, provided that another target hasn't already been set.

`AttachedToWindow()` is called for you when the `BControl` becomes a child of a view already associated with the window.

See also: [`BView::AttachedToWindow()`](BView.md#BView_AttachedToWindow "AttachedToWindow\(\), AllAttached\(\)"), [`Invoke()`](BControl.md#BControl_Invoke "Invoke\(\)"), [`BInvoker::SetTarget()`](BInvoker.md#BInvoker_SetTarget "SetTarget\(\), Target\(\), IsTargetLocal\(\), Messenger\(\)")

#### KeyDown()

`virtual void KeyDown(const char* bytes,  
int32 numBytes);`

Augments the [`BView`](BView.md "BView") version of [`KeyDown()`](BView.md#BView_KeyDown "KeyDown\(\)") to toggle the `BControl`'s value and call [`Invoke()`](BControl.md#BControl_Invoke "Invoke\(\)") when the character encoded in bytes is either `B_SPACE` or `B_ENTER`. This is done to facilitate keyboard navigation and make all derived control devices operable from the keyboard. Some derived classes—[`BCheckBox`](BCheckBox.md "BCheckBox") in particular—find this version of the function to be adequate. Others, like [`BRadioButton`](BRadioButton.md "BRadioButton"), reimplement it.

`KeyDown()` is called only when the `BControl` is the focus view in the active window. (However, if the window has a default button, `B_ENTER` events will be passed to that object and won't be dispatched to the focus view.)

See also: [`BView::KeyDown()`](BView.md#BView_KeyDown "KeyDown\(\)"), [`MakeFocus()`](BControl.md#BControl_MakeFocus "MakeFocus\(\)")

#### MessageReceived()

`virtual void MessageReceived([`BMessage`](BMessage.md "BMessage")* message);`

Handles scripting messages for the `BControl`. See "[Scripting Support](BControl.md#BControl_ScriptingSupport "Scripting Support")" for a description of the messages.

See also: [`BHandler::MessageReceived()`](BHandler.md#BHandler_MessageReceived "MessageReceived\(\)")

####  SetEnabled(), IsEnabled()

`virtual void SetEnabled(bool enabled);``bool IsEnabled() const;`

`SetEnabled()` enables the `BControl` if the `enabled` flag is `true`, and disables it if enabled is `false`. `IsEnabled()` returns whether or not the object is currently enabled. `BControl`s are enabled by default.

While disabled, a `BControl` won't let the user navigate to it; the `B_NAVIGABLE` flag is turned off if enabled is `false` and turned on again if `enabled` is `true`.

Typically, a disabled `BControl` also won't post messages or respond visually to mouse and keyboard manipulation. To indicate this nonfunctional state, the control device is displayed on-screen in subdued colors. However, it's left to each derived class to carry out this strategy in a way that's appropriate for the kind of control it implements. The `BControl` class merely marks an object as being enabled or disabled; none of its functions take the enabled state of the device into account.

Derived classes can augment `SetEnabled()` (override it) to take action when the control device becomes enabled or disabled. To be sure that `SetEnabled()` has been called to actually make a change, its current state should be checked before calling the inherited version of the function. For example:
    
    
    void MyControl::SetEnabled(bool enabled)
    {
       if ( enabled == IsEnabled() )
          return;
       BControl::SetEnabled(enabled);
       /* Code that responds to the change in state goes here. */
    }

Note, however, that you don't have to override `SetEnabled()` just to update the on-screen display when the control becomes enabled or disabled. If the `BControl` is attached to a window, the kit's version of `SetEnabled()` always calls the [`Draw()`](BView.md#BView_Draw "Draw\(\)") function. Therefore, the device on-screen will be updated automatically—as long as [`Draw()`](BView.md#BView_Draw "Draw\(\)") has been implemented to take the enabled state into account.

See also: the `BControl` [constructor](BControl.md#BControl_Constructor "BControl\(\)")

####  SetValue(), Value()

`virtual void SetValue(int32 value);``int32 Value() const;`

These functions set and return the value of the `BControl` object.

`SetValue()` assigns the object a new value. If the `value` passed is in fact different from the `BControl`'s current value, this function calls the object's [`Draw()`](BView.md#BView_Draw "Draw\(\)") function so that the new value will be reflected in what the user sees on-screen; otherwise it does nothing.

`Value()` returns the current value.

Classes derived from `BControl` should call `SetValue()` to change the value of the control device in response to user actions. The derived classes defined in the Be software kits change values only by calling this function.

Since `SetValue()` is a virtual function, you can override it to take note whenever a control's value changes. However, if you want your code to act only when the value actually changes, you must check to be sure the new value doesn't match the old before calling the inherited version of the function. For example:
    
    
    void MyControl::SetValue(int32 value)
    {
       if ( value != Value() ) {
          BControl::SetValue(value);
          /* MyControl's additions to SetValue() go here*/
       }
    }

Remember that the `BControl` version of `SetValue()` does nothing unless the new value differs from the old.

#### WindowActivated()

`virtual void WindowActivated(bool active);`

Makes sure that the `BControl`, if it's the focus view, is redrawn when the window is activated or deactivated.

See also: [`BView::WindowActivated()`](BView.md#BView_WindowActivated "WindowActivated\(\)")

* * *

### Member Functions

#### Archive()

`virtual status_t Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Archives the `BControl` by recording its label, current value, model message, and whether or not it's disabled in the [`BMessage`](BMessage.md "BMessage") `archive`.

See also: [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)"), [`Instantiate()`](BControl.md#BControl_Instantiate "Instantiate\(\)") static function

#### GetSupportedSuites()

`virtual status_t GetSupportedSuites([`BMessage`](BMessage.md "BMessage")* message);`

Adds the name "suite/vnd.Be-control" to the message. See "[Scripting Support](BControl.md#BControl_ScriptingSupport "Scripting Support")" and the "[Scripting](TheApplicationKit_Scripting.md "Scripting")" section in The Application Kit overview for more information.

See also: [`BHandler::GetSupportedSuites()`](BHandler.md#BHandler_GetSupportedSuites "GetSupportedSuites\(\)")

#### Invoke()

`virtual status_t Invoke([`BMessage`](BMessage.md "BMessage")* message = NULL);`

Copies the `BControl`'s model [`BMessage`](BMessage.md "BMessage") and sends the copy so that it will be dispatched to the designated target (which may be a [`BLooper`](BLooper.md "BLooper")'s preferred handler). The following two pieces of information are added to the copy before it's delivered:

Data name| Type code| Description  
---|---|---  
"when"| `B_INT64_TYPE`| When the control was invoked, as measured in the number of milliseconds since 12:00:00 AM January 1, 1970.  
"source"| `B_POINTER_TYPE`| A pointer to the `BControl` object. This permits the message handler to request more information from the source of the message.  
  
These two names shouldn't be used for data fields in the model.

`BControl`'s version of `Invoke()` overrides the [version](BInvoker.md#BInvoker_Invoke "Invoke\(\), InvokeNotify\(\)") that the [`BInvoker`](BInvoker.md "BInvoker") class defines. It's designed to be called by derived classes in their [`MouseDown()`](BView.md#BView_MouseDown "MouseDown\(\)") and [`KeyDown()`](BControl.md#BControl_KeyDown "KeyDown\(\)") functions; it's not called for you in `BControl` code. It's up to each derived class to define what user actions trigger the call to `Invoke()`—what activity constitutes "invoking" the control.

This function doesn't check to make sure the `BControl` is currently enabled. Derived classes should make that determination before calling `Invoke()`.

See also: [`SetEnabled()`](BControl.md#BControl_SetEnabled "SetEnabled\(\), IsEnabled\(\)")

#### IsFocusChanging()

##### protected

`bool IsFocusChanging() const;`

Returns `true` if the `BControl` is being asked to draw because the focus changed, and `false` if not. If the return value is `true`, either the `BControl` has just become the focus view or it has just lost that status and the [`Draw()`](BView.md#BView_Draw "Draw\(\)") function has been called to update the on-screen display.

This function can be called from inside [`Draw()`](BView.md#BView_Draw "Draw\(\)") to learn whether it's necessary to draw or erase the visible indication that the `BControl` is the focus view. `IsFocusChanging()` will return the new status of the view.

See also: [`MakeFocus()`](BControl.md#BControl_MakeFocus "MakeFocus\(\)")

#### MakeFocus()

`virtual void MakeFocus(bool focused = true);`

Augments the [`BView`](BView.md "BView") version of this function to call the `BControl`'s [`Draw()`](BView.md#BView_Draw "Draw\(\)") function when the focus changes. This is done to aid keyboard navigation among control devices. If the [`Draw()`](BView.md#BView_Draw "Draw\(\)") function of a derived class has a section of code that checks whether the object is in focus and marks the on-screen display to show that it is (and removes any such marking when it isn't), the visual part of keyboard navigation will be taken care of. The derived class doesn't have to reimplement `MakeFocus()`. Most of the derived classes implemented in the Interface Kit depend on this version of the function.

When [`Draw()`](BView.md#BView_Draw "Draw\(\)") is called from this function, [`IsFocusChanging()`](BControl.md#BControl_IsFocusChanging "IsFocusChanging\(\)") returns `true`.

See also: [`BView::MakeFocus()`](BView.md#BView_MakeFocus "MakeFocus\(\)"), [`KeyDown()`](BControl.md#BControl_KeyDown "KeyDown\(\)"), [`IsFocusChanging()`](BControl.md#BControl_IsFocusChanging "IsFocusChanging\(\)")

#### ResolveSpecifier()

`virtual [`BHandler`](BHandler.md "BHandler")* ResolveSpecifier([`BMessage`](BMessage.md "BMessage")* message,  
int32 index,  
[`BMessage`](BMessage.md "BMessage")* specifier,  
int32 command,  
const char* property);`

Resolves specifiers for the "Label" and "Value" properties. See "[Scripting Support](BControl.md#BControl_ScriptingSupport "Scripting Support")" and the "[Scripting](TheApplicationKit_Scripting.md "Scripting")" section in The Application Kit overview for more information.

See also: [`BHandler::ResolveSpecifier()`](BHandler.md#BHandler_ResolveSpecifier "ResolveSpecifier\(\)")

####  SetLabel(), Label()

`virtual void SetLabel(const char* string);``const char* Label() const;`

These functions set and return the label on a control device—the text that's displayed, for example, on top of a button or alongside a check box or radio button. The label is a null-terminated string.

`SetLabel()` frees the old label, replaces it with a copy of `string`, and updates the control on-screen so the new label will be displayed to the user—but only if the string that's passed differs from the current label. The label is first set by the [constructor](BControl.md#BControl_Constructor "BControl\(\)") and can be modified thereafter by this function.

`Label()` returns the current label. The string it returns belongs to the `BControl` and may be altered or freed in due course.

See also: the `BControl` constructor, [`BView::AttachedToWindow()`](BView.md#BView_AttachedToWindow "AttachedToWindow\(\), AllAttached\(\)")

* * *

### Static Functions

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Returns a new `BControl` object, allocated by new and created with the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive. However, if the archive doesn't contain data for a `BControl` object, `Instantiate()` returns `NULL`.

See also: [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"), [`Archive()`](BControl.md#BControl_Archive "Archive\(\)")

* * *

### Scripting Support

The `BControl` class implements the suite called "suite/vnd.Be-control" consisting of the following messages:

#### The Enabled Property

Whether or not the control is enabled

The "Enabled" property corresponds to the methods [`IsEnabled()`](BControl.md#BControl_IsEnabled) and [`SetEnabled()`](BControl.md#BControl_SetEnabled "SetEnabled\(\), IsEnabled\(\)"), using a boolean to store the data.

Message| Specifiers| Description  
---|---|---  
`B_GET_PROPERTY`| `B_DIRECT_SPECIFIER`| Returns whether or not the `BControl` is currently enabled.  
`B_SET_PROPERTY`| `B_DIRECT_SPECIFIER`| Enables or disables the `BControl`.  
  
#### The Label Property

The text label for the control

The "Label" property corresponds to the methods [`Label()`](BControl.md#BControl_Label) and [`SetLabel()`](BControl.md#BControl_SetLabel "SetLabel\(\), Label\(\)"), using a string to store the data.

Message| Specifiers| Description  
---|---|---  
`B_GET_PROPERTY`| `B_DIRECT_SPECIFIER`| Returns the `BControl`'s label.  
`B_SET_PROPERTY`| `B_DIRECT_SPECIFIER`| Sets the label of the `BControl`.  
  
#### The Value Property

The value of the control

The "Value" property corresponds to the methods [`Value()`](BControl.md#BControl_Value) and [`SetValue()`](BControl.md#BControl_SetValue "SetValue\(\), Value\(\)"), using an int32 to store the data.

Message| Specifiers| Description  
---|---|---  
`B_GET_PROPERTY`| `B_DIRECT_SPECIFIER`| Returns the `BControl`'s value.  
`B_SET_PROPERTY`| `B_DIRECT_SPECIFIER`| Sets the value of the `BControl`.  
  
* * *

### Archived Fields

The [`Archive()`](BControl.md#BControl_Archive "Archive\(\)") function adds the following fields to its [`BMessage`](BMessage.md "BMessage") argument:

Field| Type code| Description  
---|---|---  
`_msg`| `B_MESSAGE_TYPE`| The `BControl`'s modal invocation message.  
`_label`| `B_STRING_TYPE`| The `BControl`'s label.  
`_val`| `B_INT32_TYPE`| The `BControl`'s value.  
`_disable`| `B_BOOL_TYPE`| `true` if the control is disabled.  
  
Some of these fields may not be present if the setting they represent isn't used, or is the default value. For example, if the value is 0, the `_val` field won't be found in the archive.

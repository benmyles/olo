<!-- Classes And Methods - The Interface Kit - BTextControl -->

## BTextControl

| Derived From:| [`BControl`](BControl.md "BControl")  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/TextControl.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BTextControl_Overview.md "BTextControl")

  * [Constructor and Destructor](BTextControl.md#BTextControl_ConstructorDestructor)
  * [Hook Functions](BTextControl.md#BTextControl_HookFunctions)
  * [Member Functions](BTextControl.md#BTextControl_MemberFunctions)
  * [Static Functions](BTextControl.md#BTextControl_StaticFunctions)
  * [Archived Fields](BTextControl.md#BTextControl_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BTextControl()

`BTextControl([`BRect`](BRect.md "BRect") frame,  
const char* name,  
const char* label,  
const char* text,  
[`BMessage`](BMessage.md "BMessage")* message,  
uint32 resizingMode = B_FOLLOW_LEFT | B_FOLLOW_TOP,  
uint32 flags = B_WILL_DRAW | B_NAVIGABLE);` `BTextControl([`BMessage`](BMessage.md "BMessage")* archive);`

Initializes the `BTextControl` by assigning it a `label` and some `text`, both of which can be `NULL`. If the `label` is `NULL`, the entire bounds rectangle is assigned to the text. Otherwise, half the view is assigned to the label and half to the text, though the exact proportion can be changed by the [`SetDivider()`](BTextControl.md#BTextControl_SetDivider "SetDivider\(\), Divider\(\)") function. The label always is on the left and the text always on the right. By default, both label and text are aligned at the left margins of their respective sections; call [`SetAlignment()`](BTextControl.md#BTextControl_SetAlignment "SetAlignment\(\) , GetAlignment\(\)") to alter the alignment.

The `message` parameter is the same as the one declared for the [`BControl`](BControl.md "BControl") constructor. It establishes a model for the messages the `BTextControl` will send when it's invoked. It can be `NULL`. See [`SetMessage()`](BInvoker.md#BInvoker_SetMessage "SetMessage\(\), Message\(\), Command\(\)") and [`SetTarget()`](BInvoker.md#BInvoker_SetTarget "SetTarget\(\), Target\(\), IsTargetLocal\(\), Messenger\(\)") in the [`BInvoker`](BInvoker.md "BInvoker") class and [`Invoke()`](BControl.md#BControl_Invoke "Invoke\(\)") in the [`BControl`](BControl.md "BControl") class for more information.

The `frame`, `name`, `resizingMode`, and `flags` arguments are the same as those declared for the [`BView`](BView.md "BView") class and are passed up the inheritance hierarchy to the `BView` [constructor](BView.md#BView_Constructor "BView\(\)") without change. When the `BTextControl` is attached to a window, it will be resized to the optimal height for displaying the label and text.

#### ~BTextControl()

`virtual ~BTextControl();`

Frees memory allocated by the `BTextControl` and its [`BTextView`](BTextView.md "BTextView") child.

* * *

### Hook Functions

#### AttachedToWindow()

`virtual void AttachedToWindow();`

Augments the [`BControl`](BControl.md "BControl") version of [`AttachedToWindow()`](BControl.md#BControl_AttachedToWindow "AttachedToWindow\(\)") to set up its child [`BTextView`](BTextView.md "BTextView") and to make the view and low colors of the `BTextControl` the same as the background view color of its new parent . This function also adjusts the height of the `BTextControl` to fit the height of the label and text, given the fonts used to display them.

See also: [`BView::AttachedToWindow()`](BView.md#BView_AttachedToWindow "AttachedToWindow\(\), AllAttached\(\)")

#### Draw()

`virtual void Draw([`BRect`](BRect.md "BRect") updateRect);`

Draws the label. (The `BTextControl` defers to its child [`BTextView`](BTextView.md "BTextView") to draw the editable text string.)

See also: [`BView::Draw()`](BView.md#BView_Draw "Draw\(\)")

####  GetPreferredSize() , ResizeToPreferred()

`virtual void GetPreferredSize(float* width,  
float* height);``virtual void ResizeToPreferred();`

`GetPreferredSize()` calculates the optimal size for the `BTextControl` to display the label and the text, given its current font and the current font of its [`BTextView`](BTextView.md "BTextView") child; it places the result in the variables that the `width` and `height` arguments refer to.

`ResizeToPreferred()` resizes the `BTextControl` to its preferred size, keeping its left and top sides constant, and adjusts the size of the BTextView to fit.

See also: [`BView::GetPreferredSize()`](BView.md#BView_GetPreferredSize "GetPreferredSize\(\), ResizeToPreferred\(\)")

#### MessageReceived()

`virtual void MessageReceived([`BMessage`](BMessage.md "BMessage")* message);`

Handles scripting requests for the `BTextControl`.

See also: [`BHandler::MessageReceived()`](BHandler.md#BHandler_MessageReceived "MessageReceived\(\)")

#### MouseDown()

`virtual void MouseDown(BPoint point);`

Makes the child [`BTextView`](BTextView.md "BTextView") the focus view when the user clicks in the `BTextControl` but outside the text.

See also: [`BTextView::MouseDown()`](BTextView.md#BTextView_MouseDown "MouseDown\(\)")

#### WindowActivated()

`virtual void WindowActivated(bool active);`

Makes sure that the `BTextControl` is redrawn when the window is activated and deactivated, if its child [`BTextView`](BTextView.md "BTextView") is the current focus view. This gives the `BTextControl` a chance to draw or erase the indication that it's the current focus for keyboard navigation.

* * *

### Member Functions

#### Archive()

`virtual status_t Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Calls the inherited version of `Archive()`, which will archive the child [`BTextView`](BTextView.md "BTextView") if the `deep` flag is `true`, then adds the alignment of the label and the text, the modification message, and the divider to the [`BMessage`](BMessage.md "BMessage") archive.

See also: [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)"), [`Instantiate()`](BTextControl.md#BTextControl_Instantiate "Instantiate\(\)") static function

#### MakeFocus()

`virtual void MakeFocus(bool focused = true);`

Passes the `MakeFocus()` instruction on to the child [`BTextView`](BTextView.md "BTextView"). If the `flag` is `true`, this function selects all the text in the child [`BTextView`](BTextView.md "BTextView"), which becomes the new focus view for the window. If the `flag` is `false`, the child will no longer be the focus view. If the text has changed when the child ceases to be the focus view, the `BTextControl` is considered to have been invoked; a copy of its model message is posted so that it will be delivered to the target handler.

Note that the `BTextControl` itself never becomes the focus view, so will return `false` to all `IsFocus()` queries.

See also: [`BView::MakeFocus()`](BView.md#BView_MakeFocus "MakeFocus\(\)")

#### ResolveSpecifier()

`virtual [`BHandler`](BHandler.md "BHandler")* ResolveSpecifier([`BMessage`](BMessage.md "BMessage")* message,  
int32 index,  
[`BMessage`](BMessage.md "BMessage")* specifier,  
int32 command,  
const char* property);`

Resolves specifiers for the `Label` and `Value` properties. See "[Scripting](TheApplicationKit_Scripting.md "Scripting")" section of The Application Kit chapter for more information.

See also: [`BHandler::ResolveSpecifier()`](BHandler.md#BHandler_ResolveSpecifier "ResolveSpecifier\(\)")

####  SetAlignment() , GetAlignment()

`virtual void SetAlignment(alignment forLabel,  
alignment forText);``void GetAlignment(alignment* forLabel,  
alignment* forText) const;`

These functions set and report the alignment of the label and the text within their respective portions of the view. Three settings are possible:

Constant| Description  
---|---  
`B_ALIGN_LEFT`| The label or text is aligned at the left boundary of its part of the view rectangle.  
`B_ALIGN_RIGHT`| The label or text is aligned at the right boundary of its part of the view rectangle.  
`B_ALIGN_CENTER`| The label or text is centered within its part of the view rectangle.  
  
The default alignment is `B_ALIGN_LEFT` for both label and text.

See also: [`SetDivider()`](BTextControl.md#BTextControl_SetDivider "SetDivider\(\), Divider\(\)")

####  SetDivider(), Divider()

`virtual void SetDivider(float xCoordinate);``float Divider() const;`

These functions set and return the x coordinate value that marks the division between the label portion of the view rectangle on the left and the text portion on the right. It's stated in the coordinate system of the `BTextControl`.

See also: The `BTextControl` [constructor](BTextControl.md#BTextControl_Constructor "BTextControl\(\)")

#### SetEnabled()

`virtual void SetEnabled(bool enabled);`

Disables the `BTextControl` if the `enabled` flag is `false`, and reenables it if `enabled` is `true`. `BTextControl`s are enabled by default.

This function augments the [`BControl`](BControl.md "BControl") version of [`SetEnabled()`](BControl.md#BControl_SetEnabled "SetEnabled\(\), IsEnabled\(\)"). When the control is disabled, it makes the text unselectable (and therefore uneditable) and draws it in a way that displays its disabled state. When the control is re-enabled, it makes the text editable (and therefore selectable) and draws it as normal text.

####  SetModificationMessage(), ModificationMessage()

`virtual void SetModificationMessage([`BMessage`](BMessage.md "BMessage")* message);``[`BMessage`](BMessage.md "BMessage")* ModificationMessage() const;`

These functions set and return the message that the `BTextControl` sends to its target when the user begins to enter or edit text.

`SetModificationMessage()` assigns `message` to the `BTextControl`, freeing the message previously assigned, if any. The message becomes the responsibility of the `BTextControl` object and will be freed only when it's replaced by another message or the `BTextControl` is freed; you shouldn't free it yourself. Passing a `NULL` pointer to this function deletes the current modification message without replacing it.

The assigned [`BMessage`](BMessage.md "BMessage") becomes the model for the message that the `BTextControl` sends whenever the user modifies the text when the child [`BTextView`](BTextView.md "BTextView") is the focus view.

Before sending the message, the `BTextControl` adds these two pieces of information to it:

Field| Type code| Description  
---|---|---  
"when"| `B_INT64_TYPE`| When the user modified the text, as measured by the number of microseconds since 12:00:00 AM January 1, 1970.  
"source"| `B_POINTER_TYPE`| A pointer to the `BTextControl` object.  
  
These names should not be used for any data that you place in the model message.

`ModificationMessage()` returns the model message.

####  SetText(), Text()

`virtual void SetText(const char* text);``const char* Text() const;`

These functions set and return the text displayed by the `BTextControl`—or rather by its child [`BTextView`](BTextView.md "BTextView"). The text is first set by the constructor.

#### TextView()

`[`BTextView`](BTextView.md "BTextView")* TextView() const;`

Returns the child [`BTextView`](BTextView.md "BTextView") object that handles the `BTextControl`'s editing chores.

* * *

### Static Functions

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Returns a new `BTextControl` object, allocated by new and created with the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive. However, if the archive message doesn't contain data for a `BTextControl` object, `Instantiate()` returns `NULL`.

See also: [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"), [`Archive()`](BTextControl.md#BTextControl_Archive "Archive\(\)")

* * *

### Archived Fields

The [`Archive()`](BTextControl.md#BTextControl_Archive "Archive\(\)") function adds the following fields to its [`BMessage`](BMessage.md "BMessage") argument:

Field| Type code| Description  
---|---|---  
`_a_label`| `B_INT32_TYPE`| Alignment of label in frame.  
`_a_text`| `B_INT32_TYPE`| Alignment of text in frame.  
`_divide`| `B_FLOAT_TYPE`| x-coordinate dividing label and text in frame.  
`_mod_msg`| `B_MESSAGE_TYPE`| Invocation message for the control (only present if one was defined).  
  
The following view is added to the "_views" field (deep copy only):

Name| Level| Description  
---|---|---  
`_input_`| 0| Text input view.

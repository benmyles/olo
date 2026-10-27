<!-- Classes And Methods - The Interface Kit - BRadioButton -->

## BRadioButton

| Derived From:| [`BControl`](BControl.md "BControl")  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/RadioButton.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BRadioButton_Overview.md "BRadioButton")

  * [Constructor and Destructor](BRadioButton.md#BRadioButton_ConstructorDestructor)
  * [Hook Functions](BRadioButton.md#BRadioButton_HookFunctions)
  * [Member Functions](BRadioButton.md#BRadioButton_MemberFunctions)
  * [Static Functions](BRadioButton.md#BRadioButton_StaticFunctions)

  
  
* * *

### Constructor and Destructor

#### BRadioButton()

`BRadioButton([`BRect`](BRect.md "BRect") frame,  
const char* name,  
const char* label,  
[`BMessage`](BMessage.md "BMessage")* message,  
uint32 resizingMode = B_FOLLOW_LEFT | B_FOLLOW_TOP,  
uint32 flags = B_WILL_DRAW | B_NAVIGABLE);` `BRadioButton([`BMessage`](BMessage.md "BMessage")* archive);`

Initializes the `BRadioButton` by passing all arguments to the [`BControl`](BControl.md "BControl") constructor without change. [`BControl`](BControl.md "BControl") initializes the radio button's `label` and assigns it a model `message` that identifies the action that should be taken when the radio button is turned on. When the user turns the button on, the `BRadioButton` posts a copy of the `message` so that it can be delivered to the target handler.

The `frame`, `name`, `resizingMode`, and `flags` arguments are the same as those declared for the [`BView`](BView.md "BView") class and are passed without change from [`BControl`](BControl.md "BControl") to the [`BView`](BView.md "BView") constructor.

The `BRadioButton` draws at the bottom of its frame rectangle beginning at the left side. It ignores any extra space at the top or on the right. (However, the user can click anywhere within the `frame` rectangle to turn on the radio button). When the object is attached to a window, the height of the rectangle will be adjusted so that there is exactly the right amount of room to accommodate the label.

See also: The [`BControl`](BControl.md "BControl") and [`BView`](BView.md "BView") constructors, [`AttachedToWindow()`](BRadioButton.md#BRadioButton_AttachedToWindow "AttachedToWindow\(\)")

#### ~BRadioButton()

`virtual ~BRadioButton();`

Does nothing; a `BRadioButton` doesn't need to clean up after itself when it's deleted.

* * *

### Hook Functions

#### AttachedToWindow()

`virtual void AttachedToWindow();`

Augments the [`BControl`](BControl.md "BControl") version of [`AttachedToWindow()`](BControl.md#BControl_AttachedToWindow "AttachedToWindow\(\)") to set the view and low colors of the `BRadioButton` to the match its parent's view color, and to resize the radio button vertically to fit the height of the label it displays. The height of the label depends on the `BRadioButton`'s font.

#### Draw()

`virtual void Draw([`BRect`](BRect.md "BRect") updateRect);`

Draws the radio button—the circular icon—and its label. The center of the icon is filled when the `BRadioButton`'s value is 1 (`B_CONTROL_ON`); it's left empty when the value is 0 (`B_CONTROL_OFF`).

See also: [`BView::Draw()`](BView.md#BView_Draw "Draw\(\)")

#### GetPreferredSize()

`virtual void GetPreferredSize(float* width,  
float* height);`

Calculates the optimal size for the radio button to display the icon and the label in the current font, and places the result in the variables that the `width` and `height` arguments refer to. [`ResizeToPreferred()`](BView.md#BView_ResizeToPreferred), defined in the [`BView`](BView.md "BView") class, resizes a view's frame rectangle to the preferred size, keeping its left and top sides constant. [`AttachedToWindow()`](BRadioButton.md#BRadioButton_AttachedToWindow "AttachedToWindow\(\)") automatically resizes a radio button to its preferred height, but doesn't modify its width.

See also: [`BView::GetPreferredSize()`](BView.md#BView_GetPreferredSize "GetPreferredSize\(\), ResizeToPreferred\(\)"), [`BView::AttachedToWindow()`](BView.md#BView_AttachedToWindow "AttachedToWindow\(\), AllAttached\(\)")

#### KeyDown()

`virtual void KeyDown(const char* bytes,  
int32 numBytes);`

Augments the inherited versions of [`KeyDown()`](BControl.md#BControl_KeyDown "KeyDown\(\)") to turn the radio button on and deliver a message to the target [`BHandler`](BHandler.md "BHandler") when the character passed in `bytes` is `B_SPACE or B_ENTER`.

See also: [`BView::KeyDown()`](BView.md#BView_KeyDown "KeyDown\(\)"), [`SetValue()`](BRadioButton.md#BRadioButton_SetValue "SetValue\(\)")

#### MouseDown()

`virtual void MouseDown([`BPoint`](BPoint.md "BPoint") point);`

Responds to a mouse-down event in the radio button by tracking the cursor while the user holds the mouse button down. If the cursor is pointing to the radio button when the user releases the mouse button, this function turns the button on (and consequently turns all sibling `BRadioButton`s off), calls the `BRadioButton`'s [`Draw()`](BRadioButton.md#BRadioButton_Draw "Draw\(\)") function, and posts a message that will be delivered to the target [`BHandler`](BHandler.md "BHandler"). Unlike a [`BCheckBox`](BCheckBox.md "BCheckBox"), a `BRadioButton` posts the message—it's "invoked"—only when it's turned on, not when it's turned off.

See also: [`BControl::Invoke()`](BControl.md#BControl_Invoke "Invoke\(\)"), [`BInvoker::SetTarget()`](BInvoker.md#BInvoker_SetTarget "SetTarget\(\), Target\(\), IsTargetLocal\(\), Messenger\(\)"), [`SetValue()`](BRadioButton.md#BRadioButton_SetValue "SetValue\(\)")

#### SetValue()

`virtual void SetValue(int32 value);`

Augments the [`BControl`](BControl.md "BControl") version of [`SetValue()`](BControl.md#BControl_SetValue "SetValue\(\), Value\(\)") to turn all sibling `BRadioButton`s off (set their values to 0) when this `BRadioButton` is turned on (when the value passed is anything but 0).

* * *

### Member Functions

#### Archive()

`virtual status_t Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Calls the inherited version of [`Archive()`](BControl.md#BControl_Archive "Archive\(\)") and doesn't add anything specific to the `BRadioButton` class to the [`BMessage`](BMessage.md "BMessage") archive.

[`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)"), [`Instantiate()`](BRadioButton.md#BRadioButton_Instantiate "Instantiate\(\)") static function

* * *

### Static Functions

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Returns a new `BRadioButton` object, allocated by new and created with the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive. However, if the message doesn't contain data for an archived `BRadioButton` object, this function returns `NULL`.

[`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"), [`Archive()`](BRadioButton.md#BRadioButton_Archive "Archive\(\)")

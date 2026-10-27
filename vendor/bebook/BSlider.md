<!-- Classes And Methods - The Interface Kit - BSlider -->

## BSlider

| Derived From:| [`BControl`](BControl.md "BControl")  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/Slider.h`  
Library:| `libbe.so`  
Allocation:| Constructor only  
[Class Overview](BSlider_Overview.md "BSlider")

  * [Constructor and Destructor](BSlider.md#BSlider_ConstructorDestructor)
  * [Hook Functions](BSlider.md#BSlider_HookFunctions)
  * [Member Functions](BSlider.md#BSlider_MemberFunctions)
  * [Static Functions](BSlider.md#BSlider_StaticFunctions)
  * [Archived Fields](BSlider.md#BSlider_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BSlider()

`BSlider(BRect frame,  
const char* name,  
const char* label,  
BMessage* message,  
int32 minValue,  
int32 maxValue,  
thumb_style thumbType = B_BLOCK_THUMB,  
int32 resizingMode = B_FOLLOW_LEFT | B_FOLLOW_TOP,  
int32 flags = B_FRAME_EVENTS | B_WILL_DRAW | B_NAVIGABLE);` `BSlider(BRect frame,  
const char* name,  
const char* label,  
BMessage* message,  
int32 minValue,  
int32 maxValue,  
orientation posture,  
thumb_style thumbType = B_BLOCK_THUMB,  
int32 resizingMode = B_FOLLOW_LEFT | B_FOLLOW_TOP,  
int32 flags = B_FRAME_EVENTS | B_WILL_DRAW | B_NAVIGABLE);` `BSlider([`BMessage`](BMessage.md "BMessage")* archive);`

Initializes the `BSlider` by passing the appropriate arguments to the [`BControl`](BControl.md "BControl") constructor. [`BControl`](BControl.md "BControl") initializes the slider's `label` and assigns it a model `message` that identifies the action that should be carried out when the slider is invoked.

The `frame`, `name`, `resizingMode`, and `flags` arguments are the same as those declared for the [`BView`](BView.md "BView") class and are passed up the inheritance hierarchy to the [`BView`](BView.md "BView") constructor without change.

The `minValue` and `maxValue` parameters define the minimum and maximum values to which the slider can be set.

If you want to control whether the slider is horizontal or vertical, use the second form of the constructor, and set the `posture` argument to either `B_HORIZONTAL` or `B_VERTICAL`. If you don't specify this argument, the slider will be horizontal by default.

The `thumbType` argument defines the look of the slider's thumb, and can be either of the following values:

Constant| Description  
---|---  
`B_BLOCK_THUMB`| The thumb is a rectangular block.  
`B_TRIANGLE_THUMB`| The thumb is a triangular pointer.  
  
#### ~BSlider()

`virtual ~BSlider();`

Frees memory allocated by the `BSlider` object.

* * *

### Hook Functions

#### AttachedToWindow()

`virtual void AttachedToWindow();`

Augments the [`BControl`](BControl.md "BControl") version of this function to set the background color of the button so that it matches the background color of its parent. This function also sets the start position of the slider and allocates other memory needed for the slider to function.

See also: [`BView::AttachedToWindow()`](BView.md#BView_AttachedToWindow "AttachedToWindow\(\), AllAttached\(\)"), [`BControl::AttachedToWindow()`](BControl.md#BControl_AttachedToWindow "AttachedToWindow\(\)")

#### Draw()

`virtual void Draw(BRect updateRect);`

Draws the slider by calling [`DrawSlider()`](BSlider.md#BSlider_DrawSlider "DrawSlider\(\)").

See also: [`BView::Draw()`](BView.md#BView_Draw "Draw\(\)")

####  DrawBar(), BarFrame()

`virtual void DrawBar();``virtual BRect BarFrame() const;`

`DrawBar()` draws the slider bar. The bar is the narrow region the thumb slides through. The bar should be drawn into the offscreen view.

`BarFrame()` returns the frame rectangle that encloses the slider bar.

These functions can be augmented or replaced by your own versions to alter the appearance of the slider bar.

See also: [`OffscreenView()`](BSlider.md#BSlider_OffscreenView "OffscreenView\(\)")

#### DrawFocusMark()

`virtual void DrawFocusMark();`

If the slider is in focus, draws a mark indicating that this is the case.

You can override this function to alter the appearance of the slider control.

####  DrawHashMarks(), HashMarksFrame()

`virtual void DrawHashMarks();``virtual BRect HashMarksFrame() const;`

`DrawHashMarks()` draws the hash marks.

`HashMarksFrame()` returns the frame rectangle that encloses the hash marks. If the hash marks are set to `B_HASH_MARKS_TOP` or `B_HASH_MARKS_BOTTOM`, the rectangle encloses only the area needed to draw the marks. If the setting is `B_HASH_MARKS_BOTH`, the hash marks frame rectangle is the bounds rectangle of the entire slider control.

These functions can be augmented or replaced by your own versions to alter the appearance of the hash marks.

#### DrawSlider()

`virtual void DrawSlider();`

Draws the entire slider control by calling the functions responsible for drawing the various parts of the control:

  * [`DrawBar()`](BSlider.md#BSlider_DrawBar "DrawBar\(\), BarFrame\(\)")

  * [`DrawHashMarks()`](BSlider.md#BSlider_DrawHashMarks "DrawHashMarks\(\), HashMarksFrame\(\)")

  * [`DrawThumb()`](BSlider.md#BSlider_DrawThumb "DrawThumb\(\), ThumbFrame\(\)")

  * [`DrawFocusMark()`](BSlider.md#BSlider_DrawFocusMark "DrawFocusMark\(\)")

  * [`DrawText()`](BSlider.md#BSlider_DrawText "DrawText\(\)")

Once the slider has been drawn into the offscreen view, it's copied to its parent window.

#### DrawText()

`virtual void DrawText();`

Draws the slider's text areas. These are the minimum label, maximum label, and status message.

The minimum and maximum labels can be set using the [`SetLimitLabels()`](BSlider.md#BSlider_SetLimitLabels "SetLimitLabels\(\), MinLimitLabel\(\), MaxLimitLabel\(\)") function. The status message is obtained by calling the [`UpdateText()`](BSlider.md#BSlider_UpdateText "UpdateText\(\)") function. If you want there to be a status message, simply override [`UpdateText()`](BSlider.md#BSlider_UpdateText "UpdateText\(\)") to return the string you want drawn as the status message.

Note

![Note](./images/admonitions/Info_32.png)

In the default implementation of this function, the limit labels are only drawn if both of them have been configured to a value other than `NULL`. If either of them is `NULL`, neither will be drawn.

This function can be augmented or replaced by your own version to alter the appearance or placement of the text.

####  DrawThumb(), ThumbFrame()

`virtual void DrawThumb();``virtual BRect ThumbFrame() const;`

`DrawThumb()` draws the slider's thumb. If you choose to reimplement this function, you should call [`Style()`](BSlider.md#BSlider_Style) to determine whether to draw a block thumb or a triangle thumb.

`ThumbFrame()` returns the frame rectangle that encloses the thumb.

These functions can be augmented or replaced by your own versions to alter the appearance of the thumb.

#### FrameResized()

`virtual void FrameResized(float* width,  
float* height);`

Augments the [`BControl`](BView.md "BView") version of [`FrameResized()`](BView.md#BView_FrameResized "FrameResized\(\)") to adjust the offscreen view and bitmap used for rendering the slider.

####  GetPreferredSize(), ResizeToPreferred()

`virtual void GetPreferredSize(float* width,  
float* height);``virtual void ResizeToPreferred();`

`GetPreferredSize()` calculates the width and height of the area needed to render the complete slider control, given the current configuration of the control's various settings.

The `width` returned by this function is the width of the slider control. The `height` is calculated by taking the height of the slider bar itself, adding two times the height of the hash marks, then adding room for the status text (if there is any) and limit strings (if there are any).

Note that the height always includes room for hash marks both above and below the slider, even if the slider has no hash marks or only one of the two sets of marks.

`ResizeToPreferred()` resizes the slider control to be the preferred size.

You can override these functions to alter the preferred size of the slider; this is particularly important if you override other functions to change the appearance of the slider.

#### KeyDown()

`virtual void KeyDown(const char* bytes,  
int32 numBytes);`

Augments the inherited version of [`KeyDown()`](BControl.md#BControl_KeyDown "KeyDown\(\)") to let the up and right arrow keys increment the value of the slider, and the down and left arrow keys to decrement the value of the slider.

See also: [`BControl::Invoke()`](BControl.md#BControl_Invoke "Invoke\(\)"), [`BView::KeyDown()`](BView.md#BView_KeyDown "KeyDown\(\)")

#### MouseDown()

`virtual void MouseDown(BPoint point);`

Overrides the [`BView`](BView.md "BView") version of [`MouseDown()`](BView.md#BView_MouseDown "MouseDown\(\)") to track mouse the mouse when the button is pressed. A single click causes the thumb to immediately reposition itself to the clicked location, and a click and drag motion causes the slider to follow the mouse cursor until the button is released.

If a modification message has been established, it is sent repeatedly while the mouse button is down. This can be used, for example, to let the changes to the value of the slider be instantly reflected in an onscreen display. When the mouse button is released and the slider has been set to its resting position, the slider's model message is sent.

See also: [`BControl::Invoke()`](BControl.md#BControl_Invoke "Invoke\(\)"), [`BInvoker::SetTarget()`](BInvoker.md#BInvoker_SetTarget "SetTarget\(\), Target\(\), IsTargetLocal\(\), Messenger\(\)"), [`ModificationMessage()`](BSlider.md#BSlider_ModificationMessage), [`SetModificationMessage()`](BSlider.md#BSlider_SetModificationMessage "SetModificationMessage\(\), ModificationMessage\(\)")

* * *

### Member Functions

#### Archive()

`virtual status_t Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Archives the `BSlider` by recording its the fields shown [here](BSlider.md#BSlider_ArchivedFields "Archived Fields") in the [`BMessage`](BMessage.md "BMessage") `archive`.

See also: [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)"), [`Instantiate()`](BSlider.md#BSlider_Instantiate "Instantiate\(\)") static function

#### OffscreenView()

`[`BView`](BView.md "BView")* OffscreenView([`BView`](BView.md "BView")* view);`

Returns a pointer to the offscreen [`BView`](BView.md "BView") in which the slider is rendered prior to being copied to the screen. Because the slider is a complex graphical construct, it's rendered offscreen, then copied onto the screen. This function is provided to make it possible to further customize the appearance of a custom slider control.

####  SetBarColor(), BarColor()

`virtual void SetBarColor(rgb_color bar_color);``virtual rgb_color BarColor() const;`

`SetBarColor()` sets the color of the slider bar.

`BarColor()` returns the slider bar's color.

####  SetBarThickness(), BarThickness()

`virtual void SetBarThickness(float thickness);``float BarThickness() const;`

`SetBarThickness()` sets the slider bar's thickness and `BarThickness()` returns the current thickness.

The slider bar's thickness determines how many pixels across the slider bar is. If no thickness is defined, the bar occupies the entire unused width or height of the slider's bounding rectangle.

#### SetFont()

`virtual void SetFont(const [`BFont`](BFont.md "BFont")* font,  
uint32 properties = B_FONT_ALL);`

Sets the `font` used for the slider's labels to font. The `properties` flags indicate which properties of the font should be used, and which should remain unchanged.

####  SetHashMarks(), HashMarks()

`virtual void SetHashMarks(hash_mark_location where);``hash_mark_location HashMarks() const;`

`SetHashMarks()` sets the placement of the hash marks. This value can be one of the following:

Constant| Description  
---|---  
`B_HASH_MARKS_NONE`| The slider should have no hash marks.  
`B_HASH_MARKS_TOP`| The slider should have hash marks above the slider bar. For horizontal sliders only.  
`B_HASH_MARKS_BOTTOM`| The slider should have hash marks below the slider bar. For horizontal sliders only.  
`B_HASH_MARKS_BOTH`| The slider should have hash marks above and below the slider bar.  
`B_HASH_MARKS_LEFT`| The slider should have has marks to the left of the slider bar. For vertical sliders only.  
`B_HASH_MARKS_RIGHT`| The slider should have has marks to the right of the slider bar. For vertical sliders only.  
  
`HashMarks()` returns the current placement of the hash marks.

####  SetHashMarkCount(), HashMarkCount()

`virtual void SetHashMarkCount(int32 hash_mark_count);``int32 HashMarkCount() const;`

`SetHashMarkCount()` sets the number of hash marks that will be displayed above and/or below the slider.

`HashMarkCount()` returns the number of hash marks currently set up.

####  SetKeyIncrementValue(), KeyIncrementValue()

`virtual void SetKeyIncrementValue(int32 increment_value);``int32 KeyIncrementValue() const;`

`SetKeyIncrementValue()` sets the amount by which the slider's value changes when the keyboard is used to move the thumb.

`KeyIncrementValue()` returns the amount by which the slider's value changes when the keyboard is used to move the thumb.

####  SetLimitLabels(), MinLimitLabel(), MaxLimitLabel()

`virtual void SetLimitLabels(const char* minLabel,  
const char* maxLabel);``const char* MinLimitLabel() const;``const char* MaxLimitLabel() const;`

`SetLimitLabels()` sets the labels used to mark the minimum and maximum values on the slider.

`MinLimitLabel()` and `MaxLimitLabel()` return the current strings used to mark the minimum and maximum values on the slider.

####  SetModificationMessage(), ModificationMessage()

`virtual void SetModificationMessage([`BMessage`](BMessage.md "BMessage")* message);``[`BMessage`](BMessage.md "BMessage")* ModificationMessage() const;`

`SetModificationMessage()` sets the model message that's sent while the mouse is being tracked. The modification message is sent repeatedly as long as the mouse button is held down after being initially clicked inside the slider.

`ModificationMessage()` returns the modification message currently set up.

See also: [`MouseDown()`](BSlider.md#BSlider_MouseDown "MouseDown\(\)")

####  SetOrientation(), Orientation()

`virtual void SetOrientation(orientation posture);``orientation Orientation() const;`

`SetOrientation()` sets the slider's orientation and `Orientation()` returns its current orientation. The possible values are `B_HORIZONTAL` and `B_VERTICAL`.

####  SetPosition(), Position()

`virtual void SetPosition(float* position);``virtual float Position() const;`

`SetPosition()` sets the value of the slider given a value between 0.0 and 1.0, where 0.0 is the minimum value of the slider, and 1.0 is the maximum value. This lets you set the slider to a relative position without having to look up the maximum and minimum values of the slider.

`Position()` returns the slider value scaled to the range 0.0 to 1.0.

####  SetSnoozeAmount(), SnoozeAmount()

`virtual void SetSnoozeAmount(int32 snooze_time);``int32 SnoozeAmount() const;`

`SetSnoozeAmount()` sets the time (in microseconds) that the slider snoozes for between updates while the mouse is being tracked.

`SnoozeAmount()` returns the time that the slider snoozes while the mouse is being tracked.

####  SetStyle(), Style()

`virtual void SetStyle(thumb_style style);``thumb_style Style() const;`

`SetStyle()` sets the thumb style for the slider. The style can be either of the following two values:

Constant| Description  
---|---  
`B_BLOCK_THUMB`| The thumb is a rectangular block.  
`B_TRIANGLE_THUMB`| The thumb is a triangular pointer.  
  
`Style()` returns the current thumb style.

####  SetValue(), Value(), ValueForPoint()

`virtual void SetValue(int32* value);``virtual int32 Value() const;``virtual int32 ValueForPoint([`BPoint`](BPoint.md "BPoint") location) const;`

`SetValue()` augments the [`BControl`](BControl.md "BControl") [function](BControl.md#BControl_SetValue "SetValue\(\), Value\(\)") to pin the value to the maximum and minimum values for the slider and the position of the thumb before calling the inherited function.

`ValueForPoint()` returns the slider value represented by the given screen coordinates.

#### UpdateText()

`virtual char* UpdateText() const;`

Returns the status message that should be displayed with the slider. By default, this function returns `NULL`; if you want a status message to be displayed, simply override this function to return the appropriate string.

The pointer you return is yours; the [`DrawText()`](BSlider.md#BSlider_DrawText "DrawText\(\)") routine won't dispose of it unless you augment it to do so.

####  UseFillColor(), FillColor()

`virtual void UseFillColor(bool use_fill,  
const rgb_color* bar_color = NULL);``virtual bool FillColor(rgb_color* bar_color) const;`

`UseFillColor()` sets the color used to fill the area of the slider bar that's "filled." That is, the part of the bar between the minimum value of the slider and the current thumb position. If the `use_fill` parameter is `true`, filling this area of the slider with the fill color is turned on. If `use_fill` is `false`, the entire slider bar is always drawn in the bar color.

`FillColor()` stores the current fill color in `bar_color`. The boolean value returned by the function is `true` if the fill color is being used, `false` if not.

* * *

### Static Functions

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Returns a new `BSlider` object, allocated by new and created with the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive. However, if the archive doesn't contain data for a `BSlider` object, `Instantiate()` returns `NULL`.

See also: [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"), [`Archive()`](BControl.md#BControl_Archive "Archive\(\)")

* * *

### Archived Fields

The [`Archive()`](BSlider.md#BSlider_Archive "Archive\(\)") function adds the following fields to its [`BMessage`](BMessage.md "BMessage") argument:

Field| Type code| Description  
---|---|---  
`_mod_msg`| `B_RAW_TYPE`| The flattened modification message.  
`_sdelay`| `B_INT32_TYPE`| The snooze amount.  
`_bcolor`| `B_INT32_TYPE`| The bar color.  
`_fcolor`| `B_INT32_TYPE`| The fill color.  
`_minlbl`| `B_STRING_TYPE`| The minimum limit label.  
`_maxlbl`| `B_STRING_TYPE`| The maximum limit label.  
`_min`| `B_INT32_TYPE`| The minimum possible value.  
`_max`| `B_INT32_TYPE`| The maximum possible value.  
`_incrementvalue`| `B_INT32_TYPE`| The keyboard navigation increment value.  
`_hashcount`| `B_INT32_TYPE`| The number of hash marks to draw.  
`_hashloc`| `B_INT16_TYPE`| Where to draw the hash marks.  
`_sstyle`| `B_INT32_TYPE`| The thumb style.  
  
Some of these fields may not be present if the setting they represent isn't used, or is the default value. For example, if there is no minimum limit label set, the `_min_lbl` field doesn't exist.

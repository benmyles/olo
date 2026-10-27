<!-- Classes And Methods - The Interface Kit - BBox -->

## BBox

| Derived From:| [`BView`](BView.md "BView")  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/Box.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BBox_Overview.md "BBox")

  * [Constructor and Destructor](BBox.md#BBox_ConstructorDestructor)
  * [Member Functions](BBox.md#BBox_MemberFunctions)
  * [Static Functions](BBox.md#BBox_StaticFunctions)
  * [Archived Fields](BBox.md#BBox_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BBox()

`BBox([`BRect`](BRect.md "BRect") frame,  
const char* name = NULL,  
uint32 resizingMode = B_FOLLOW_LEFT | B_FOLLOW_TOP,  
uint32 flags = B_WILL_DRAW | B_FRAME_EVENTS | B_NAVIGABLE_JUMP,  
border_style border = B_FANCY_BORDER);` `BBox([`BMessage`](BMessage.md "BMessage")* archive);`

Initializes the `BBox` by passing the `frame`, `name`, `resizingMode`, and `flags` to the [`BView`](BView.md "BView") constructor, and sets the style of its border to `border`. Three border styles are possible:

Constant| Description  
---|---  
`B_PLAIN_BORDER`| The border is a simple line, lighter on the left and top than on the right and bottom so that the box looks raised from the surrounding surface.  
`B_FANCY_BORDER`| The border is a fancier line that looks like a 3D groove inset into the surrounding surface of the view.  
`B_NO_BORDER`| There is no border. This option is not that useful for a `BBox` object; it turns the box into something other than a box.  
  
The constructor also sets the font for displaying the `BBox`'s label to the system bold font (`be_bold_font`). However, the new object doesn't have a label; call [`SetLabel()`](BBox.md#BBox_SetLabel "SetLabel\(\), Label\(\), LabelView\(\)") to assign it one.

#### ~BBox()

`virtual ~BBox();`

Frees the label, if the `BBox` has one.

* * *

### Member Functions

#### Archive()

`virtual Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Stores the `BBox` in the [`BMessage`](BMessage.md "BMessage") archive.

See also: [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)"), [`Instantiate()`](BBox.md#BBox_Instantiate "Instantiate\(\)") static function

#### AttachedToWindow()

`virtual void AttachedToWindow();`

Makes the `BBox`'s background view color and its low color match the background color of its new parent.

#### Draw()

`virtual void Draw([`BRect`](BRect.md "BRect")updateRect);`

Draws the box and its label. This function is called automatically in response to update messages.

#### FrameResized()

`virtual void FrameResized(float width,  
float height);`

Makes sure that the parts of the box that change when it's resized are redrawn.

See also: [`BView::FrameResized()`](BView.md#BView_FrameResized "FrameResized\(\)")

####  SetBorder(), Border()

`virtual void SetBorder(border_style border);``border_style Border() const;`

These functions set and return the style of border the `BBox` draws—`B_PLAIN_BORDER`, `B_FANCY_BORDER`, or `B_NO_BORDER`. The border style is initially set by the `BBox` constructor.

#### SetLabel(), Label(), LabelView()

`void SetLabel(const char* string);` `status_t SetLabel([`BView`](BView.md "BView")* viewLabel);`

`const char* Label() const;``[`BView`](BView.md "BView")* LabelView() const;`

These functions set and return the label that's displayed along the top edge of the box. `SetLabel()` copies `string` and makes it the `BBox`'s label, freeing the previous label, if any. If `string` is `NULL`, it removes the current label and frees it.

Alternately, you can use the second form of `SetLabel()` to specify a view to be used as the `BBox`'s label. This view can be anything, including controls.

`Label()` returns a pointer to the `BBox`'s current label, or `NULL` if it doesn't have one (or the label is a view).

`LabelView()` returns a pointer to the [`BView`](BView.md "BView") that's being used as the `BBox`'s label, or `NULL` if either there is no label, or the label is a string.

Return Code| Description  
---|---  
`B_OK`.| No error occurred.  
  
No other errors are returned.

* * *

### Static Functions

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Returns a new `BBox` object, allocated by new and created with the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive. However, if the archive message doesn't contain data for a `BBox` object, this function returns `NULL`.

See also: [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"), [`Archive()`](BBox.md#BBox_Archive "Archive\(\)")()

* * *

### Archived Fields

The [`Archive()`](BBox.md#BBox_Archive "Archive\(\)") function adds the following fields to its [`BMessage`](BMessage.md "BMessage") argument:

Field| Type code| Description  
---|---|---  
`_label`| `B_STRING_TYPE`| The box's descriptive text.  
`_style`| `B_BOOL_TYPE`| Exists and is `true` if the style is not `B_FANCY_BORDER`.

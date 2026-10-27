<!-- Classes And Methods - The Interface Kit - BStringView -->

## BStringView

| Derived From:| [`BView`](BView.md "BView")  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/StringView.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BStringView_Overview.md "BStringView")

  * [Constructor and Destructor](BStringView.md#BStringView_ConstructorDestructor)
  * [Hook Functions](BStringView.md#BStringView_HookFunctions)
  * [Member Functions](BStringView.md#BStringView_MemberFunctions)
  * [Static Functions](BStringView.md#BStringView_StaticFunctions)
  * [Archived Fields](BStringView.md#BStringView_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BStringView()

`BStringView([`BRect`](BRect.md "BRect") frame,  
const char* name,  
const char* text,  
uint32 resizingMode = B_FOLLOW_LEFT | B_FOLLOW_TOP,  
uint32 flags = B_WILL_DRAW);` `BStringView([`BMessage`](BMessage.md "BMessage")* archive);`

Initializes the `BStringView` by assigning it a `text` string and the system plain font (`be_plain_font`). The `frame`, `name`, `resizingMode`, and `flags` arguments are passed unchanged to the [`BView`](BView.md "BView") constructor.

The `frame` rectangle needs to be large enough to display the entire string in the current font. The string is drawn at the bottom of the frame rectangle and, by default, is aligned to the left side. A different horizontal alignment can be set by calling [`SetAlignment()`](BStringView.md#BStringView_SetAlignment "SetAlignment\(\), Alignment\(\)").

#### ~BStringView()

`virtual ~BStringView();`

Frees the text string.

* * *

### Hook Functions

#### AttachedToWindow()

`virtual void AttachedToWindow();`

Sets the `BStringView`'s low color and its background view color to match the background color of its new parent view.

See also: [`BView::AttachedToWindow()`](BView.md#BView_AttachedToWindow "AttachedToWindow\(\), AllAttached\(\)")

#### Draw()

`virtual void Draw([`BRect`](BRect.md "BRect") updateRect);`

Draws the string along the bottom of the `BStringView`'s frame rectangle in the current high color.

* * *

### Member Functions

#### Archive()

`virtual status_t Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Calls the inherited version of [`Archive()`](BView.md#BView_Archive "Archive\(\)"), then adds the string and its alignment to the [`BMessage`](BMessage.md "BMessage") archive.

####  SetAlignment(), Alignment()

`void SetAlignment(alignment flag);``alignment Alignment() const;`

These functions align the string within the `BStringView`'s frame rectangle and return the current alignment. The alignment flag can be:

Constant| Description  
---|---  
`B_ALIGN_LEFT`| The string is aligned at the left side of the frame rectangle.  
`B_ALIGN_RIGHT`| The string is aligned at the right side of the frame rectangle.  
`B_ALIGN_CENTER`| The string is aligned so that the center of the string falls midway between the left and right sides of the frame rectangle.  
  
The default is `B_ALIGN_LEFT`.

####  SetText(), Text()

`void SetText(const char* string);``const char* Text() const;`

These functions set and return the text string that the `BStringView` draws. `SetText()` frees the previous string and copies string to replace it. `Text()` returns the null-terminated string.

* * *

### Static Functions

#### Instantiate()

See [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)")

* * *

### Archived Fields

The [`Archive()`](BStringView.md#BStringView_Archive "Archive\(\)") function adds the following fields to its [`BMessage`](BMessage.md "BMessage") argument:

Field| Type code| Description  
---|---|---  
`_text`| `B_STRING_TYPE`| Text displayed in the view (present only if it is not `NULL`).  
`_align`| `B_INT32_TYPE`| Alignment of text in frame.

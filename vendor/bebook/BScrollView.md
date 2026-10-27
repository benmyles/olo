<!-- Classes And Methods - The Interface Kit - BScrollView -->

## BScrollView

| Derived From:| [`BView`](BView.md "BView")  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/ScrollView.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BScrollView_Overview.md "BScrollView")

  * [Constructor and Destructor](BScrollView.md#BScrollView_ConstructorDestructor)
  * [Hook Functions](BScrollView.md#BScrollView_HookFunctions)
  * [Member Functions](BScrollView.md#BScrollView_MemberFunctions)
  * [Static Functions](BScrollView.md#BScrollView_StaticFunctions)
  * [Archived Fields](BScrollView.md#BScrollView_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BScrollView()

`BScrollView(const char* name,  
[`BView`](BView.md "BView")* target,  
uint32 resizingMode = B_FOLLOW_LEFT | B_FOLLOW_TOP,  
uint32 flags = 0,  
bool horizontal = false,  
bool vertical = false,  
border_style border = B_FANCY_BORDER);` `BScrollView([`BMessage`](BMessage.md "BMessage")* archive);`

Creates a `BScrollView` that has `target` as its target view. If `horizontal` is `true`, a horizontal scroll bar is added to the bottom of the target view. If `vertical` is `true`, a vertical scroll bar is added to the target's right side. The `BScrollView`'s frame is manufactured for you to contain the target view, the scroll bars, and the optional border; the location of the target view doesn't change. The scroll bar "thickness" constants, defined in `ScrollBar.h`, will help you compute the size of the frame:

Constant| Description  
---|---  
`B_V_SCROLL_BAR_WIDTH`| The width of a vertical scroll bar.  
`B_H_SCROLL_BAR_HEIGHT`| The height of a horizontal scroll bar.  
  
The target and the scroll bars are added as (sibling) children of the scroll view, and the target's [`TargetedByScrollView()`](BView.md#BView_TargetedByScrollView "TargetedByScrollView\(\)") function is called. The target must not already be the child of some other view.

The border argument can be one of three values:

Constant| Description  
---|---  
`B_PLAIN_BORDER`| Draw a border consisting of just a simple line around the target view and scroll bars.  
`B_FANCY_BORDER`| Draw a fancier border that looks like a 3D groove inset into the surface of the view.  
`B_NO_BORDER`| Don't draw a border.  
  
If the resize mode of the target view is `B_FOLLOW_ALL_SIDES`, it and the scroll bars will be automatically resized to fill the scroll view whenever the scroll view is resized.

The scroll bars created by the `BScrollView` have an initial range of 0 to 1000. Use the [`ScrollBar()`](BScrollView.md#BScrollView_ScrollBar "ScrollBar\(\)") function to retrieve the scroll bars and reset their ranges.

The `name`, `resizingMode`, and `flags` arguments are identical to those declared in the [`BView`](BView.md "BView") class and are passed to the [`BView`](BView.md "BView") constructor. If a border is requested, `B_WILL_DRAW` is automatically added to the flags mask. The other two arguments are passed to the [`BView`](BView.md "BView") class unchanged.

See also: The [`BView`](BView.md "BView") [constructor](BView.md#BView_Constructor "BView\(\)"), [`TargetedByScrollView()`](BView.md#BView_TargetedByScrollView "TargetedByScrollView\(\)")

#### ~BScrollView()

`virtual ~BScrollView();`

Does nothing.

* * *

### Hook Functions

#### AttachedToWindow()

`virtual void AttachedToWindow();`

Resizes scroll bars belonging to `BScrollView`s that occupy the right bottom corner of a document window (`B_DOCUMENT_WINDOW`) so that room is left for the resize knob. This function assumes that vertical scroll bars are `B_V_SCOLL_BAR_WIDTH` units wide and horizontal scroll bars are `B_H_SCROLL_BAR_HEIGHT` units high. It doesn't check to make sure the window is actually resizable.

This function also sets the default high color to a medium shade of gray.

See also: [`BView::AttachedToWindow()`](BView.md#BView_AttachedToWindow "AttachedToWindow\(\), AllAttached\(\)")

#### Draw()

`virtual void Draw([`BRect`](BRect.md "BRect") updateRect);`

Draws the border around the target view and scroll views, provided a border was requested when the `BScrollView` was constructed.

See also: The `BScrollView` [constructor](BScrollView.md#BScrollView_Constructor "BScrollView\(\)"), [`BView::Draw()`](BView.md#BView_Draw "Draw\(\)")

* * *

### Member Functions

#### Archive()

`virtual status_t Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Calls the inherited version of [`Archive()`](BView.md#BView_Archive "Archive\(\)"), which will archive the target view and scroll bars if the `deep` flag is `true`. This function then adds the `BScrollView`'s border style to the archive message.

See also: [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)"), [`Instantiate()`](BScrollView.md#BScrollView_Instantiate "Instantiate\(\)") static function

#### ScrollBar()

`[`BScrollBar`](BScrollBar.md "BScrollBar")* ScrollBar(orientation posture) const;`

Returns the horizontal scroll bar if posture is `B_HORIZONTAL` and the vertical scroll bar if posture is `B_VERTICAL`. If the `BScrollView` doesn't contain a scroll bar with the requested orientation, this function returns `NULL`.

See also: The [`BScrollBar`](BScrollBar.md "BScrollBar") class

####  SetBorder(), Border()

`virtual void SetBorder(border_style border);``border_style Border() const;`

These functions set and return the style of the `BScrollView`'s border, which may be `B_PLAIN_BORDER`, `B_FANCY_BORDER`, or `B_NO_BORDER`. The border is originally set by the constructor. The three constants and the border's effect on the `BScrollView` are discussed there.

See also: The `BScrollView` [constructor](BScrollView.md#BScrollView_Constructor "BScrollView\(\)")

####  SetBorderHighlighted(), IsBorderHighlighted()

`virtual status_t SetBorderHighlighted(bool highlighted);``bool IsBorderHighlighted() const;`

`SetBorderHighlighted()` highlights the border of the `BScrollView` when the `highlighted` flag is `true`, and removes the highlighting when the flag is `false`. This function works by calling [`Draw()`](BScrollView.md#BScrollView_Draw "Draw\(\)"). However, it works only for a border in the `B_FANCY_BORDER` style. If successful, it returns `B_OK`. Otherwise, it returns `B_ERROR`.

`IsBorderHighlighted()` returns whether the border is currently highlighted. The return value is always `false` for a `BScrollView` that doesn't have a border or has only a "plain" one.

Highlighting a `BScrollView`'s border shows that the target view is the current focus view for the window. Typically, the target view calls `SetBorderHighlighted()` from its [`MakeFocus()`](BView.md#BView_MakeFocus "MakeFocus\(\)") function when the focus changes. (The target knows that it's inside a `BScrollView` because of the [`TargetedByScrollView()`](BView.md#BView_TargetedByScrollView "TargetedByScrollView\(\)") notification it received.)

* * *

### Static Functions

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Returns a new `BScrollView` object, allocated by new and created with the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive. However, if the message doesn't contain data for an archived `BScrollView` object, `Instantiate()` returns `NULL`.

[`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"), [`Archive()`](BScrollBar.md#BScrollBar_Archive "Archive\(\)")

* * *

### Archived Fields

The [`Archive()`](BScrollView.md#BScrollView_Archive "Archive\(\)") function adds the following fields to its [`BMessage`](BMessage.md "BMessage") argument:

Field| Type code| Description  
---|---|---  
`_style`| `B_INT32_TYPE`| Border type.  
  
The following views are added to the `_views` field (deep copy only):

Field| Type code| Description  
---|---|---  
`target`| 0| Target view for the object.  
`_VSB_`| 0| Vertical [`BScrollBar`](BScrollBar.md "BScrollBar").  
`_HSB_`| 0| Horizontal [`BScrollBar`](BScrollBar.md "BScrollBar").  
  
Some of these fields may not be present if the setting they represent isn't used, or is the default value. For example, if the border type is `B_FANCY_BORDER`, the `_style` field won't be found in the archive.

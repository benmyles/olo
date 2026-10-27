<!-- Classes And Methods - The Interface Kit - BTabView -->

## BTabView  
  
| Derived From:| [`BView`](BView.md "BView")  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/TabView.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BTabView_Overview.md "BTabView")

  * [Constructor and Destructor](BTabView.md#BTabView_ConstructorDestructor)
  * [Hook Functions](BTabView.md#BTabView_HookFunctions)
  * [Member Functions](BTabView.md#BTabView_MemberFunctions)
  * [Static Functions](BTabView.md#BTabView_StaticFunctions)
  * [Archived Fields](BTabView.md#BTabView_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BTabView()

`BTabView([`BRect`](BRect.md "BRect") frame,  
const char* name,  
button_width width = B_WIDTH_AS_USUAL,  
uint32 resizingMode = B_FOLLOW_ALL,  
uint32 flags = B_FULL_UPDATE_ON_RESIZE | B_WILL_DRAW | B_NAVIGABLE | B_NAVIGABLE_JUMP | B_FRAME_EVENTS);` `BTabView(const [`BTabView`](BTabView.md "BTabView")& tabView);` `BTabView([`BMessage`](BMessage.md "BMessage")* archive);`

Initializes the `BTabView` to the `frame` rectangle, stated in its eventual parent's coordinate system, and assigns it the specified `name`, `resizingMode`, and `flags`. These are described in detail in the [`BView`](BView.md "BView") section.

The `width` parameter, which can be one of the following values, specifies the widths of the tabs in the `BTabView`. All tabs in a `BTabView` are the same width.

Constant| Description  
---|---  
`B_WIDTH_FROM_WIDEST`| Each tab's width is determined based upon the width of the widest tab name.  
`B_WIDTH_AS_USUAL`| The default tab width is used for all tabs.  
`B_WIDTH_FROM_LABEL`| The tab width for each tab is based on the width of the tab's label.  
  
The container view for the `BTabView`'s subviews is created and added to the `BTabView` as well.

#### ~BTabView()

`virtual ~BTabView();`

Frees all memory the `BTabView` allocated, and deletes every [`BTab`](BTab.md "BTab") attached to it.

* * *

### Hook Functions

#### AttachedToWindow()

`virtual void AttachedToWindow();`

Calls [`BView::AttachedToWindow()`](BView.md#BView_AttachedToWindow "AttachedToWindow\(\), AllAttached\(\)"), then selects the first tab in the tab view.

#### Draw()

`virtual void Draw([`BRect`](BRect.md "BRect") updateRect);`

Draws the tabs and the box that encloses the container view that displays the selected tab's target view.

#### DrawBox()

`virtual void DrawBox([`BRect`](BRect.md "BRect") selTabRect);`

Draws the box that encloses the container view. `selTabRect` is the frame rectangle of the currently-selected tab; this information is used to allow the box to attach properly to the current tab. This is the same rectangle that the [`DrawTabs()`](BTabView.md#BTabView_DrawTabs "DrawTabs\(\)") function returns.

This is called for you by the [`Draw()`](BTabView.md#BTabView_Draw "Draw\(\)") function and is provided primarily as a hook for customizing the appearance of your `BTabView`.

#### DrawTabs()

`virtual [`BRect`](BRect.md "BRect") DrawTabs();`

Draws all the tabs in the `BTabView` and returns the frame rectangle of the currently-selected tab. This rectangle should then be passed to [`DrawBox()`](BTabView.md#BTabView_DrawBox "DrawBox\(\)") to draw the container view's enclosing box.

This is called for you by the [`Draw()`](BTabView.md#BTabView_Draw "Draw\(\)") function and is provided primarily as a hook for customizing the appearance of your `BTabView`.

#### KeyDown()

`virtual void KeyDown(const char* bytes,  
int32 numBytes);`

The `KeyDown()` function handles keyboard navigation of the `BTabView`; the down and left arrow keys move the focus to the left, and the up and right arrow keys move the focus to the right. The space bar and enter keys select the focused tab.

All other keys are passed through to the [`BView::KeyDown()`](BView.md#BView_KeyDown "KeyDown\(\)") function for further processing.

See also: the Keyboard Information appendix, "B_KEY_DOWN" in the Message Protocols appendix, [`BView::KeyDown()`](BView.md#BView_KeyDown "KeyDown\(\)"), [`modifiers()`](TheInputServer_Functions.md#modifiers "modifiers\(\)")

#### MouseDown()

`virtual void MouseDown([`BPoint`](BPoint.md "BPoint") point);`

Identifies which tab (if any) the user clicked and selects that tab. If the mouse was not inside a tab when clicked, the [`BView::MouseDown`](BView.md#BView_MouseDown "MouseDown\(\)") function is called.

See also: "B_MOUSE_DOWN" in the Message Protocols appendix, [`BView::GetMouse()`](BView.md#BView_GetMouse "GetMouse\(\)")

#### WindowActivated()

`virtual void WindowActivated(bool active);`

Calls the inherited version of [`WindowActivated()`](BView.md#BView_WindowActivated "WindowActivated\(\)"), then calls [`DrawTabs()`](BTabView.md#BTabView_DrawTabs "DrawTabs\(\)") to redraw the tabs.

See also: [`BWindow::WindowActivated()`](BWindow.md#BWindow_WindowActivated "WindowActivated\(\)")

* * *

### Member Functions

####  AddTab(), RemoveTab()

`virtual void AddTab([`BView`](BView.md "BView")* target,  
[`BTab`](BTab.md "BTab")* tab = NULL);``virtual [`BTab`](BTab.md "BTab")* RemoveTab(int32 tab_index);`

`AddTab()` adds the specified `tab` as the rightmost tab in the `BTabView`. The new tab's target view is set to `target`.

If `tab` is `NULL`, a new [`BTab`](BTab.md "BTab") object is constructed and added to the `BTabView`. You can get a pointer to the new tab using the [`TabAt()`](BTabView.md#BTabView_TabAt "TabAt\(\)") function

if you choose to reimplement `AddTab()`, you should call the inherited form of this function once the [`BTab`](BTab.md "BTab") has been customized.

`RemoveTab()` removes the tab with the specified `index` number from the `BTabView` and returns a pointer to the [`BTab`](BTab.md "BTab") object. The [`BTab`](BTab.md "BTab") is not deleted—if you don't need it anymore, you can do that yourself.

#### Archive()

`virtual Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Calls the inherited version of [`Archive()`](BView.md#BView_Archive "Archive\(\)") and stores the `BTabView` in the [`BMessage`](BMessage.md "BMessage") archive.

See also: [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)"), [`Instantiate()`](BTabView.md#BTabView_Instantiate "Instantiate\(\)") static function

#### MakeFocus()

`virtual void MakeFocus(bool focused = true);`

Makes the `BTabView` the current focus by first calling [`BView::MakeFocus()`](BView.md#BView_MakeFocus "MakeFocus\(\)"), then making the currently-selected tab the focus of the `BTabView`.

####  Select(), Selection()

`virtual void Select(int32 tab);``int32 Selection() const;`

`Select()` selects the tab specified by its index number, first deselecting the previously-selected tab.

`Selection()` returns the index number of the currently-selected tab.

####  SetFocusTab() , FocusTab()

`virtual void SetFocusTab(int32 tab,  
bool focused);``int32 FocusTab() const;`

`SetFocusTab()` sets the focus state of the specified tab. If `focused` is `true`, the specified tab becomes the new focus; if `focused` is `false`, the focus is removed from the currently-focused tab but no new focus is set (the `BTabView` becomes focusless).

`FocusTab()` returns the index number of the tab that is currently the focus.

####  SetTabHeight(), TabHeight()

`virtual void SetTabHeight(float height);``float TabHeight() const;`

`SetTabHeight()` sets the height of the tabs in the `BTabView`. `TabHeight()` returns the current tab height.

When you change the tab height, the container view for the target views is resized so that the `BTabView` doesn't change size. Making the tabs taller by _`n`_ pixels causes the container view's top edge to move down by _`n`_ pixels, and decreasing the heights of the tabs increases the height of the container view.

####  SetTabWidth(), TabWidth()

`virtual void SetTabWidth(button_width width);``button_width TabWidth() const;`

`SetTabWidth()` sets the width of the tabs in the `BTabView`, and `TabWidth()` returns the current width setting. `width` must be one of the following values:

Constant| Description  
---|---  
`B_WIDTH_FROM_WIDEST`| Each tab's width is determined based upon the width of the widest tab name.  
`B_WIDTH_AS_USUAL`| The default tab width is used for all tabs.  
  
#### TabAt()

`virtual [`BTab`](BTab.md "BTab")* TabAt(int32 tab_index);`

Returns a pointer to the [`BTab`](BTab.md "BTab") object at the specified index. The leftmost tab is index number 0.

#### TabFrame()

`virtual [`BRect`](BRect.md "BRect") TabFrame(int32 tab_index);`

Returns the frame rectangle of the tab whose index number is specified. The leftmost tab is index number 0.

See also: [`DrawTabs()`](BTabView.md#BTabView_DrawTabs "DrawTabs\(\)"), [`DrawBox()`](BTabView.md#BTabView_DrawBox "DrawBox\(\)"), [`Draw()`](BTabView.md#BTabView_Draw "Draw\(\)")

* * *

### Static Functions

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Returns a new `BTabView` object, allocated by new and created with the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive. However, if the `archive` message doesn't contain data for a `BTabView` object, `Instantiate()` returns `NULL`.

See also: [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"), [`Archive()`](BTabView.md#BTabView_Archive "Archive\(\)")

* * *

### Archived Fields

The [`Archive()`](BTabView.md#BTabView_Archive "Archive\(\)") function adds the following fields to its [`BMessage`](BMessage.md "BMessage") argument:

Field| Type code| Description  
---|---|---  
`_high`| `B_FLOAT_TYPE`| The height of the tabs.  
`_but_width`| `B_INT16_TYPE`| The tab width value.  
`_sel`| `B_INT32_TYPE`| Which tab is selected.  
  
The archived tabs are added to the `_l_items` field (deep copy only). The tabs' target views are added to the `_views` field (deep copy only), with the leftmost tab inserted first in the `_views` array, and the rightmost tab inserted last.

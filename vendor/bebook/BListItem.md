<!-- Classes And Methods - The Interface Kit - BListItem -->

## BListItem

| Derived From:| –  
---|---  
Mix-in Classes:| [`BArchivable`](BArchivable.md "BArchivable")  
Declared In:| `interface/ListItem.h`  
Library:| `libbe.so`  
Allocation:| Constructor only  
[Class Overview](BListItem_Overview.md "BListItem")

  * [Constructor and Destructor](BListItem.md#BListItem_ConstructorDestructor)
  * [Hook Functions](BListItem.md#BListItem_HookFunctions)
  * [Member Functions](BListItem.md#BListItem_MemberFunctions)
  * [Archived Fields](BListItem.md#BListItem_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BListItem()

`BListItem(uint32 level = 0,  
bool expanded = true);` `BListItem([`BMessage`](BMessage.md "BMessage")* archive);`

The constructors create a new `BListItem` object. The level and expanded arguments are only used if the item is added to a [`BOutlineListView`](BOutlineListView.md "BOutlineListView") object; see [`BOutlineListView::AddItem()`](BOutlineListView.md#BOutlineListView_AddItem "AddItem\(\), AddUnder\(\)") for more information.

#### ~BListItem()

`~BListItem();`

The destructor is empty.

* * *

### Hook Functions

#### DrawItem()

`virtual void DrawItem([`BView`](BView.md "BView")* owner,  
[`BRect`](BRect.md "BRect") itemRect,  
bool drawEverything = false) = 0;`

Hook function that's invoked when the item's owner (a [`BListView`](BListView.md "BListView") object) needs to draw this item. `itemRect` is the area within the owner to which the item's drawing is clipped. If `drawEverything` is `true`, this function should touch every pixel in `itemRect`; if it's `false`, the function can assume that the background color for the item is already painted. `DrawItem()` should be implemented to visually reflect the state of the item, highlighting it if it's selected, dimming it if it's disabled, and so on.

The owner view provides the drawing environment for this item. All drawing instructions should be invoked on owner. See "[Creating a Custom List Item](BListItem_Overview.md#BListItem_Overview_Custom "Creating a Custom List Item")" for an example `DrawItem()` function.

#### Update()

`virtual void Update([`BView`](BView.md "BView")* owner,  
const [`BFont`](BFont.md "BFont")* font);`

Hook function that's called whenever the item's `owner` changes in a way that could affect the appearance of this item. It's always called when the item is added to a list view.

The default implementation sets the item's width to that of the owner, and sets its height so it will accommodate `font` (the owner's current [`BFont`](BFont.md "BFont") setting).

* * *

### Member Functions

#### OutlineLevel()

`uint32 OutlineLevel() const;`

Returns the outline level of the item; this is only meaningful if the item is in a [`BOutlineListView`](BOutlineListView.md "BOutlineListView").

####  Select(), Deselect(), IsSelected()

`void Select();``void Deselect();``bool IsSelected() const;`

`Select()` and `Deselect()` set the item's selected state; `IsSelected()` returns the state. The setting functions don't automatically update the item's display: After calling `Select()` or `Deselect()`, you must tell the owner to redrawn this item (see [`BListView::InvalidateItem()`](BListView.md#BListView_InvalidateItem "InvalidateItem\(\)")).

Also, `Select()` doesn't deselect the current selection. If this item is part of a single-selection list view, you have to deselect the current selection yourself.

Note

![Note](./images/admonitions/Info_32.png)

If possible, you should use [`BListView`](BListView.md "BListView")'s versions of these functions (see [`BListView::Select()`](BListView.md#BListView_Select "Select\(\)"). They update the view and manage the selection for you.

####  SetEnabled(), IsEnabled()

`void SetEnabled(bool enabled);``bool IsEnabled() const;`

`SetEnabled()` sets the list item's enabled state; `IsEnabled()` returns the state. A disabled item can't be selected by the user or by [`BListView::Select()`](BListView.md#BListView_Select "Select\(\)") , but it can be selected through [`BListItem::Select()`](BListItem.md#BListItem_Select "Select\(\), Deselect\(\), IsSelected\(\)"). If an item is already selected, `SetEnabled`(`false`) doesn't deselect it.

After calling `SetEnabled()`, you must tell the owner list view to redraw the item (see [`BListView::InvalidateItem()`](BListView.md#BListView_InvalidateItem "InvalidateItem\(\)")).

Note that [`BListView`](BListView.md "BListView") doesn't provide smart versions of these functions (as it does for `Select()`).

####  SetExpanded(), IsExpanded()

`void SetExpanded(bool expanded);``bool IsExpanded() const;`

These functions set and return the item's expanded state. This is only meaningful if the item is part of a [`BOutlineListView`](BOutlineListView.md "BOutlineListView"). The item is not automatically redrawn; you must tell the [`BOutlineListView`](BOutlineListView.md "BOutlineListView") to redraw the item (and its sublist) through [`BListView::InvalidateItem()`](BListView.md#BListView_InvalidateItem "InvalidateItem\(\)").

Note

![Note](./images/admonitions/Info_32.png)

If possible, you should use [`BOutlineListView`](BOutlineListView.md "BOutlineListView")'s [`Expand()`](BOutlineListView.md#BOutlineListView_Expand) and [`Collapse()`](BOutlineListView.md#BOutlineListView_Collapse "Collapse\(\), Expand\(\)") functions instead of `SetExpanded()`. The [`BOutlineListView`](BOutlineListView.md "BOutlineListView") functions redraw the affected part of the list for you.

####  SetHeight(), SetWidth(), Height(), Width()

`void SetHeight(float height);``void SetWidth(float height);``float Height() const;``float Width() const;`

These functions set and return the width and height of the item. The item's dimensions are adjusted when [`Update()`](BListItem.md#BListItem_Update "Update\(\)") is called.

* * *

### Archived Fields

The `Archive()` function adds the following fields to its [`BMessage`](BMessage.md "BMessage") argument:

Field| Type code| Description  
---|---|---  
`_sel`| `B_BOOL_TYPE`| `true` if the item is selectable.  
`_disable`| `B_BOOL_TYPE`| `true` if the item is disabled.  
`_li_expanded`| `B_BOOL_TYPE`| `true` if the item is expanded.  
`_li_outline_level`| `B_INT32_TYPE`| The outline level of the item.  
  
Some of these fields may not be present if the setting they represent isn't used, or is the default value. For example, if the item is not selectable, the `_sel` field won't be found in the archive.

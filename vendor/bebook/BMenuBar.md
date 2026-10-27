<!-- Classes And Methods - The Interface Kit - BMenuBar -->

## BMenuBar

| Derived From:| [`BMenu`](BMenu.md "BMenu")  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/MenuBar.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BMenuBar_Overview.md "BMenuBar")

  * [Constructor and Destructor](BMenuBar.md#BMenuBar_ConstructorDestructor)
  * [Hook Functions](BMenuBar.md#BMenuBar_HookFunctions)
  * [Member Functions](BMenuBar.md#BMenuBar_MemberFunctions)
  * [Static Functions](BMenuBar.md#BMenuBar_StaticFunctions)
  * [Archived Fields](BMenuBar.md#BMenuBar_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BMenuBar()

`BMenuBar([`BRect`](BRect.md "BRect") frame,  
const char* name,  
uint32 resizingMode = B_FOLLOW_LEFT_RIGHT | B_FOLLOW_TOP,  
menu_layout layout = B_ITEMS_IN_ROW,  
bool resizeToFit = true);` `BMenuBar([`BMessage`](BMessage.md "BMessage")* archive);`

Initializes the `BMenuBar` by assigning it a `frame` rectangle, a `name`, and a `resizingMode`, just like other [`BView`](BView.md "BView")s. These values are passed up the inheritance hierarchy to the [`BView`](BView.md "BView") [`constructor`](BView.md#BView_Constructor "BView\(\)"). The default resizing mode (`B_FOLLOW_LEFT_RIGHT` plus `B_FOLLOW_TOP`) is designed for a true menu bar (one that's displayed along the top edge of a window). It permits the menu bar to adjust itself to changes in the window's width, while keeping it glued to the top of the window frame.

The layout argument determines how items are arranged in the menu bar. By default, they're arranged in a row as befits a true menu bar. If an instance of this class is being used to implement something other than a horizontal menu, items can be laid out in a column ( `B_ITEMS_IN_COLUMN` ) or in a matrix ( `B_ITEMS_IN_MATRIX` ).

If the `resizeToFit` flag is turned on, as it is by default, the frame rectangle of the `BMenuBar` will be automatically resized to fit the items it displays. This is generally a good idea, since it relieves you of the responsibility of testing user preferences to determine what size the menu bar should be. Because the font and font size for menu items are user preferences, items can vary in size from user to user.

When `resizeToFit` is `true`, the frame rectangle determines only where the menu bar is located, not how large it will be. The rectangle's left and top data members are respected, but the right and bottom sides are adjusted to accommodate the items that are added to the menu bar.

Two kinds of adjustments are made if the layout is `B_ITEMS_IN_ROW`, as it typically is for a menu bar:

  * The height of the menu bar is adjusted to the height of a single item.

  * If the `resizingMode` includes `B_FOLLOW_LEFT_RIGHT`, the width of the menu bar is adjusted to match the width of its parent view. This means that a true menu bar (one that's a child of the window's top view) will always be as wide as the window.

Two similar adjustments are made if the menu bar layout is `B_ITEMS_IN_COLUMN`:

  * The width of the menu bar is adjusted to the width of the widest item.

  * If the `resizingMode` includes `B_FOLLOW_TOP_BOTTOM`, the height of the menu bar is adjusted to match the height of its parent view.

After setting up the key menu bar and adding items to it, you may want to set the minimum width of the window so that certain items won't be hidden when the window is resized smaller.

Change the `resizingMode`, the `layout`, and the `resizeToFit` flag as needed for `BMenuBar`s that are used for a purpose other than to implement a true menu bar.

See also: the [`BMenu`](BMenu.md "BMenu") [constructor](BMenu.md#BMenu_Constructor "BMenu\(\)"), [`BWindow::SetSizeLimits()`](BWindow.md#BWindow_SetSizeLimits "SetSizeLimits\(\), GetSizeLimits\(\), SetZoomLimits\(\)")

#### ~BMenuBar()

`virtual ~BMenuBar();`

Frees all the items and submenus in the entire menu hierarchy, and all memory allocated by the `BMenuBar`.

* * *

### Hook Functions

#### AttachedToWindow()

`virtual void AttachedToWindow();`

Finishes the initialization of the `BMenuBar` by setting up the whole hierarchy of menus that it controls, and by making the [`BWindow`](BWindow.md "BWindow") to which it has become attached the target handler for all items in the menu hierarchy, except for those items for which a target has already been set.

This function also makes the `BMenuBar` the key menu bar, the `BMenuBar` object whose menu hierarchy the user can navigate from the keyboard. If a window contains more than one `BMenuBar` in its view hierarchy, the last one that's added to the window gets to keep this designation. However, the key menu bar should always be the real menu bar at the top of the window. It can be explicitly set with [`BWindow`](BWindow.md "BWindow")'s [`SetKeyMenuBar()`](BWindow.md#BWindow_SetKeyMenuBar) function.

#### Draw()

`virtual void Draw([`BRect`](BRect.md "BRect") updateRect);`

Draws the menu—whether as a true menu bar, as some other kind of menu list, or as a single item that controls a pop-up menu. This function is called as the result of update messages; you don't need to call it yourself.

See also: [`BView::Draw()`](BView.md#BView_Draw "Draw\(\)")

#### MouseDown()

`virtual void MouseDown([`BPoint`](BPoint.md "BPoint") point);`

Initiates mouse tracking and keyboard navigation of the menu hierarchy. This function is called to notify the `BMenuBar` of a mouse-down event.

See also: [`BView::MouseDown()`](BView.md#BView_MouseDown "MouseDown\(\)")

* * *

### Member Functions

#### Archive()

`virtual status_t Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Calls the inherited version of [`Archive()`](BMenu.md#BMenu_Archive "Archive\(\)") which serves to archive the `BMenuBar`'s current state and, if the `deep` flag is `true`, all its menu items. This function then adds the `BMenuBar`'s border style to the [`BMessage`](BMessage.md "BMessage") archive.

See also: [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)"), [`BMenu::Archive()`](BMenu.md#BMenu_Archive "Archive\(\)"), [`Instantiate()`](BMenuBar.md#BMenuBar_Instantiate "Instantiate\(\)") static function

####  Hide() , Show()

`virtual void Hide();``virtual void Show();`

These functions override their [`BMenu`](BMenu.md "BMenu") counterparts to restore the normal behavior for views when they're hidden and unhidden. When an ordinary [`BMenu`](BMenu.md "BMenu") is hidden, the window that displays it is also removed from the screen. But it would be a mistake to remove the window that displays a `BMenuBar`. Hiding a `BMenuBar` is like hiding a typical view; only the view is hidden, not the window.

See also: [`BView::Hide()`](BView.md#BView_Hide "Hide\(\), Show\(\)")

####  SetBorder(), Border()

`void SetBorder(menu_bar_border border);``menu_bar_border Border() const;`

`SetBorder()` determines how the menu list is bordered. The `border` argument can be any of three values:

Constant| Description  
---|---  
`B_BORDER_FRAME`| The border is drawn around the entire frame rectangle.  
`B_BORDER_CONTENTS`| The border is drawn around just the list of items.  
`B_BORDER_EACH_ITEM`| A border is drawn around each item.  
  
`Border()` returns the current setting. The default is `B_BORDER_FRAME`.

* * *

### Static Functions

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Returns a new `BMenuBar` object, allocated by new and created with the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive. However, if the archive message doesn't contain data for a `BMenuBar` object, this function returns `NULL`.

See also: [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"), [`Archive()`](BMenuBar.md#BMenuBar_Archive "Archive\(\)")

* * *

### Archived Fields

The [`Archive()`](BMenuBar.md#BMenuBar_Archive "Archive\(\)") function adds the following fields to its [`BMessage`](BMessage.md "BMessage") argument:

Field| Type code| Description  
---|---|---  
`_border`| `B_INT32_TYPE`| Menu bar border (exists only if not `B_BORDER_FRAME`)

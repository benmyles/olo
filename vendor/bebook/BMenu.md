<!-- Classes And Methods - The Interface Kit - BMenu -->

## BMenu

| Derived From:| [`BView`](BView.md "BView")  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/Menu.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BMenu_Overview.md "BMenu")

  * [Constructor and Destructor](BMenu.md#BMenu_ConstructorDestructor)
  * [Hook Functions](BMenu.md#BMenu_HookFunctions)
  * [Member Functions](BMenu.md#BMenu_MemberFunctions)
  * [Static Functions](BMenu.md#BMenu_StaticFunctions)
  * [Scripting Support](BMenu.md#BMenu_Scripting)
  * [Archived Fields](BMenu.md#BMenu_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BMenu()

`BMenu(const char* name,  
menu_layout layout = B_ITEMS_IN_COLUMN);` `BMenu(const char* name,  
float width,  
float height);` `BMenu([`BMessage`](BMessage.md "BMessage")* archive);` `protected BMenu([`BRect`](BRect.md "BRect") frame,  
const char* name,  
uint32 resizingMode,  
uint32 flags,  
menu_layout layout,  
bool resizeToFit);`

Initializes the `BMenu` object. The name of the object becomes the initial label of the supermenu item that controls the menu and brings it to the screen. (It's also the name that can be passed to [`BView`](BView.md "BView")'s [`FindView()`](BView.md#BView_FindView "FindView\(\)") function.)

A new `BMenu` object doesn't contain any items; you need to call [`AddItem()`](BMenu.md#BMenu_AddItem "AddItem\(\)") to set up its contents.

A menu can arrange its items in any of three ways:

Constant| Description  
---|---  
`B_ITEMS_IN_COLUMN`| The items are stacked vertically in a column, one on top of the other, as in a typical menu.  
`B_ITEMS_IN_ROW`| The items are laid out horizontally in a row, from end to end, as in a typical menu bar.  
`B_ITEMS_IN_MATRIX`| The items are arranged in a custom fashion, such as a matrix.  
  
Either `B_ITEMS_IN_ROW` or the default `B_ITEMS_IN_COLUMN` can be passed as the layout argument to the public constructor. (A column is the default for ordinary menus; a row is the default for [`BMenuBar`](BMenuBar.md "BMenuBar")s.) This version of the constructor isn't designed for `B_ITEMS_IN_MATRIX` layouts.

A `BMenu` object can arrange items that are laid out in a column or a row entirely on its own. The menu will be resized to exactly fit the items that are added to it.

However, when items are laid out in a custom matrix, the menu needs more help. First, the constructor must be informed of the exact width and height of the menu rectangle. The version of the constructor that takes these two parameters is designed just for matrix menus—it sets the layout to `B_ITEMS_IN_MATRIX`. Then, when items are added to the menu, the `BMenu` object expects to be informed of their precise positions within the specified area. The menu is not resized to fit the items that are added. Finally, when items in the matrix change, you must take care of any required adjustments in the layout yourself.

The protected version of the constructor is supplied for derived classes that don't simply devise different sorts of menu items or arrange them in a different way, but invent a different kind of menu. If the `resizeToFit` flag is `true`, it's expected that the layout will be `B_ITEMS_IN_COLUMN` or `B_ITEMS_IN_ROW`. The menu will resize itself to fit the items that are added to it. If the layout is `B_ITEMS_IN_MATRIX`, the `resizeToFit` flag should be `false`. 

#### ~BMenu()

`virtual ~BMenu();`

Deletes all the items that were added to the menu and frees all memory allocated by the `BMenu` object. Deleting the items serves also to delete any submenus those items control and, thus, the whole branch of the menu hierarchy.

* * *

### Hook Functions

#### AttachedToWindow()

`virtual void AttachedToWindow();`

Finishes initializing the `BMenu` object by laying out its items and resizing the `BMenu` view to fit. This function is called for you each time the `BMenu` is assigned to a window. For a submenu, that means each time the menu is shown on-screen.

See also: [`BView::AttachedToWindow()`](BView.md#BView_AttachedToWindow "AttachedToWindow\(\), AllAttached\(\)")

#### Draw()

`virtual void Draw([`BRect`](BRect.md "BRect") updateRect);`

Draws the menu. This function is called for you whenever the menu is placed on-screen or is updated while on-screen. It's not a function you need to call yourself.

See also: [`BView::Draw()`](BView.md#BView_Draw "Draw\(\)")

#### KeyDown()

`virtual void KeyDown(const char* bytes,  
int32 numBytes);`

Handles keyboard navigation through the menu. This function is called to respond to messages reporting key-down events. It should not be called from application code.

See also: [`BView::KeyDown()`](BView.md#BView_KeyDown "KeyDown\(\)")

* * *

### Member Functions

#### AddItem()

`bool AddItem([`BMenuItem`](BMenuItem.md "BMenuItem")* item);` `bool AddItem([`BMenuItem`](BMenuItem.md "BMenuItem")* item,  
int32 index);` `bool AddItem([`BMenuItem`](BMenuItem.md "BMenuItem")* item,  
[`BRect`](BRect.md "BRect") frame);` `bool AddItem([`BMenu`](BMenu.md "BMenu")* submenu);` `bool AddItem([`BMenu`](BMenu.md "BMenu")* submenu,  
int32 index);` `bool AddItem([`BMenu`](BMenu.md "BMenu")* submenu,  
[`BRect`](BRect.md "BRect") frame);`

Adds an item to the menu list at `index`—or, if no index is mentioned, to the end of the list. If items are arranged in a matrix rather than a list, it's necessary to specify the item's `frame` rectangle—the exact position where it should be located in the menu view. Assume a coordinate system for the menu that has the origin, (0.0, 0.0), at the left top corner of the view rectangle. The rectangle will have the width and height that were specified when the menu was constructed.

The versions of this function that take an `index` (even an implicit one) can be used only if the menu arranges items in a column or row (`B_ITEMS_IN_COLUMN` or `B_ITEMS_IN_ROW`); it's an error to use them for items arranged in a matrix. Conversely, the versions of this function that take a frame rectangle can be used only if the menu arranges items in a matrix (`B_ITEMS_IN_MATRIX`); it's an error to use them for items arranged in a list.

If a `submenu` is specified rather than an `item`, `AddItem()` constructs a controlling [`BMenuItem`](BMenuItem.md "BMenuItem") for the submenu and adds the item to the menu.

If it's unable to add the item to the menu—for example, if the index is out-of-range or the wrong version of the function has been called—`AddItem()` returns `false`. If successful, it returns `true`.

See also: the `BMenu` constructor, the [`BMenuItem`](BMenuItem.md "BMenuItem") class, [`RemoveItem()`](BMenu.md#BMenu_RemoveItem "RemoveItem\(\)")

#### AddSeparatorItem()

`bool AddSeparatorItem();`

Creates an instance of the [`BSeparatorItem`](BSeparatorItem.md "BSeparatorItem") class and adds it to the end of the menu list, returning `true` if successful and `false` if not (a very unlikely possibility). This function is a shorthand for:
    
    
    [BSeparatorItem](BSeparatorItem.md "BSeparatorItem")* separator = new [BSeparatorItem](BSeparatorItem.md "BSeparatorItem");
    AddItem(separator);

A separator serves only to separate other items in the list. It counts as an item and has an indexed position in the list, but it doesn't do anything. It's drawn as a horizontal line across the menu. Therefore, it's appropriately added only to menus where the items are laid out in a column.

See also: [`AddItem()`](BMenu.md#BMenu_AddItem "AddItem\(\)")

#### Archive()

`virtual status_t Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Calls the inherited version of [`Archive()`](BView.md#BView_Archive "Archive\(\)"), then archives the `BMenu` by recording its layout and all current settings in the [`BMessage`](BMessage.md "BMessage") `archive`. If the `deep` flag is `true`, all of the menu items are also archived.

See also: [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)"), [`Instantiate()`](BMenu.md#BMenu_Instantiate "Instantiate\(\)") static function

#### CountItems()

`int32 CountItems() const;`

Returns the total number of items in the menu, including separator items.

#### FindItem()

`[`BMenuItem`](BMenuItem.md "BMenuItem")* FindItem(const char* label) const;` `[`BMenuItem`](BMenuItem.md "BMenuItem")* FindItem(uint32 command) const;`

Returns the item with the specified `label`—or the one that sends a message with the specified `command`. If there's more than one item in the menu hierarchy with that particular `label` or associated with that particular `command`, this function returns the first one it finds. It recursively searches the menu by working down the list of items in order. If an item controls a submenu, it searches the submenu before returning to check any remaining items in the menu.

If none of the items in the menu hierarchy meet the stated criterion, `FindItem()` returns `NULL`.

#### FindMarked()

`[`BMenuItem`](BMenuItem.md "BMenuItem")* FindMarked();`

Returns the first marked item in the menu list (the one with the lowest index), or `NULL` if no item is marked.

See also: [`SetRadioMode()`](BMenu.md#BMenu_SetRadioMode "SetRadioMode\(\), IsRadioMode\(\)"), [`BMenuItem::SetMarked()`](BMenuItem.md#BMenuItem_SetMarked "SetMarked\(\), IsMarked\(\)")

####  Hide(), Show()

##### protected

`void Hide();`

`void Show(bool selectFirst);` `virtual void Show();`

These functions hide the menu (remove the `BMenu` view from the window it's in and remove the window from the screen) and show it (attach the `BMenu` to a window and place the window on-screen). If the `selectFirst` flag passed to `Show()` is `true`, the first item in the menu will be selected when it's shown. If `selectFirst` is `false`, the menu is shown without a selected item.

The version of `Show()` that doesn't take an argument simply calls the version that does and passes it a `selectFirst` value of `false`.

These functions are not ones that you'd ordinarily call, even when implementing a derived class. You'd need them only if you're implementing a nonstandard menu of some kind and want to control when the menu appears on-screen.

See also: [`BView::Show()`](BView.md#BView_Show), [`Track()`](BMenu.md#BMenu_Track "Track\(\)")

#### IndexOf()

`int32 IndexOf([`BMenuItem`](BMenuItem.md "BMenuItem")* item) const;` `int32 IndexOf(BMenu* submenu) const;`

Returns the index of the specified menu `item`—or the item that controls the specified `submenu`. Indices record the position of the item in the menu list. They begin at 0 for the item at the top of a column or at the left of a row and include separator items.

If the menu doesn't contain the specified `item`, or the item that controls `submenu`, the return value will be `B_ERROR`.

See also: [`AddItem()`](BMenu.md#BMenu_AddItem "AddItem\(\)")

#### InvalidateLayout()

`void InvalidateLayout();`

Forces the `BMenu` to recalculate the layout of all menu items and, consequently, its own size. It can do this only if the items are arranged in a row or a column. If the items are arranged in a matrix, it's up to you to keep their layout up-to-date.

All `BMenu` and [`BMenuItem`](BMenuItem.md "BMenuItem") functions that change an item in a way that might affect the overall menu automatically invalidate the menu's layout so it will be recalculated. For example, changing the label of an item might cause the menu to become wider (if it needs more room to accommodate the longer label) or narrower (if it no longer needs as much room as before).

Therefore, you don't need to call `InvalidateLayout()` after using a kit function to change a menu or menu item; it's called for you. You'd call it only when making some other change to a menu.

See also: the `BMenu` [constructor](BMenu.md#BMenu_Constructor "BMenu\(\)")

####  ItemAt(), SubmenuAt()

`[`BMenuItem`](BMenuItem.md "BMenuItem")* ItemAt(int32 index) const;``BMenu* SubmenuAt(int32 index) const;`

These functions return the item at `index`—or the submenu controlled by the item at `index`. If there's no item at the index, they return `NULL`. `SubmenuAt()` is a shorthand for:
    
    
    ItemAt(index)->Submenu()

It returns `NULL` if the item at `index` doesn't control a submenu.

See also: [`AddItem()`](BMenu.md#BMenu_AddItem "AddItem\(\)")

#### Layout()

##### protected

`menu_layout Layout() const;`

Returns `B_ITEMS_IN_COLUMN` if the items in the menu are stacked in a column from top to bottom, `B_ITEMS_IN_ROW` if they're stretched out in a row from left to right, or `B_ITEMS_IN_MATRIX` if they're arranged in some custom fashion. By default `BMenu` items are arranged in a column and [`BMenuBar`](BMenuBar.md "BMenuBar") items in a row.

The layout is established by the constructor.

See also: The `BMenu` [constructor](BMenu.md#BMenu_Constructor "BMenu\(\)") and [`BMenuBar`](BMenuBar.md "BMenuBar") [constructor](BMenuBar.md#BMenuBar_Constructor "BMenuBar\(\)").

#### RemoveItem()

`[`BMenuItem`](BMenuItem.md "BMenuItem")* RemoveItem(int32 index);` `bool RemoveItem([`BMenuItem`](BMenuItem.md "BMenuItem")* item);` `bool RemoveItem(BMenu* submenu);`

Removes the item at `index`, or the specified `item`, or the item that controls the specified `submenu`. Removing the item doesn't free it.

  * If passed an `index`, this function returns a pointer to the item so you can free it. It returns a `NULL` pointer if the item couldn't be removed (for example, if the `index` is out-of-range).

  * If passed an `item`, it returns `true` if the item was in the list and could be removed, and `false` if not.

  * If passed a `submenu`, it returns `true` if the submenu is controlled by an item in the menu and that item could be removed, and `false` otherwise.

When an item is removed from a menu, it loses its target; the cached value is set to `NULL`. If the item controls a submenu, it remains attached to the submenu even after being removed.

See also: [`AddItem()`](BMenu.md#BMenu_AddItem "AddItem\(\)")

#### ScreenLocation()

##### protected

`virtual [`BPoint`](BPoint.md "BPoint") ScreenLocation();`

Returns the point where the left top corner of the menu should appear when the menu is shown on-screen. The point is specified in the screen coordinate system.

This function is called each time a hidden menu (a submenu of another menu) is brought to the screen. It can be overridden in a derived class to change where the menu appears. For example, the [`BPopUpMenu`](BPopUpMenu.md "BPopUpMenu") class overrides it so that a pop-up menu pops up over the controlling item.

####  SetEnabled(), IsEnabled()

`virtual void SetEnabled(bool enabled);``bool IsEnabled() const;`

`SetEnabled()` enables the `BMenu` if the `enabled` flag is `true`, and disables it if `enabled` is `false`. If the menu is a submenu, this enables or disables its controlling item, just as if `SetEnabled()` were called for that item. The controlling item is updated so that it displays its new state, if it happens to be visible on-screen.

Disabling a menu disables its entire branch of the menu hierarchy. All items in the menu, including those that control other menus, are disabled.

`IsEnabled()` returns `true` if the `BMenu`, and every `BMenu` above it in the menu hierarchy, is enabled. It returns `false` if the `BMenu`, or any `BMenu` above it in the menu hierarchy, is disabled.

See also: [`BMenuItem::SetEnabled()`](BMenuItem.md#BMenuItem_SetEnabled "SetEnabled\(\), IsEnabled\(\)")

####  SetItemMargins(), GetItemMargins()

##### protected

`void SetItemMargins(float left,  
float top,  
float right,  
float bottom);``void GetItemMargins(float* left,  
float* top,  
float* right,  
float* bottom);`

These functions set and get the margins around each item in the `BMenu`. For the purposes of this function, you should assume that all items are enclosed in a rectangle of the same size, one big enough for the largest item. Keyboard shortcuts are displayed in the right margin and check marks in the left.

See also: [`SetMaxContentWidth()`](BMenu.md#BMenu_SetMaxContentWidth "SetMaxContentWidth\(\), MaxContentWidth\(\)")

####  SetLabelFromMarked() , IsLabelFromMarked()

##### protected

`void SetLabelFromMarked(bool flag);``bool IsLabelFromMarked();`

`SetLabelFromMarked()` determines whether the label of the item that controls the menu (the label of the superitem) should be taken from the currently marked item within the menu. If `flag` is `true`, the menu is placed in radio mode and the superitem's label is reset each time the user selects a different item. If `flag` is `false`, the setting for radio mode doesn't change and the label of the superitem isn't automatically reset.

`IsLabelFromMarked()` returns whether the superitem's label is taken from the marked item (but not necessarily whether the `BMenu` is in radio mode).

See also: [`SetRadioMode()`](BMenu.md#BMenu_SetRadioMode "SetRadioMode\(\), IsRadioMode\(\)")

####  SetMaxContentWidth(), MaxContentWidth()

`virtual void SetMaxContentWidth(float width);``float MaxContentWidth() const;`

These functions set and return the maximum width of an item's content area. The content area is where the item label is drawn; it excludes the margin on the left where a check mark might be placed and the margin on the right where a shortcut character or a submenu symbol might appear. The content area is the same size for all items in the menu.

Normally, a menu will be wide enough to accommodate its longest item. However, items wider than the maximum set by `SetMaxContentWidth()` are truncated to fit.

See also: [`SetItemMargins()`](BMenu.md#BMenu_SetItemMargins "SetItemMargins\(\), GetItemMargins\(\)"), [`BMenuItem::TruncateLabel()`](BMenuItem.md#BMenuItem_TruncateLabel "TruncateLabel\(\)")

####  SetRadioMode(), IsRadioMode()

`virtual void SetRadioMode(bool flag);``bool IsRadioMode();`

`SetRadioMode()` puts the `BMenu` in radio mode if `flag` is `true` and takes it out of radio mode if `flag` is `false`. In radio mode, only one item in the menu can be marked at a time. If the user selects an item, a check mark is placed in front of it automatically (you don't need to call [`BMenuItem`](BMenuItem.md "BMenuItem")'s [`SetMarked()`](BMenuItem.md#BMenuItem_SetMarked "SetMarked\(\), IsMarked\(\)") function; it's called for you). If another item was marked at the time, its mark is removed. Selecting a currently marked item retains the mark.

`IsRadioMode()` returns whether the `BMenu` is currently in radio mode. The default radio mode is `false` for ordinary `BMenu`s, but `true` for [`BPopUpMenu`](BPopUpMenu.md "BPopUpMenu")s.

`SetRadioMode()` doesn't change any of the items in the menu. If you want an initial item to be marked when the menu is put into radio mode, you must mark it yourself.

When `SetRadioMode()` turns radio mode off, it calls SetLabelFromMarked() and passes it an argument of `false`—turning off the feature that changes the label of the menu's superitem each time the marked item changes. Similarly, when [`SetLabelFromMarked()`](BMenu.md#BMenu_SetLabelFromMarked "SetLabelFromMarked\(\) , IsLabelFromMarked\(\)") turns on this feature, it calls `SetRadioMode()` and passes it an argument of `true`—turning radio mode on.

#### SetTargetForItems()

`virtual status_t SetTargetForItems([`BHandler`](BHandler.md "BHandler")* handler);` `virtual status_t SetTargetForItems([`BMessenger`](BMessenger.md "BMessenger") messenger);`

Assigns `handler` or `messenger` as the target for all the items in the menu. The proposed target is subject to the restrictions imposed by the `SetTarget()` function that [`BMenuItem`](BMenuItem.md "BMenuItem") inherits from [`BInvoker`](BInvoker.md "BInvoker") in the Application Kit. See that function for further information.

If it's unable to set the target of any item, `SetTargetForItems()` aborts and returns the error it encountered. If successful in setting the target of all items, it returns `B_OK`.

This function doesn't work recursively (it doesn't descend into submenus), and it only acts on items that are currently in the `BMenu` (it doesn't affect items that are added later).

####  SetTriggersEnabled(), AreTriggersEnabled()

`virtual void SetTriggersEnabled(bool flag);``bool AreTriggersEnabled() const;`

`SetTriggersEnabled()` enables the triggers for all items in the menu if `flag` is `true` and disables them if `flag` is `false`. `AreTriggersEnabled()` returns whether the triggers are currently enabled or disabled. They're enabled by default.

Triggers are displayed to the user only if they're enabled, and only when keyboard actions can operate the menu.

Triggers are appropriate for some menus, but not for others. `SetTriggersEnabled()` is typically called to initialize the `BMenu` when it's constructed, not to enable and disable triggers as the application is running. If triggers are ever enabled for a menu, they should always be enabled; if they're ever disabled, they should always be disabled.

See also: [`BMenuItem::SetTrigger()`](BMenuItem.md#BMenuItem_SetTrigger "SetTrigger\(\), Trigger\(\)")

####  Superitem(), Supermenu()

`BMenuItem* Superitem() const;``BMenu * Supermenu() const;`

These functions return the supermenu item that controls the `BMenu` and the supermenu where that item is located. The supermenu could be a [`BMenuBar`](BMenuBar.md "BMenuBar") object. If the `BMenu` hasn't been made the submenu of another menu, both functions return `NULL`.

See also: [`AddItem()`](BMenu.md#BMenu_AddItem "AddItem\(\)")

#### Track()

##### protected

`[`BMenuItem`](BMenuItem.md "BMenuItem")* Track(bool openAnyway = false,  
[`BRect`](BRect.md "BRect")* clickToOpenRect = NULL);`

Initiates tracking of the cursor within the menu. This function passes tracking control to submenus (and submenus of submenus) depending on where the user moves the mouse. If the user ends tracking by invoking an item, `Track()` returns the item. If the user didn't invoke any item, it returns `NULL`. The item doesn't have to be located in the `BMenu`; it could, for example, belong to a submenu of the `BMenu`.

If the `openAnyway` flag is `true`, `Track()` opens the menu and leaves it open even though a mouse button isn't held down. This enables menu navigation from the keyboard. If a `clickToOpenRect` is specified and the user has set the click-to-open preference, `Track()` will leave the menu open if the user releases the mouse button while the cursor is inside the rectangle. The rectangle should be stated in the screen coordinate system.

`Track()` is called by the `BMenu` to initiate tracking in the menu hierarchy. You would need to call it yourself only if you're implementing a different kind of menu that starts to track the cursor under nonstandard circumstances.

* * *

### Static Functions

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Returns a new `BMenu` object, allocated by new and created with the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive. However, if the `archive` message doesn't contain data for a `BMenu` object, `Instantiate()` returns `NULL`.

See also: [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"), [`Archive()`](BMenu.md#BMenu_Archive "Archive\(\)")

* * *

### Scripting Support

The `BMenu` class implements the suite called "suite/vnd.Be-menu" consisting of the following messages:

#### The Enabled Property

The "Enabled" property reflects whether the menu or menu item is enabled or disabled.

Message| Specifiers| Description  
---|---|---  
`B_GET_PROPERTY`| `B_DIRECT_SPECIFIER`| Returns `true` if menu or menu item is enabled; `false` otherwise.  
`B_SET_PROPERTY`| `B_DIRECT_SPECIFIER`| Enables or disables menu or menu item.  
  
#### The Label Property

The "Label" property refers to the text label of a menu or menu item.

Message| Specifiers| Description  
---|---|---  
`B_GET_PROPERTY`| `B_DIRECT_SPECIFIER`| Returns the string label of the menu or menu item.  
`B_SET_PROPERTY`| `B_DIRECT_SPECIFIER`| Sets the string label of the menu or menu item.  
  
#### The Mark Property

The "Mark" property refers to whether or not a given menu item or a given menu's superitem is marked.

Message| Specifiers| Description  
---|---|---  
`B_GET_PROPERTY`| `B_DIRECT_SPECIFIER`| Returns `true` if the menu item or the menu's superitem is marked; `false` otherwise.  
`B_SET_PROPERTY`| `B_DIRECT_SPECIFIER`| Marks or unmarks the menu item or the menu's superitem.  
  
#### The Menu Property

The "Menu" property refers to individual `BMenu`s in the menu.

Message| Specifiers| Description  
---|---|---  
`B_CREATE_PROPERTY`| `B_NAME_SPECIFIER`,  
`B_INDEX_SPECIFIER`,  
`B_REVERSE_INDEX_SPECIFIER`| Adds a new menu item at the specified index with the text label found in "data" and the int32 command found in "what" (used as the `what` field in the [`BMessage`](BMessage.md "BMessage") sent by the item).  
`B_DELETE_PROPERTY`| `B_NAME_SPECIFIER`,  
`B_INDEX_SPECIFIER`,  
`B_REVERSE_INDEX_SPECIFIER`| Removes the selected menu or menus.  
any other| `B_NAME_SPECIFIER`, `B_INDEX_SPECIFIER`, `B_REVERSE_INDEX_SPECIFIER`| Directs scripting message to the specified menu, first popping the current specifier off the stack.  
  
#### The MenuItem Property

The "MenuItem" property refers to individual `BMenu`Items in the menu.

Message| Specifiers| Description  
---|---|---  
`B_COUNT_PROPERTIES`| `B_DIRECT_SPECIFIER`| Counts the number of menu items in the specified menu.  
`B_CREATE_PROPERTY`| `B_NAME_SPECIFIER`,  
`B_INDEX_SPECIFIER`,  
`B_REVERSE_INDEX_SPECIFIER`| Adds a new menu item at the specified index with the text label found in "data" and the int32 command found in "what" (used as the `what` field in the [`BMessage`](BMessage.md "BMessage") sent by the item).  
`B_DELETE_PROPERTY`| `B_NAME_SPECIFIER`,  
`B_INDEX_SPECIFIER`,  
`B_REVERSE_INDEX_SPECIFIER`| Removes the specified menu item from its parent menu.  
`B_EXECUTE_PROPERTY`| `B_NAME_SPECIFIER`,  
`B_INDEX_SPECIFIER`,  
`B_REVERSE_INDEX_SPECIFIER`| Invokes the specified menu item.  
any other| `B_NAME_SPECIFIER`, `B_INDEX_SPECIFIER`, `B_REVERSE_INDEX_SPECIFIER`| Directs scripting message to the specified menu, first popping the current specifier off the stack.  
  
* * *

### Archived Fields

The [`Archive()`](BMenu.md#BMenu_Archive "Archive\(\)") function adds the following fields to its [`BMessage`](BMessage.md "BMessage") argument:

Field| Type code| Description  
---|---|---  
`_layout`| `B_INT32_TYPE`| Menu layout (Exists only if layout not `B_ITEMS_IN_ROW`).  
`_rsize_to_fit`| `B_BOOL_TYPE`| `true` if menu resizes to fit items.  
`_disable`| `B_BOOL_TYPE`| `true` if menu is disabled.  
`_radio`| `B_BOOL_TYPE`| `true` if menu is in radio mode.  
`_trig_disabled`| `B_BOOL_TYPE`| `true` if menu triggers are disabled.  
`_dyn_label`| `B_BOOL_TYPE`| `true` if menu label mirrors the currently selected item.  
`_maxwidth`| `B_FLOAT_TYPE`| Maximum content width of the menu.  
`_items` (array)| `B_MESSAGE_TYPE`| Menu items (only in deep copy).  
`_i_frames` (array)| `B_MESSAGE_TYPE`| Location of items (only in deep copy of layout `B_ITEMS_IN_MATRIX`)  
  
Some of these fields may not be present if the setting they represent isn't used, or is the default value. For example, if the menu is disabled, the `_disable` field won't be found in the archive.

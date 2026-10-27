<!-- Classes And Methods - The Interface Kit - BMenuItem -->

## BMenuItem

| Derived From:| [`BInvoker`](BInvoker.md "BInvoker")  
---|---  
Mix-in Classes:| [`BArchivable`](BArchivable.md "BArchivable")  
Declared In:| `interface/MenuItem.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BMenuItem_Overview.md "BMenuItem")

  * [Constructor and Destructor](BMenuItem.md#BMenuItem_ConstructorDestructor)
  * [Hook Functions](BMenuItem.md#BMenuItem_HookFunctions)
  * [Member Functions](BMenuItem.md#BMenuItem_MemberFunctions)
  * [Static Functions](BMenuItem.md#BMenuItem_StaticFunctions)
  * [Archived Fields](BMenuItem.md#BMenuItem_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BMenuItem

`BMenuItem(const char* label,  
[`BMessage`](BMessage.md "BMessage")* message,  
char shortcut = 0,  
uint32 modifiers = 0);` `BMenuItem([`BMenu`](BMenu.md "BMenu")* submenu,  
[`BMessage`](BMessage.md "BMessage")* message = NULL);` `BMenuItem([`BMessage`](BMessage.md "BMessage")* archive);`

Creates a new `BMenuItem` and sets its `label`, its invocation `message`, both of which can be `NULL`. When the user invokes the menu item, `message` is copied and the following fields

Whenever the user invokes the item, the model message is copied and the copy is posted and marked for delivery to the target handler. Three pieces of information are added to the copy before it's posted:

Field| Type code| Description  
---|---|---  
`when`| `B_INT64_TYPE`| The time the item was invoked, as measured by the number of microseconds since 12:00:00 AM January 1, 1970.  
`source`| `B_POINTER_TYPE`| A pointer to the `BMenuItem` object.  
`index`| `B_INT32_TYPE`| The index of the item, its ordinal position in the menu. Indices begin at 0.  
  
By default, the target of the message is the window that the contains the menu that the item is part of. Use [`SetTarget()`](BInvoker.md#BInvoker_SetTarget "SetTarget\(\), Target\(\), IsTargetLocal\(\), Messenger\(\)") to set a different target.

The constructor can also set a keyboard `shortcut` for the item. The character that's passed as the `shortcut` parameter will be displayed to the right of the item's label. Upper case characters are mapped to lower case characters; if you want the user to have to type an upper case character, you must include `B_SHIFT_KEY` in the modifers mask.

The `modifiers` mask determines which modifier keys the user must hold down for the shortcut to work. The mask can be formed by combining any of the modifiers constants, especially these:

  * `B_SHIFT_KEY`
  * `B_CONTROL_KEY`
  * `B_OPTION_KEY`

`B_COMMAND_KEY` is required for all keyboard shortcuts; it doesn't have to be explicitly included in the mask.

If the `BMenuItem` is constructed to control a `submenu`, it can't take a shortcut and it typically doesn't post messages—its role is to bring up the submenu. However, it can be assigned a model `message` if the application must take some collateral action when the submenu is opened. The item's initial label will be taken from the name of the submenu. It can be changed after construction by calling [`SetLabel()`](BMenuItem.md#BMenuItem_SetLabel "SetLabel\(\), Label\(\)").

#### ~BMenuItem()

`virtual ~BMenuItem();`

Frees the item's label and its model [`BMessage`](BMessage.md "BMessage") object. If the item controls a submenu, that menu and all its items are also freed. Deleting a `BMenuItem` destroys the entire menu hierarchy under that item.

* * *

### Hook Functions

####  Draw(), DrawContent()

##### protected

`virtual void Draw();``virtual void DrawContent();`

These functions draw the menu item and highlight it if it's currently selected. They're called by the `Draw()` function of the [`BMenu`](BMenu.md "BMenu") where the item is located whenever the menu is required to display itself; they don't need to be called from within application code.

However, they can both be overridden by derived classes that display something other than a textual label. The `Draw()` function is called first. It draws the background for the entire item, then calls `DrawContent()` to draw the label within the item's content area. After `DrawContent()` returns, it draws the check mark (if the item is currently marked) and the keyboard shortcut (if any). It finishes by calling [`Highlight()`](BMenuItem.md#BMenuItem_Highlight "Highlight\(\)") if the item is currently selected.

Both functions draw by calling functions of the [`BMenu`](BMenu.md "BMenu") in which the item is located. For example:
    
    
    void MyItem::DrawContent()
    {
       . . .
       Menu()->DrawBitmap(image);
       . . .
    }

A derived class can override either `Draw()`, if it needs to draw the entire item, or `DrawContent()`, if it needs to draw only within the content area. A `Draw()` function can find the frame rectangle it should draw within by calling the `BMenuItem`'s [`Frame()`](BMenuItem.md#BMenuItem_Frame "Frame\(\)") function; a `DrawContent()` function can calculate the content area from the point returned by [`ContentLocation()`](BMenuItem.md#BMenuItem_ContentLocation "ContentLocation\(\)") and the dimensions provided by [`GetContentSize()`](BMenuItem.md#BMenuItem_GetContentSize "GetContentSize\(\)").

When `DrawContent()` is called, the pen is positioned to draw the item's label and the high color is appropriately set. The high color may be a shade of gray, if the item is disabled, or black if it's enabled. If some other distinction is used to distinguish disabled from enabled items, `DrawContent()` should check the item's current state by calling [`IsEnabled()`](BMenuItem.md#BMenuItem_IsEnabled).

Note

![Note](./images/admonitions/Info_32.png)

If a derived class implements its own `DrawContent()` function, but still wants to draw a textual string, it should do so by assigning the string as the `BMenuItem`'s label and calling the inherited version of `DrawContent()`, not by calling `DrawString()`. This preserves the `BMenuItem`'s ability to display a trigger character in the string.

* * *

### Member Functions

#### Archive()

`virtual status_t Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Archives the `BMenuItem` by recording its current settings in the [`BMessage`](BMessage.md "BMessage") `archive`.

See also: [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)"), [`Instantiate()`](BMenuItem.md#BMenuItem_Instantiate "Instantiate\(\)") static function

#### ContentLocation()

##### protected

`[`BPoint`](BPoint.md "BPoint") ContentLocation() const;`

Returns the left top corner of the content area of the item, in the coordinate system of the [`BMenu`](BMenu.md "BMenu") to which it belongs. The content area of an item is the area where it displays its label (or whatever graphic substitutes for the label). It doesn't include the part of the item where a check mark or a keyboard shortcut could be displayed, nor the border and background around the content area.

You would need to call this function only if you're implementing a [`DrawContent()`](BMenuItem.md#BMenuItem_DrawContent) function to draw the contents of the menu item. The content rectangle can be calculated from the point returned by this function and the size specified by [`GetContentSize()`](BMenuItem.md#BMenuItem_GetContentSize "GetContentSize\(\)").

If the item isn't part of a menu, the return value is indeterminate.

#### Frame()

`[`BRect`](BRect.md "BRect") Frame() const;`

Returns the rectangle that frames the entire menu item, in the coordinate system of the [`BMenu`](BMenu.md "BMenu") to which the item belongs. If the item hasn't been added to a menu, the return value is indeterminate.

See also: [`BMenu::AddItem()`](BMenu.md#BMenu_AddItem "AddItem\(\)")

#### GetContentSize()

##### protected

`virtual void GetContentSize(float* width,  
float* height);`

Writes the size of the item's content area into the variables referred to by `width` and `height`. The content area of an item is the area where its label (or whatever substitutes for the label) is drawn.

A [`BMenu`](BMenu.md "BMenu") calls `GetContentSize()` for each of its items as it arranges them in a column or a row; the function is not called for items in a matrix. The information it provides helps determine where each item is located and the overall size of the menu.

`GetContentSize()` must report a size that's large enough to display the content of the item (and separate one item from another). By default, it reports an area just large enough to display the item's label. This area is calculated from the label and the [`BMenu`](BMenu.md "BMenu")'s current font.

If you design a class derived from `BMenuItem` and implement your own [`Draw()`](BMenuItem.md#BMenuItem_Draw "Draw\(\), DrawContent\(\)") or [`DrawContent()`](BMenuItem.md#BMenuItem_DrawContent) function, you should also implement a `GetContentSize()` function to report how much room will be needed to draw the item's contents.

See also: [`DrawContent()`](BMenuItem.md#BMenuItem_DrawContent), [`ContentLocation()`](BMenuItem.md#BMenuItem_ContentLocation "ContentLocation\(\)")

#### Highlight()

##### protected

`virtual void Highlight(bool flag);`

Highlights the menu item when `flag` is `true`, and removes the highlighting when `flag` is `false`. Highlighting simply inverts all the colors in the item's frame rectangle (except for the check mark).

This function is called by the [`Draw()`](BMenuItem.md#BMenuItem_Draw "Draw\(\), DrawContent\(\)") function whenever the item is selected and needs to be drawn in its highlighted state. There's no reason to call it yourself, unless you define your own version of [`Draw()`](BMenuItem.md#BMenuItem_Draw "Draw\(\), DrawContent\(\)"). However, it can be reimplemented in a derived class, if items belonging to that class need to be highlighted in some way other than simple inversion.

#### Invoke()

##### protected

`virtual status_t Invoke([`BMessage`](BMessage.md "BMessage")* message = NULL);`

Augments the [`BInvoker`](BInvoker.md "BInvoker") version of [`Invoke()`](BInvoker.md#BInvoker_Invoke "Invoke\(\), InvokeNotify\(\)") to ensure that only enabled menu items that are attached to a menu hierarchy can be invoked. This function appropriately marks items when the user invokes them. Before sending a message, it adds `when`, `source`, and `index` field to it, as explained under the `BMenuItem` [constructor](BMenuItem.md#BMenuItem_Constructor "BMenuItem").

#### IsSelected()

##### protected

`bool IsSelected() const;`

Returns `true` if the menu item is currently selected, and `false` if not. Selected items are highlighted.

#### Menu()

`[`BMenu`](BMenu.md "BMenu")* Menu() const;`

Returns the menu where the item is located, or `NULL` if the item hasn't yet been added to a menu.

See also: [`BMenu::AddItem()`](BMenu.md#BMenu_AddItem "AddItem\(\)")

####  SetEnabled(), IsEnabled()

`virtual void SetEnabled(bool enabled);``bool IsEnabled() const;`

`SetEnabled()` enables the `BMenuItem` if the `enabled` flag is `true`, disables it if `enabled` is `false`, and updates the item if it's visible on-screen. If the item controls a submenu, this function calls the submenu's `SetEnabled()` virtual function, passing it the same flag. This ensures that the submenu is enabled or disabled as well.

`IsEnabled()` returns `true` if the `BMenuItem` is enabled, its menu is enabled, and all menus above it in the hierarchy are enabled. It returns `false` if the item is disabled or any objects above it in the menu hierarchy are disabled.

Items and menus are enabled by default.

When using these functions, keep in mind that:

  * Disabling a `BMenuItem` that controls a submenu serves to disable the entire menu hierarchy under the item.

  * Passing an argument of `true` to `SetEnabled()` is not sufficient to enable the item if it's located in a disabled branch of the menu hierarchy. It can only undo a previous `SetEnabled()` call (with an argument of `false`) on the same item.

See also: [`BMenu::SetEnabled()`](BMenu.md#BMenu_SetEnabled "SetEnabled\(\), IsEnabled\(\)")

####  SetLabel(), Label()

`virtual void SetLabel(const char* string);``const char* Label() const;`

`SetLabel()` frees the item's current label and copies `string` to replace it. If the menu is visible on-screen, it will be redisplayed with the item's new label. If necessary, the menu will become wider (or narrower) so that it fits the new label.

The Interface Kit calls this virtual function to:

  * Set the initial label of an item that controls a submenu to the name of the submenu, and

  * Subsequently set the item's label to match the marked item in the submenu, if the submenu was set up to have this feature.

`Label()` returns a pointer to the current label.

See also: [`BMenu::SetLabelFromMarked()`](BMenu.md#BMenu_SetLabelFromMarked "SetLabelFromMarked\(\) , IsLabelFromMarked\(\)"), the `BMenuItem` [constructor](BMenuItem.md#BMenuItem_Constructor "BMenuItem")

####  SetMarked(), IsMarked()

`virtual void SetMarked(bool flag);``bool IsMarked() const;`

`SetMarked()` adds a check mark to the left of the item label if `flag` is `true`, or removes an existing mark if `flag` is `false`. If the menu is visible on-screen. it's redisplayed with or without the mark.

`IsMarked()` returns whether the item is currently marked.

See also: [`BMenu::SetLabelFromMarked()`](BMenu.md#BMenu_SetLabelFromMarked "SetLabelFromMarked\(\) , IsLabelFromMarked\(\)"), [`BMenu::FindMarked()`](BMenu.md#BMenu_FindMarked "FindMarked\(\)")

####  SetShortcut(), Shortcut()

`virtual void SetShortcut(char shortcut,  
uint32 modifiers);``char Shortcut(uint32* modifiers = NULL) const;`

`SetShortcut()` sets the `shortcut` character that's displayed at the right edge of the menu item and the set of `modifiers` that are associated with the character. These two arguments work just like the arguments passed to the `BMenuItem` constructor. See [Shortcuts and Triggers](BMenuItem_Overview.md#BMenuItem_Overview_ShortcutsAndTriggers "Shortcuts and Triggers") for a more complete description.

`Shortcut()` returns the character that's used as the keyboard shortcut for invoking the item, and writes a mask of all the modifier keys the shortcut requires to the variable referred to by `modifiers`. Since the Command key is required to operate the keyboard shortcut for any menu item, `B_COMMAND_KEY` will always be part of the `modifiers` mask. The mask can also be tested against the `B_CONTROL_KEY`, `B_OPTION_KEY`, and `B_SHIFT_KEY` constants.

The shortcut is initially set by the `BMenuItem` [constructor](BMenuItem.md#BMenuItem_Constructor "BMenuItem").

####  SetTrigger(), Trigger()

`virtual void SetTrigger(char trigger);``char Trigger() const;`

`SetTrigger()` sets the `trigger` character that the user can type to invoke the item while the item's menu is open on-screen. If a `trigger` is not set, the Interface Kit will select one for the item, so it's not necessary to call `SetTrigger()`.

The character passed to this function has to match a character displayed in the item—either the keyboard shortcut or a character in the label. The case of the character doesn't matter; lowercase arguments will match uppercase characters in the item and uppercase arguments will match lowercase characters. When the item can be invoked by its trigger, the trigger character is underlined.

If more than one character in the item matches the character passed, `SetTrigger()` tries first to mark the keyboard shortcut. Failing that, it tries to mark an uppercase letter at the beginning of a word. Failing that, it marks the first instance of the character in the label.

If the `trigger` doesn't match any characters in the item, the item won't have a trigger, not even one selected by the system.

`Trigger()` returns the character set by `SetTrigger()`, or `NULL` if `SetTrigger()` didn't succeed or if `SetTrigger()` was never called and the trigger is selected automatically.

See also: [`BMenu::SetTriggersEnabled()`](BMenu.md#BMenu_SetTriggersEnabled "SetTriggersEnabled\(\), AreTriggersEnabled\(\)")

#### Submenu()

`[`BMenu`](BMenu.md "BMenu")* Submenu() const;`

Returns the [`BMenu`](BMenu.md "BMenu") object that the item controls, or `NULL` if the item doesn't control a submenu.

See also: the `BMenuItem` [constructor](BMenuItem.md#BMenuItem_Constructor "BMenuItem"), the [`BMenu`](BMenu.md "BMenu") class

#### TruncateLabel()

##### protected

`virtual void TruncateLabel(float maxWidth,  
char* newLabel);`

Removes characters from the middle of the item label and replaces them with an ellipsis. This is done so that the label will fit within `maxWidth` coordinate units. The shortened string is copied into the `newLabel` buffer.

This function is called by the `BMenuItem` when it draws the item's label, but only if it's necessary to fit a long item into a smaller space. It can be reimplemented by derived classes to do a better job of shortening the string based on the actual content of the label.

Your version of `TruncateLabel()` should be careful to not cut the trigger character from the string.

See also: [`BFont::GetTruncatedStrings()`](BFont.md#BFont_GetTruncatedStrings "GetTruncatedStrings\(\), TruncateString\(\)")

* * *

### Static Functions

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Returns a new `BMenuItem` object, allocated by new and created with the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive. However, if the archive message doesn't contain data for a `BMenuItem` object, this function returns `NULL`.

See also: [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"), [`Archive()`](BMenuItem.md#BMenuItem_Archive "Archive\(\)")

* * *

### Archived Fields

The [`Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)") function adds the following fields to its [`BMessage`](BMessage.md "BMessage") argument:

Field| Type code| Description  
---|---|---  
`_label`| `B_STRING_TYPE`| Text label of the menu item.  
`_disable`| `B_BOOL_TYPE`| `true` if menu item is disabled.  
`_marked`| `B_BOOL_TYPE`| `true` if menu item is marked.  
`_user_trig`| `B_INT32_TYPE`| User-defined trigger character.  
`_shortcut`| `B_INT32_TYPE`| Shortcut character.  
`_mods`| `B_INT32_TYPE`| Shortcut modifiers.  
`_msg`| `B_MESSAGE_TYPE`| Model message for the item.  
`_submenu`| `B_MESSAGE_TYPE`| Submenu (only in deep copy).  
  
Some of these fields may not be present if the setting they represent isn't used, or is the default value. For example, if no shortcut key was defined, the `_shortcut` and `_mods` fields won't be found in the archive.

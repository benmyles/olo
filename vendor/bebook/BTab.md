<!-- Classes And Methods - The Interface Kit - BTab -->

## BTab

| Derived From:| [`BArchivable`](BArchivable.md "BArchivable")  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/TabView.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BTab_Overview.md "BTab")

  * [Constructor and Destructor](BTab.md#BTab_ConstructorDestructor)
  * [Hook Functions](BTab.md#BTab_HookFunctions)
  * [Member Functions](BTab.md#BTab_MemberFunctions)
  * [Static Functions](BTab.md#BTab_StaticFunctions)
  * [Archived Fields](BTab.md#BTab_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BTab()

`BTab([`BView`](BView.md "BView")* tabView = NULL);` `BTab([`BMessage`](BMessage.md "BMessage")* archive);`

Initializes the `BTab` to be enabled, but neither selected nor the current focus. The specified `tabView` becomes the tab's target view—when the tab is selected, its target view is activated. See the [`BTabView`](BTabView.md "BTabView") class for further details on how this works.

If an `archive` message is specified, the message's contents are used to duplicate the archived `BTab` object.

See also: [`SetView()`](BTab.md#BTab_SetView)

#### ~BTab()

`virtual ~BTab();`

Frees all memory the `BTab` allocated. If there is a target view assigned to the tab, it is removed from the parent window and deleted.

* * *

### Hook Functions

####  DrawFocusMark() , DrawTab() , DrawLabel()

`virtual void DrawFocusMark([`BView`](BView.md "BView")* owner,  
[`BRect`](BRect.md "BRect") frame);``virtual void DrawTab([`BView`](BView.md "BView")* owner,  
[`BRect`](BRect.md "BRect") frame,  
tab_position position,  
bool full = true);``virtual void DrawLabel([`BView`](BView.md "BView")* owner,  
[`BRect`](BRect.md "BRect") frame);`

These three functions can be implemented by your `BTab`-derived class to create a new visual appearance for your application's tabs. The owner is the [`BView`](BView.md "BView") in which your tab is being drawn, and the frame is the rectangle in which the tab is to be drawn.

The `DrawFocusMark()` function draws the mark indicating that the `BTab` object is in focus. By default, this consists of a line in the keyboard navigation color, drawn across the bottom of the tab's frame rectangle.

`DrawTab()` is called to draw the tab. It draws the tab's title by calling `DrawLabel()`, then renders the lines to create the tab itself. The position of the tab may affect how the tab is rendered—for example, if the tab is frontmost, it may have a different appearance than the other tabs.

If `full` is `true`, the complete tab is drawn inside the frame rectangle. If `full` is `false`, the right side of the tab is being obscured by the tab to its left, so the right edge should be eliminated or truncated as necessary.

* * *

### Member Functions

#### Archive()

`virtual status_t Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Calls the inherited version of [`Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)") and stores the `BTab` in the [`BMessage`](BMessage.md "BMessage") archive.

See also: [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)"), [`Instantiate()`](BTab.md#BTab_Instantiate "Instantiate\(\)") static function

####  IsEnabled(), SetEnabled()

`bool IsEnabled() const;``virtual void SetEnabled(bool enabled);`

The `IsEnabled()` function returns the `true` if the tab is enabled (and can therefore be selected by the user) or `false` if the tab is disabled.

`SetEnabled()` is called to enable or disable the tab. Pass a value of `true` to enable the tab, or `false` to disable it.

####  IsSelected() , Deselect() , Select()

`bool IsSelected() const;``virtual void Deselect();``virtual void Select([`BView`](BView.md "BView")* owner);`

The `IsSelected()` function returns true if the tab is currently selected, `false` if it's not.

`Deselect()` is called to deselect the tab. This removes the tab's target view from the owner window by calling the target view's [`RemoveSelf()`](BView.md#BView_RemoveSelf "RemoveSelf\(\)") function.

`Select()` is called to select the tab. This also adds the tab's target view to the specified owner view. This is called after the previously selected tab's `Deselect()` function is called.

####  Label(), SetLabel()

`const char* Label() const;``virtual void SetLabel(const char* label);`

The `Label()` function returns the tab's label. The label is the same as the target view's name.

`SetLabel()` is called to set the tab's label. This also changes the target view's name to match the tab's label, if a target view exists.

Note

![Note](./images/admonitions/Info_32.png)

If the tab doesn't have a target view, `SetLabel()` does nothing. Make sure a target view has been set (by calling [`SetView()`](BTab.md#BTab_SetView), [`BTabView::AddTab()`](BTabView.md#BTabView_AddTab "AddTab\(\), RemoveTab\(\)") with a valid target view argument, or in the `BTab` constructor) before you call `SetLabel()`.

####  IsFocus(), MakeFocus()

`bool IsFocus() const;``void MakeFocus(bool inFocus = true);`

`IsFocus()` returns `true` if the tab is the current focus or `false` if it is not.

`MakeFocus()` specifies whether or not the tab is the current focus. Pass `true` to make the tab the current focus, or `false` if you don't want it to be the focus.

####  View(), SetView()

`[`BView`](BView.md "BView")* View() const;``virtual void SetView([`BView`](BView.md "BView")* view);`

The `View()` function returns the tab's target view.

`SetView()` is called to set the tab's target view to the specified `view`.

* * *

### Static Functions

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Returns a new `BTab` object, allocated by new and created with the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive. However, if the message doesn't contain archived data for a `BTab`, `Instantiate()` returns `NULL`.

See also: [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"), [`Archive()`](BTab.md#BTab_Archive "Archive\(\)")

* * *

### Archived Fields

The [`Archive()`](BTab.md#BTab_Archive "Archive\(\)") function adds the following fields to its [`BMessage`](BMessage.md "BMessage") argument:

Field| Type code| Description  
---|---|---  
`_disable`| `B_BOOL_TYPE`| `true` if the tab is disabled. Not present if the tab is enabled.

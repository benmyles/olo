<!-- Classes And Methods - The Interface Kit - BListView -->

## BListView

| Derived From:| [`BView`](BView.md "BView"), [`BInvoker`](BInvoker.md "BInvoker")  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/ListView.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BListView_Overview.md "BListView")

  * [Constructor and Destructor](BListView.md#BListView_ConstructorDestructor)
  * [Hook Functions](BListView.md#BListView_HookFunctions)
  * [Member Functions](BListView.md#BListView_MemberFunctions)
  * [Static Functions](BListView.md#BListView_StaticFunctions)
  * [Scripting Support](BListView.md#BListView_ScriptingSupport)
  * [Archived Fields](BListView.md#BListView_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BListView()

`BListView([`BRect`](BRect.md "BRect") frame,  
const char* name,  
list_view_type type = B_SINGLE_SELECTION_LIST,  
uint32 resizingMode = B_FOLLOW_LEFT | B_FOLLOW_TOP,  
uint32 flags = B_WILL_DRAW | B_NAVIGABLE | B_FRAME_EVENTS);` `BListView([`BMessage`](BMessage.md "BMessage")* archive);`

Initializes the new `BListView`. The `frame`, `name`, `resizingMode`, and `flags` arguments are identical to those declared for the [`BView`](BView.md "BView") class and are passed unchanged to the [`BView`](BView.md "BView") [constructor](BView.md#BView_Constructor "BView\(\)").

The list `type` can be either:

Constant| Description  
---|---  
`B_SINGLE_SELECTION_LIST`| The user can select only one item in the list at a time. This is the default setting.  
`B_MULTIPLE_SELECTION_LIST`| The user can select any number of items by holding down an Option key (for discontinuous selections) or a Shift key (for contiguous selections).  
  
The list begins life empty. Call [`AddItem()`](BListView.md#BListView_AddItem "AddItem\(\)") or [`AddList()`](BListView.md#BListView_AddList "AddList\(\)") to put items in the list. Call [`Select()`](BListView.md#BListView_Select "Select\(\)") to select one of the items so that it's highlighted when the list is initially displayed to the user.

#### ~BListView()

`virtual ~BListView();`

Frees the selection and invocation messages, if any, and any memory allocated to hold the list of items, but not the items themselves.

* * *

### Hook Functions

#### AttachedToWindow()

`virtual void AttachedToWindow();`

Sets up the `BListView` and makes the [`BWindow`](BWindow.md "BWindow") to which it has become attached the target for the messages it sends when items are selected or invoked—provided another target hasn't already been set. In addition, this function calls [`BListItem::Update()`](BListItem.md#BListItem_Update "Update\(\)") for each item in the list to give it a chance to adjust its layout. The `BListView`'s vertical scroll bar is also adjusted.

This function is called for you when the `BListView` becomes part of a window's view hierarchy.

See also: [`BView::AttachedToWindow()`](BView.md#BView_AttachedToWindow "AttachedToWindow\(\), AllAttached\(\)"), [`BInvoker::SetTarget()`](BInvoker.md#BInvoker_SetTarget "SetTarget\(\), Target\(\), IsTargetLocal\(\), Messenger\(\)")

#### Draw()

`virtual void Draw([`BRect`](BRect.md "BRect") updateRect);`

Calls upon every item in the `updateRect` area of the view to draw itself.

`Draw()` is called for you whenever the list view is to be updated or redisplayed; you don't need to call it yourself. You also don't need to reimplement it; to change the way items are drawn, define a new version of [`DrawItem()`](BListItem.md#BListItem_DrawItem "DrawItem\(\)") in a class derived from [`BListItem`](BListItem.md "BListItem").

See also: [`BView::Draw()`](BView.md#BView_Draw "Draw\(\)")

#### FrameResized()

`virtual void FrameResized(float width,  
float height);`

Updates the on-screen display in response to a notification that the `BListView`'s frame rectangle has been resized. In particular, this function looks for a vertical scroll bar that's a sibling of the `BListView`. It adjusts this scroll bar to reflect the way the list view was resized, under the assumption that it must have the `BListView` as its target.

`FrameResized()` is called automatically at the appropriate times; you shouldn't call it yourself.

See also: [`BView::FrameResized()`](BView.md#BView_FrameResized "FrameResized\(\)")

#### InitiateDrag()

`virtual bool InitiateDrag([`BPoint`](BPoint.md "BPoint") point,  
int32 index,  
bool wasSelected);`

Implemented by derived classes to permit users to drag items. This function is called from the `BListView`'s [`MouseDown()`](BListView.md#BListView_MouseDown "MouseDown\(\)") function; it should initiate the drag-and-drop operation and return `true`, or refuse to do so and return `false`. By default, it always returns `false`.

The `point` that's passed to `InitiateDrag()` is the same as the point passed to [`MouseDown()`](BListView.md#BListView_MouseDown "MouseDown\(\)"); it's where the cursor was located when the user pressed the mouse button. The `index` of the item under the cursor (the item that would be dragged) is passed as the second argument, and the `wasSelected` flag indicates whether or not the item was selected before the mouse button went down.

A `BListView` allows users to autoscroll the list by holding the mouse button down and dragging outside its frame rectangle. If a derived class implements `InitiateDrag()` to drag an item each time the user moves the mouse with a button down, it will hide this autoscrolling behavior. Therefore, derived classes typically permit users to drag items only if they're already selected (if `wasSelected` is `true`). In other words, it takes two mouse-down events to drag an item—one to select it and one to begin dragging it.

See also: [`BView::DragMessage()`](BView.md#BView_DragMessage "DragMessage\(\)")

#### KeyDown()

`virtual void KeyDown(const char* bytes,  
int32 numBytes);`

Permits the user to operate the list using the following keys:

Keys| Perform Action  
---|---  
Up Arrow and Down Arrow| Select the items that are immediately before and immediately after the currently selected item.  
Page Up and Page Down| Select the items that are one viewful above and below the currently selected item—or the first and last items if there's no item a viewful away.  
Home and End| Select the first and last items in the list.  
Enter and the Space bar| Invoke the current selection.  
  
This function also incorporates the inherited [`BView`](BView.md "BView") version so that the Tab key can navigate to another view.

`KeyDown()` is called to report `B_KEY_DOWN` messages when the `BListView` is the focus view of the active window; you shouldn't call it yourself.

See also: [`BView::KeyDown()`](BView.md#BView_KeyDown "KeyDown\(\)"), [`Select()`](BListView.md#BListView_Select "Select\(\)"), [`Invoke()`](BListView.md#BListView_Invoke "Invoke\(\)")

#### MouseDown()

`virtual void MouseDown([`BPoint`](BPoint.md "BPoint") point);`

Responds to `B_MOUSE_DOWN` messages by selecting items, invoking them (if the mouse-down event is the second of a double-click), and autoscrolling the list (when the user drags with a mouse button down). This function also calls [`InitiateDrag()`](BListView.md#BListView_InitiateDrag "InitiateDrag\(\)") to give derived classes the opportunity to drag items. You can implement that function; you shouldn't override (or call) this one.

See also: [`BView::MouseDown()`](BView.md#BView_MouseDown "MouseDown\(\)"), [`Select()`](BListView.md#BListView_Select "Select\(\)"), [`Invoke()`](BListView.md#BListView_Invoke "Invoke\(\)"),

#### SelectionChanged()

`virtual void SelectionChanged();`

Implemented by derived classes to do whatever they please when the selection changes.

#### TargetedByScrollView()

`virtual void TargetedByScrollView([`BScrollView`](BScrollView.md "BScrollView")* view);`

Notes the fact that the `BListView` is the target of a [`BScrollView`](BScrollView.md "BScrollView") and arranges for the border of the [`BScrollView`](BScrollView.md "BScrollView") to be highlighted when the `BListView` is the current focus view of the active window.

See also: [`BScrollView::SetBorderHighlighted()`](BScrollView.md#BScrollView_SetBorderHighlighted "SetBorderHighlighted\(\), IsBorderHighlighted\(\)")

#### WindowActivated()

`virtual void WindowActivated(bool active);`

Makes sure that the [`BScrollView`](BScrollView.md "BScrollView") that targets the `BListView` is redrawn when the window is activated and deactivated, if the `BListView` is the current focus view. This gives the [`BScrollView`](BScrollView.md "BScrollView") a chance to highlight or unhighlight its border, as appropriate.

See also: [`BView::WindowActivated()`](BView.md#BView_WindowActivated "WindowActivated\(\)")

* * *

### Member Functions

#### AddItem()

`virtual bool AddItem([`BListItem`](BListItem.md "BListItem")* item);` `virtual bool AddItem([`BListItem`](BListItem.md "BListItem")* item,  
int32 index);`

Adds an item to the `BListView` at `index`—or, if no index is supplied, at the end of the list. If necessary, additional memory is allocated to accommodate the new item.

Adding an item never removes an item already in the list. If the item is added at an index that's already occupied, items currently in the list are bumped down one slot to make room.

If `index` is out of range (greater than the current item count, or less than zero), this function fails and returns `false`. Otherwise, it returns `true`.

See also: [`BListItem`](BListItem.md "BListItem"), [`BList::AddItem()`](BList.md#BList_AddItem "AddItem\(\)")

#### AddList()

`virtual bool AddList([`BList`](BList.md "BList")* list);` `virtual bool AddList([`BList`](BList.md "BList")* list,  
int32 index);`

Adds the contents of another `list` to this `BListView`. The items from the [`BList`](BList.md "BList") are inserted at `index`—or, if no index is given, they're appended to the end of the list. If the index is out of range, the function fails and returns `false`. If successful, it returns `true`.

The `BListView` doesn't check to be sure that all the items it adds from the list are pointers to [`BListItem`](BListItem.md "BListItem") objects. It assumes that they are; if the assumption is `false`, subsequent `BListView` operations will fail.

See also: [`AddItem()`](BListView.md#BListView_AddItem "AddItem\(\)"), [`BList::AddList()`](BList.md#BList_AddList "AddList\(\)")

#### Archive()

`virtual Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Stores the `BListView` in the [`BMessage`](BMessage.md "BMessage") archive.

See also: [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)"), [`Instantiate()`](BListView.md#BListView_Instantiate "Instantiate\(\)") static function

#### CountItems()

`int32 CountItems() const;`

Returns the number of [`BListItem`](BListItem.md "BListItem")s currently in the list.

See also: [`BList::CountItems`](BList.md#BList_CountItems "CountItems\(\)")

#### CurrentSelection()

`int32 CurrentSelection(int32 index = 0) const;`

Returns the index of a currently selected item in the list, or a negative number if no item is selected. The domain of the `index` passed as an argument is the current set of selected items; the first selected item is at `index` 0, the second at `index` 1, and so on, even if the selection is not contiguous. The domain of the returned index is the set of all items in the list.

To get all currently selected items, increment the passed `index` until the function returns a negative number:
    
    
    [BListItem](BListItem.md "BListItem")* item;
    int32 selected;
    while ( (selected = myListView->CurrentSelection(i)) >= 0 ) {
       item = ItemAt(selected);
       . . .
    }

See also: [`Select()`](BListView.md#BListView_Select "Select\(\)")

####  Deselect(), DeselectAll(), DeselectExcept()

`void Deselect(int32 index);``void DeselectAll();``void DeselectExcept(int32 start,  
int32 finish);`

These functions deselect the item at `index`, all the items, or all the items except those from index `start` through index `finish`.

See also: [`Select()`](BListView.md#BListView_Select "Select\(\)")

#### DoForEach()

`void DoForEach(bool (*func)([`BListItem`](BListItem.md "BListItem")*));` `void DoForEach(bool (*func)([`BListItem`](BListItem.md "BListItem")*),  
void* arg2);`

Calls the `func` function once for each item in the `BListView`. [`BListItem`](BListItem.md "BListItem")s are visited in order, beginning with the first one in the list (index 0) and ending with the last. If a call to `func` returns `true`, the iteration is stopped, even if some items have not yet been visited.

`func` must be a function that takes one or two arguments. The first argument is a pointer to the [`BListItem`](BListItem.md "BListItem"); the second argument, if `func` requires one, is passed to `DoForEach()` as `arg2`.

See also: [`BList::DoForEach()`](BList.md#BList_DoForEach "DoForEach\(\)")

#### HasItem()

`bool HasItem([`BListItem`](BListItem.md "BListItem")* item) const;`

Returns `true` if item is in the list, and `false` if not.

See also: [`BList::HasItem()`](BList.md#BList_HasItem "HasItem\(\)")

#### IndexOf()

`int32 IndexOf([`BListItem`](BListItem.md "BListItem")* item) const;` `int32 IndexOf([`BPoint`](BPoint.md "BPoint")* point) const;`

Returns the index where a particular `item`—or the item whose display rectangle includes a particular `point`—is located in the list. If the item is in the list more than once, the index returned will be the position of its first occurrence. To determine whether an item lies at the specified point, only the y-coordinate value of the point is considered.

If the `item` isn't in the list or the y-coordinate of the `point` doesn't intersect with the data rectangle of the `BListView`, the return value will be a negative number.

See also: [`BList::IndexOf()`](BList.md#BList_IndexOf "IndexOf\(\)")

#### InvalidateItem()

`void InvalidateItem(int32 index);`

Invalidates the item at `index` so that an update message will be sent forcing the `BListView` to redraw it.

See also: [`BView::Invalidate()`](BView.md#BView_Invalidate "Invalidate\(\)")

#### Invoke()

`virtual status_t Invoke([`BMessage`](BMessage.md "BMessage")* message = NULL) const;`

Augments the [`BInvoker`](BInvoker.md "BInvoker") version of [`Invoke()`](BInvoker.md#BInvoker_Invoke "Invoke\(\), InvokeNotify\(\)") to add three pieces of information to each message the `BListView` sends:

Field| Type code| Description  
---|---|---  
`when`| `B_INT64_TYPE`| When the message is sent, as measured by the number of microseconds since 12:00:00 AM 1970.  
`source`| `B_POINTER_TYPE`| A pointer to the `BListView` object.  
`index`| `B_INT32_TYPE`| An array containing the index of every selected item.  
  
This function is called to send both the selection message and the invocation message. It can also be called from application code. The default target of the message (established by [`AttachedToWindow()`](BListView.md#BListView_AttachedToWindow "AttachedToWindow\(\)")) is the [`BWindow`](BWindow.md "BWindow") where the `BListView` is located.

What it means to "invoke" selected items depends entirely on the invocation [`BMessage`](BMessage.md "BMessage") and the receiver's response to it. This function does nothing but send the message.

See also: [`Select()`](BListView.md#BListView_Select "Select\(\)"), [`SetInvocationMessage()`](BListView.md#BListView_SetInvocationMessage "SetInvocationMessage\(\), InvocationMessage\(\), InvocationCommand\(\)"), [`BInvoker::SetTarget()`](BInvoker.md#BInvoker_SetTarget "SetTarget\(\), Target\(\), IsTargetLocal\(\), Messenger\(\)")

#### IsItemSelected()

`bool IsItemSelected(int32 index) const;`

Returns `true` if the item at `index` is currently selected, and `false` if it's not.

See also: [`CurrentSelection()`](BListView.md#BListView_CurrentSelection "CurrentSelection\(\)")

####  ItemAt() , FirstItem() , LastItem()

`[`BListItem`](BListItem.md "BListItem")* ItemAt(int32 index) const;``[`BListItem`](BListItem.md "BListItem")* FirstItem() const;``[`BListItem`](BListItem.md "BListItem")* LastItem() const;`

The first of these functions returns the [`BListItem`](BListItem.md "BListItem") at `index`, or `NULL` if the index is out of range. The other two functions return the very first and very last items in the list, or `NULL` if the list is empty. None of the functions alters the contents of the list—they don't remove the returned item.

See also: [`Items()`](BListView.md#BListView_Items "Items\(\)"), [`BList::ItemAt()`](BList.md#BList_ItemAt "ItemAt\(\)"), [`BList::FirstItem()`](BList.md#BList_FirstItem "FirstItem\(\)"), [`BList::LastItem()`](BList.md#BList_LastItem "LastItem\(\)")

#### ItemFrame()

`[`BRect`](BRect.md "BRect") ItemFrame(int32 index);`

Returns the frame rectangle of the [`BListItem`](BListItem.md "BListItem") at `index`. The rectangle is stated in the coordinate system of the `BListView` and defines the area where the item is drawn. Items can differ in height, but all have the same width.

See also: [`DrawItem()`](BListItem.md#BListItem_DrawItem "DrawItem\(\)")

#### Items()

`const [`BListItem`](BListItem.md "BListItem")** Items() const;`

Returns a pointer to the `BListView`'s list of [`BListItem`](BListItem.md "BListItem")s. You can index directly into the list of items if you're certain that the index is in range:
    
    
    [BListItem](BListItem.md "BListItem")* item = Items()[index];

Although the practice is discouraged, you can also step through the list of items by incrementing the list pointer that `Items()` returns. Be aware that the list isn't null-terminated—you have to detect the end of the list by some other means. The simplest method is to count items:
    
    
    [BListItem](BListItem.md "BListItem")** ptr = myListView->Items();
    
    for ( long i = myListView->CountItems(); i > 0; i—)
    {
       . . .
       *ptr++;
    }

You should never use the items pointer to alter the contents of the list.

See also: [`DoForEach()`](BListView.md#BListView_DoForEach "DoForEach\(\)"), [`SortItems()`](BListView.md#BListView_SortItems "SortItems\(\)"), [`BList::Items()`](BList.md#BList_Items "Items\(\)")

####  MakeEmpty(), IsEmpty()

`virtual void MakeEmpty();``bool IsEmpty() const;`

`MakeEmpty()` empties the `BListView` of all its items, without freeing the [`BListItem`](BListItem.md "BListItem") objects.

`IsEmpty()` returns `true` if the list is empty (if it contains no items), and `false` otherwise.

See also: [`RemoveItem()`](BListView.md#BListView_RemoveItem), [`BList::MakeEmpty()`](BList.md#BList_MakeEmpty "MakeEmpty\(\)")

#### MakeFocus()

`virtual void MakeFocus(bool focused = true);`

Overrides the [`BView`](BView.md "BView") version of [`MakeFocus()`](BView.md#BView_MakeFocus "MakeFocus\(\)") to draw an indication that the `BListView` has become the focus for keyboard events when the focused flag is `true`, and to remove that indication when the flag is `false`.

####  MoveItem(), ReplaceItem(), SwapItems()

`bool MoveItem(int32 from,  
int32 to);``bool ReplaceItem(int32 index,  
[`BListItem`](BListItem.md "BListItem")* item);``bool SwapItems(int32 index1,  
int32 index2);`

`MoveItem()` moves the item located at the index `from` to the index specified by the `to` argument.

`ReplaceItem()` replaces the item located at the specifie `index` with the specified `item`.

`SwapItems()` swaps the items specified by `index1` and `index2` in the list.

These functions return `true` if the requested operation is completed successfully, or `false` if the operation failed (for example, if a specified index is out of range).

####  RemoveItem(), RemoveItems()

`virtual [`BListItem`](BListItem.md "BListItem")* RemoveItem(int32 index);` `virtual [`BListItem`](BListItem.md "BListItem")* RemoveItem([`BListItem`](BListItem.md "BListItem")* item);`

`virtual [`BListItem`](BListItem.md "BListItem")* RemoveItems(int32 index,  
int32 count);`

`RemoveItem()` removes a single item from the `BListView`. If passed an `index`, it removes the item at that index and returns it. If there's no item at the index, it returns `NULL`. If passed an `item`, this function looks for that particular item in the list, removes it, and returns `true`. If it can't find the item, it returns `false`. If the item is in the list more than once, this function removes only its first occurrence.

`RemoveItems()` removes `count` number of items from the `BListView`, beginning with the item at `index`. If the list doesn't contain `count` items between `index` and the end of the list, this function removes as many items as there are.

The list is compacted after an item is removed. Therefore, you shouldn't try to empty a list (or a range within a list) by removing items incrementing indices. You should either start with the highest index and move towards the head of the list, or remove at the same index (the lowest in the range) some number of times. As an example of the latter, the following code removes the first five items in the list:
    
    
    for ( int32 i = 0; i <= 4; i++ )
       myListView->RemoveItem(0);

See also: [`MakeEmpty()`](BListView.md#BListView_MakeEmpty "MakeEmpty\(\), IsEmpty\(\)"), [`BList::RemoveItem()`](BList.md#BList_RemoveItem "RemoveItem\(\) , RemoveItems\(\)")

#### ScrollTo()

`virtual void ScrollTo([`BPoint`](BPoint.md "BPoint") point);` `inline void ScrollTo(float x,  
float y);`

Augments the [`BView`](BView.md "BView") version of [`ScrollTo()`](BView.md#BView_ScrollTo) to do some class-internal housekeeping when the list is scrolled. For all practical purposes, these functions are identical to their [`BView`](BView.md "BView") counterparts.

#### ScrollToSelection()

`void ScrollToSelection();`

Scrolls the list so that the first item in the current selection is visible.

#### Select()

`void Select(int32 index,  
bool extend = false);` `void Select(int32 start,  
int32 finish,  
bool extend = false);`

Selects the item located at `index`—or all the items from the `start` index through the `finish` index – provided that none of the indices are out of range. If the `extend` flag is `false`, as it is by default, this function removes the highlighting from the previously selected item(s) and highlights the new selection. However, if the `extend` flag is `true`, the newly selected items are added to the current selection.

If you want your newly-selected item to be scrolled into view, you must call [`ScrollToSelection()`](BListView.md#BListView_ScrollToSelection "ScrollToSelection\(\)") after invoking `Select()`. The `Select()` function itself does not do any scrolling.

`Select()` can be called to set an initial selection in the list or change the current selection. It permits the program to select a number of items, even for a `B_SINGLE_SELECTION_LIST` list.

If you want to be informed of successful selections, you either have to implement [`SelectionChanged()`](BListView.md#BListView_SelectionChanged "SelectionChanged\(\)") or supply a selection message (through [`SetSelectionMessage()`](BListView.md#BListView_SetSelectionMessage "SetSelectionMessage\(\), SelectionMessage\(\), SelectionCommand\(\)") ). The former is invoked and the latter is sent when an item is selected.

See also: [`Invoke()`](BListView.md#BListView_Invoke "Invoke\(\)"), [`Deselect()`](BListView.md#BListView_Deselect "Deselect\(\), DeselectAll\(\), DeselectExcept\(\)")

####  SetInvocationMessage(), InvocationMessage(), InvocationCommand()

`virtual void SetInvocationMessage([`BMessage`](BMessage.md "BMessage")* message);``[`BMessage`](BMessage.md "BMessage")* InvocationMessage() const;``uint32 InvocationCommand() const;`

These functions set and return information about the [`BMessage`](BMessage.md "BMessage") that the `BListView` sends when currently selected items are invoked.

`SetInvocationMessage()` assigns `message` to the `BListView`, freeing any message previously assigned. The message becomes the responsibility of the `BListView` object and will be freed only when it's replaced by another message or the `BListView` is freed; you shouldn't free it yourself. Passing a `NULL` pointer to this function deletes the current message without replacing it.

When sending the message, the [`Invoke()`](BListView.md#BListView_Invoke "Invoke\(\)") function makes a copy of it and adds two pieces of relevant information—"when" the message is sent and the "source" `BListView`. These names should not be used for any data that you add to the invocation `message`.

`InvocationMessage()` returns a pointer to the [`BMessage`](BMessage.md "BMessage") and `InvocationCommand()` returns its `what` data member. The message belongs to the `BListView`; it can be altered by adding or removing data, but it shouldn't be deleted. To get rid of the current message, pass a `NULL` pointer to `SetInvocationMessage()`.

See also: [`Invoke()`](BListView.md#BListView_Invoke "Invoke\(\)"), the [`BMessage`](BMessage.md "BMessage") class

####  SetListType() , ListType()

`virtual void SetListType(list_view_type type);``list_view_type ListType() const;`

These functions set and return the list type—whether or not it permits multiple selections. The list_view_type must be either `B_SINGLE_SELECTION_LIST` or `B_MULTIPLE_SELECTION_LIST`. The type is first set when the `BListView` is constructed.

See also: the `BListView` [constructor](BListView.md#BListView_Constructor "BListView\(\)")

####  SetSelectionMessage(), SelectionMessage(), SelectionCommand()

`virtual void SetSelectionMessage([`BMessage`](BMessage.md "BMessage")* message);``[`BMessage`](BMessage.md "BMessage")* SelectionMessage() const;``uint32 SelectionCommand() const;`

These functions set, and return information about, the message that a `BListView` sends whenever a new item is selected. They're exact counterparts to the functions described above under [`SetInvocationMessage()`](BListView.md#BListView_SetInvocationMessage "SetInvocationMessage\(\), InvocationMessage\(\), InvocationCommand\(\)"), except that the selection message is sent whenever an item in the list is selected, rather than when invoked. It's more common to take action (to initiate a message) when invoking an item than when selecting one.

See also: [`Select()`](BListView.md#BListView_Select "Select\(\)"), [`Invoke()`](BListView.md#BListView_Invoke "Invoke\(\)"), the [`BMessage`](BMessage.md "BMessage") class

#### SortItems()

`void* SortItems(int (*compareFunc)(const void* , const void* ));`

Rearranges the items in the list. The items are sorted using the `compareFunc` comparison function passed as an argument. This function should return a negative number if the first item is ordered before the second, a positive number if the second comes before the first, and 0 if the two items are ordered equivalently.

Although the comparison function is, in the usual manner for such functions, declared to take two void* arguments, each argument should be a pointer to an item in the list—in other words, a pointer to a [`BListItem`](BListItem.md "BListItem") pointer:
    
    
    int compare_func([BListItem](BListItem.md "BListItem")** firstArg, [BListItem](BListItem.md "BListItem")** secondArg);

See also: [`Items()`](BListView.md#BListView_Items "Items\(\)"), [`BList::SortItems()`](BList.md#BList_SortItems "SortItems\(\)")

* * *

### Static Functions

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Returns a new `BListView` object, allocated by new and created with the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive. However, if the archive message doesn't contain data for a `BListView` object, this function returns `NULL`.

See also: [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"), [`Archive()`](BListView.md#BListView_Archive "Archive\(\)")()

* * *

### Scripting Support

The `BListView` class implements the suite called "suite/vnd.Be-list-view" consisting of the following messages:

#### The Item Property

The "Item" property refers to individual [`BListItem`](BListItem.md "BListItem")s in the list.

Message| Specifiers| Description  
---|---|---  
`B_COUNT_PROPERTIES`| `B_DIRECT_SPECIFIER`| Returns the number of [`BListItem`](BListItem.md "BListItem")s currently in the list.  
`B_EXECUTE_PROPERTY`| `B_INDEX_SPECIFIER`,  
`B_REVERSE_INDEX_SPECIFIER`,  
`B_RANGE_SPECIFIER`,  
`B_REVERSE_RANGE_SPECIFIER`| Select and invoke the specified items, first removing any existing selection.  
  
#### The Selection Property

The "Selection" property refers to the set of selected BListItems. If the `BListView` is of type `B_SINGLE_SELECTION_LIST`, then only one item can be selected at any given instant; the list type cannot be overridden through the scripting interface. Attempting to access multiple selected list elements in a `B_SINGLE_SELECTION_LIST` results in an error.

Message| Specifiers| Description  
---|---|---  
`B_COUNT_PROPERTIES`| `B_DIRECT_SPECIFIER`| Returns int32 count of items in the selection.  
`B_EXECUTE_PROPERTY`| `B_DIRECT_SPECIFIER`| Invoke items in selection.  
`B_GET_PROPERTY`| `B_DIRECT_SPECIFIER`| Returns int32 indices of all items in the selection.  
`B_SET_PROPERTY`| `B_DIRECT_SPECIFIER`,  
`B_INDEX_SPECIFIER`,  
`B_REVERSE_INDEX_SPECIFIER`,  
`B_RANGE_SPECIFIER`,  
`B_REVERSE_RANGE_SPECIFIER`| Extends current selection or deselects specified items. Boolean field "data" chooses selection or deselection.  
`B_SET_PROPERTY`| `B_DIRECT_SPECIFIER`| Select or deselect all items in the selection. Boolean field "data" chooses selection or deselection.  
  
* * *

### Archived Fields

The [`Archive()`](BListView.md#BListView_Archive "Archive\(\)") function adds the following fields to its [`BMessage`](BMessage.md "BMessage") argument:

Field| Type code| Description  
---|---|---  
`_lv_type`| `B_INT32_TYPE`| The list type.  
`_l_items` (array)| `B_MESSAGE_TYPE`| List items in the list (deep copy only)  
`_msg`| `B_MESSAGE_TYPE`| Invocation message.  
`_2nd_msg`| `B_MESSAGE_TYPE`| Selection message.  
  
Some of these fields may not be present if the setting they represent isn't used, or is the default value. For example, if there is no selection message, the `_2nd_msg` field won't be found in the archive.

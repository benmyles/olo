<!-- Classes And Methods - The Interface Kit - BOutlineListView -->

## BOutlineListView

| Derived From:| [`BListView`](BListView.md "BListView")  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/OutlineListView.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BOutlineListView_Overview.md "BOutlineListView")

  * [Constructor and Destructor](BOutlineListView.md#BOutlineListView_ConstructorDestructor)
  * [Hook Functions](BOutlineListView.md#BOutlineListView_HookFunctions)
  * [Member Functions](BOutlineListView.md#BOutlineListView_MemberFunctions)
  * [Static Functions](BOutlineListView.md#BOutlineListView_StaticFunctions)

  
  
* * *

### Constructor and Destructor

#### BOutlineListView()

`BOutlineListView([`BRect`](BRect.md "BRect") frame,  
const char* name,  
list_view_type type = B_SINGLE_SELECTION_LIST,  
uint32 resizingMode = B_FOLLOW_LEFT | B_FOLLOW_TOP,  
uint32 flags = B_WILL_DRAW | B_FRAME_EVENTS | B_NAVIGABLE);` `BOutlineListView([`BMessage`](BMessage.md "BMessage")* archive);`

Initializes the `BOutlineListView`. This constructor matches the [`BListView`](BListView.md "BListView") constructor in every detail, including default arguments. All argument values are passed to the [`BListView`](BListView.md "BListView") [constructor](BListView.md#BListView_Constructor "BListView\(\)") without change. The `BOutlineListView` class doesn't do any initialization of its own.

#### ~BOutlineListView()

`virtual ~BOutlineListView();`

Does nothing; this class relies on the [`BListView`](BListView.md "BListView") [constructor](BListView.md#BListView_Constructor "BListView\(\)") destructor.

* * *

### Hook Functions

#### KeyDown()

`virtual void KeyDown(const char* bytes,  
int32 numBytes);`

Augments the inherited version of [`KeyDown()`](BListView.md#BListView_KeyDown "KeyDown\(\)") to allow users to navigate the outline hierarchy using the arrow keys and to expand or collapse sections of the outline using Control—arrow key combinations.

#### MouseDown()

`virtual void MouseDown([`BPoint`](BPoint.md "BPoint") point);`

Augments the inherited version of [`MouseDown()`](BView.md#BView_MouseDown "MouseDown\(\)") to permit users to expand and collapse sections of the outline by clicking on an item's latch.

* * *

### Member Functions

####  AddItem(), AddUnder()

`virtual bool AddItem([`BListItem`](BListItem.md "BListItem")* item);` `virtual bool AddItem([`BListItem`](BListItem.md "BListItem")* item,  
int32 index);`

`virtual bool AddUnder([`BListItem`](BListItem.md "BListItem")* item,  
[`BListItem`](BListItem.md "BListItem")* superitem);`

These functions add an `item` to the list. `AddItem()` adds the item at `index`—where the index counts all items assigned to the `BOutlineListView`—or, if an `index` isn't specified, at the end of the list. The two versions of this function override their [`BListView`](BListView.md "BListView") counterparts to ensure that the item is correctly entered into the outline. If the item is added to a portion of the list that is collapsed, it won't be visible.

`AddUnder()` adds an `item` immediately after another item in the list and at one outline level deeper. The level of the `item` is modified accordingly. Thus, the item already in the list becomes the superitem for the newly added `item`. If its new superitem is collapsed or is in a collapsed part of the list, the item will not be visible.

Unlike `AddUnder()`, `AddItem()` respects the outline level of the item. By setting the item's level before calling `AddItem()`, you can add it as a subitem to an item at a higher outline level or insert it as a superitem to items at a lower level.

See also: the [`BListItem`](BListItem.md "BListItem") class

#### AddList()

`virtual bool AddList([`BList`](BList.md "BList")* newItems);` `virtual bool AddList([`BList`](BList.md "BList")* newItems,  
int32 index);`

Adds a group of items to the list just as [`AddItem()`](BOutlineListView.md#BOutlineListView_AddItem "AddItem\(\), AddUnder\(\)") adds a single item. The `index` counts all items assigned to the `BOutlineListView`. The `newItems` [`BList`](BList.md "BList") must contain pointers to [`BListItem`](BListItem.md "BListItem") objects.

See also: [`BListView::AddList()`](BListView.md#BListView_AddList "AddList\(\)")

#### Archive()

`virtual status_t Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Archives the `BOutlineListView` object much as the [`Archive()`](BListView.md#BListView_Archive "Archive\(\)") function in the [`BListView`](BListView.md "BListView") class does, but makes sure that all items are archived, including items in collapsed sections of the list, when the `deep` flag is `true`.

See also: [`BListView::Archive()`](BListView.md#BListView_Archive "Archive\(\)"), [`Instantiate()`](BOutlineListView.md#BOutlineListView_Instantiate "Instantiate\(\)") static function

####  Collapse(), Expand()

`void Collapse([`BListItem`](BListItem.md "BListItem")* item);``void Expand([`BListItem`](BListItem.md "BListItem")* item);`

These functions collapse and expand the section of the list controlled by the `item` superitem. If `item` isn't a superitem, it is nevertheless flagged as expanded or collapsed so that it will behave appropriately in case it does become a superitem.

See also: [`BListItem::SetExpanded()`](BListItem.md#BListItem_SetExpanded "SetExpanded\(\), IsExpanded\(\)")

#### CountItemsUnder()

`int32 CountItemsUnder([`BListItem`](BListItem.md "BListItem")* underItem,  
bool oneLevelOnly) const;`

Counts the items located under the `underItem` superitem and returns that value. If the `oneLevelOnly` argument is `true`, only items directly contained within the specified superitem are considered. If `oneLevelOnly` is `false`, all subitems of the specified item are considered, as are all subitems of those subitems and so forth.

Note

![Note](./images/admonitions/Info_32.png)

When `oneLevelOnly` is `false`, `CountItemsUnder()` acts just like [`FullListCountItems()`](BOutlineListView.md#BOutlineListView_FullListCountItems "FullListCountItems\(\), FullListCurrentSelection\(\), FullListFirstItem\(\) , FullListLastItem\(\), FullListIndexOf\(\), FullListItemAt\(\), FullListHasItem\(\), FullListIsEmpty\(\), FullListDoForEach\(\), FullListSortItems\(\)"), except the first item in the list that's considered is `underItem` instead of the first item in the full list.

See also: [`BListView::CountItems()`](BListView.md#BListView_CountItems "CountItems\(\)")

#### EachItemUnder()

`[`BListItem`](BListItem.md "BListItem")* EachItemUnder([`BListItem`](BListItem.md "BListItem")* underItem,  
bool oneLevelOnly,  
[`BListItem`](BListItem.md "BListItem")*  (*eachFunc) ([`BListItem`](BListItem.md "BListItem")*, void* ),  
void* data);`

Calls the function `eachFunc` for every item located under the `underItem` superitem. If `oneLevelOnly` is `true`, `eachFunc` is only called for items located directly under the `underItem`; if `oneLevelOnly` is `false`, the `eachFunc` is called once for every subitem of `underItem`, including subitems of subitems, recursively.

The `data` argument is passed through to each call to the `eachFunc` function as the second argument to that function, and may be used for whatever purpose your `eachFunc` requires.

If the `eachFunc` function returns a pointer to a [`BListItem`](BListItem.md "BListItem") (rather than `NULL`), processing is stopped immediately, even if there are items still unvisited. The [`BListItem`](BListItem.md "BListItem") pointer returned by the `eachFunc` function is returned by `EachItemUnder()`.

Note

![Note](./images/admonitions/Info_32.png)

When `oneLevelOnly` is `false`, `EachItemUnder()` acts just like [`FullListDoForEach()`](BOutlineListView.md#BOutlineListView_FullListDoForEach), except the first item in the list that's considered is `underItem` instead of the first item in the full list.

See also: [`BListView::DoForEach()`](BListView.md#BListView_DoForEach "DoForEach\(\)")

####  FullListCountItems(), FullListCurrentSelection(), FullListFirstItem() , FullListLastItem(), FullListIndexOf(), FullListItemAt(), FullListHasItem(), FullListIsEmpty(), FullListDoForEach(), FullListSortItems()

`int32 FullListCountItems() const;``int32 FullListCurrentSelection(int32 index = 0) const;``[`BListItem`](BListItem.md "BListItem")* FullListFirstItem() const;``[`BListItem`](BListItem.md "BListItem")* FullListLastItem() const;`

`int32 FullListIndexOf([`BPoint`](BPoint.md "BPoint") point) const;` `int32 FullListIndexOf([`BListItem`](BListItem.md "BListItem")* item) const;`

`[`BListItem`](BListItem.md "BListItem")* FullListItemAt(int32 index) const;``bool FullListHasItem([`BListItem`](BListItem.md "BListItem")* item) const;``bool FullListIsEmpty() const;`

`void FullListDoForEach(bool (*func)([`BListItem`](BListItem.md "BListItem")*));` `void FullListDoForEach(bool (*func)([`BListItem`](BListItem.md "BListItem")*), void* ));` `void FullListDoForEach(bool (*func)([`BListItem`](BListItem.md "BListItem")*), void* ), void* );`

`void FullListSortItems(int (*compareFunc)(const [`BListItem`](BListItem.md "BListItem") *, const [`BListItem`](BListItem.md "BListItem") *));`

These functions parallel a similar set of functions defined in the [`BListView`](BListView.md "BListView") class. The [`BListView`](BListView.md "BListView") functions have identical names, but without the `FullList…()` prefix. When applied to a `BOutlineListView` object, the inherited functions consider only items in sections of the outline that can be displayed on-screen—that is, they skip over items in collapsed portions of the list.

These `BOutlineListView` functions, on the other hand, consider all items in the list. For example, [`IndexOf()`](BListView.md#BListView_IndexOf "IndexOf\(\)") and `FullListIndexOf()` both return an index to a given item. However, for [`IndexOf()`](BListView.md#BListView_IndexOf "IndexOf\(\)") the index is to the position of the item in the list that can be currently displayed, but for `FullListIndexOf()` it's to the item's position in the full list, including collapsed sections.

#### IsExpanded()

`bool IsExpanded(int32 index);`

Returns `true` if the item at `index` is marked as controlling an expanded section of the list, and `false` if it's marked as controlling a collapsed section or if there's no item at that index. If a superitem is expanded, the `BOutlineListView` can display its subitems; if not, the subitems are hidden.

The `index` passed to this function is to the full list of items assigned to the `BOutlineListView`.

See also: [`BListItem::IsExpanded()`](BListItem.md#BListItem_IsExpanded)

#### ItemUnderAt()

`[`BListItem`](BListItem.md "BListItem")* ItemUnderAt([`BListItem`](BListItem.md "BListItem")* underItem,  
bool oneLevelOnly,  
int32 index) const;`

Returns a pointer to the `index`th item under the specified `underItem`. If `oneLevelOnly` is `true`, only items located directly under the `underItem` are considered. If `oneLevelOnly` is `false`, subitems are scanned recursively to locate the appropriate index.

Note

![Note](./images/admonitions/Info_32.png)

When `oneLevelOnly` is `false`, `ItemUnderAt()` acts just like [`FullListItemAt()`](BOutlineListView.md#BOutlineListView_FullListItemAt), except the first item in the list that's considered is `underItem` instead of the first item in the full list.

#### MakeEmpty()

`virtual void MakeEmpty();`

Overrides the [`BListView`](BListView.md "BListView") version of [`MakeEmpty()`](BListView.md#BListView_MakeEmpty "MakeEmpty\(\), IsEmpty\(\)") to remove all items from the list. The [`BListView`](BListView.md "BListView") version of this function won't work as advertised on a `BOutlineListView`.

####  RemoveItem(), RemoveItems()

`virtual bool RemoveItem([`BListItem`](BListItem.md "BListItem")* item);` `virtual [`BListItem`](BListItem.md "BListItem")* RemoveItem(int32 index);`

`virtual bool RemoveItems(int32 index,  
int32 count);`

These functions work like their [`BListView`](BListView.md "BListView") counterparts, except that:

  * They can remove items from any part of the list, including collapsed sections. The index counts all items assigned to the `BOutlineListView`; the specified item can be hidden.

  * If the item being removed is a superitem, they also remove all of its subitems.

Warning

![Warning](./images/admonitions/Stop_32.png)

The [`BListView`](BListView.md "BListView") versions of these functions will not produce reliable results when applied to a `BOutlineListView`, even if the item being removed is in an expanded section of the list and is not a superitem.

See also: [`BListView::RemoveItem()`](BListView.md#BListView_RemoveItem)

#### SortItemsUnder()

`void SortItemsUnder([`BListItem`](BListItem.md "BListItem")* underItem,  
bool oneLevelOnly,  
int (*compareFunc)(const [`BListItem`](BListItem.md "BListItem") *, const [`BListItem`](BListItem.md "BListItem") *));`

Sorts the items located under the specified `underItem`. If `oneLevelOnly` is `true`, only items located directly under the `underItem` are considered. If `oneLevelOnly` is `false`, subitems are scanned recursively and sorted.

The hierarchy of the list is ignored when sorting with `oneLevelOnly` set to `false`; items can (and will) be moved from one level to another. If you want to sort without moving items from one level to another, you should call `SortItemsUnder()` once for each superitem with `oneLevelOnly` set to `true`.

Note

![Note](./images/admonitions/Info_32.png)

When `oneLevelOnly` is `false`, `SortItemsUnder()` acts just like [`FullListSortItems()`](BOutlineListView.md#BOutlineListView_FullListSortItems), except the first item in the list that's considered is `underItem` instead of the first item in the full list.

#### Superitem()

`[`BListItem`](BListItem.md "BListItem")* Superitem(const [`BListItem`](BListItem.md "BListItem")* item);`

Returns the superitem for the `item` passed as an argument—that is, the item under which the argument `item` is grouped—or `NULL` if the `item` is at the outermost level of the outline (level 0) or isn't in the list.

* * *

### Static Functions

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Returns a new `BOutlineListView` object, allocated by new and created with the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive. However, this function returns `NULL` if the specified `archive` doesn't contain data for a `BOutlineListView` object.

See also: [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"), [`Archive()`](BOutlineListView.md#BOutlineListView_Archive "Archive\(\)")

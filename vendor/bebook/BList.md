<!-- Classes And Methods - The Support Kit - BList -->

## BList

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `support/List.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BList_Overview.md "BList")

  * [Constructor and Destructor](BList.md#BList_ConstructorDestructor)
  * [Member Functions](BList.md#BList_MemberFunctions)
  * [Operators](BList.md#BList_Operators)

  
  
* * *

### Constructor and Destructor

#### BList()

`BList(int32 count = 20);` `BList(const BList& anotherList);`

Initializes the `BList` by allocating enough memory to hold `count` items. As the list grows and shrinks, additional memory is allocated and freed in blocks of the same size.

The copy constructor creates an independent list of data pointers, but it doesn't copy the pointed-to data. For example:
    
    
    BList* newList = new BList(oldList);

Here, the contents of `oldList` and `newList`—the actual data pointers—are separate and independent. Adding, removing, or reordering items in `oldList` won't affect the number or order of items in `newList`. But if you modify the data that an item in `oldList` points to, the modification will be seen through the analogous item in `newList`.

The block size of a `BList` that's created through the copy constructor is the same as that of the original `BList`.

#### ~BList()

`virtual ~BList();`

Frees the list of data pointers, but doesn't free the data that they point to. To destroy the data, you need to free each item individually:
    
    
    void* anItem;
    for ( int32 i = 0; anItem = myList->ItemAt(i); i++ )
       delete anItem;
    
    delete myList;

See also: [`MakeEmpty()`](BList.md#BList_MakeEmpty "MakeEmpty\(\)")

* * *

### Member Functions

#### AddItem()

`bool AddItem(void* item);` `bool AddItem(void* item,  
int32 index);`

Adds an item to the `BList` at `index`—or, if no index is supplied, at the end of the list. If necessary, additional memory is allocated to accommodate the new item.

Adding an item never removes an item already in the list. If the item is added at an index that's already occupied, items currently in the list are bumped down one slot to make room.

If `index` is out of range (greater than the current item count, or less than zero), the function fails and returns `false`. Otherwise it returns `true`.

#### AddList()

`bool AddList(BList* item);` `bool AddList(BList* item,  
int32 index);`

Adds the contents of another `BList` to this `BList`. The items from the other `BList` are inserted at `index`—or, if no index is given, they're appended to the end of the list. If the index is out of range, the function fails and returns `false`. If successful, it returns `true`.

See also: [`AddItem()`](BList.md#BList_AddItem "AddItem\(\)")

#### CountItems()

`int32 CountItems() const;`

Returns the number of items currently in the list.

#### DoForEach()

`void DoForEach(bool (*func)(void * ));` `void DoForEach(bool (*func)(void * ),  
void* arg2);`

Calls the `func` function once for each item in the `BList`. Items are visited in order, beginning with the first one in the list (index 0) and ending with the last. If a call to `func` returns `true`, the iteration is stopped, even if some items have not yet been visited.

`func` must be a function that takes one or two arguments. The first argument is the currently-considered item from the list; the second argument, if `func` requires one, is passed to `DoForEach()` as `arg2`.

#### FirstItem()

`void* FirstItem() const;`

Returns the first item in the list, or `NULL` if the list is empty. This function doesn't remove the item from the list.

See also: [`LastItem()`](BList.md#BList_LastItem "LastItem\(\)"), [`ItemAt()`](BList.md#BList_ItemAt "ItemAt\(\)")

#### HasItem()

`bool HasItem(void* item) const;`

Returns `true` if item is in the list, and `false` if not.

#### IndexOf()

`int32 IndexOf(void* item) const;`

Returns the index where a particular item is located in the list, or a negative number if the item isn't in the list. If the item is in the list more than once, the index returned will be the position of its first occurrence.

#### IsEmpty()

`bool IsEmpty() const;`

Returns `true` if the list is empty (if it contains no items), and `false` otherwise.

See also: [`MakeEmpty()`](BList.md#BList_MakeEmpty "MakeEmpty\(\)")

#### ItemAt()

`void* ItemAt(int32 index) const;`

Returns the item at `index`, or `NULL` if the index is out of range. This function doesn't remove the item from the list.

See also: [`Items()`](BList.md#BList_Items "Items\(\)"), [`FirstItem()`](BList.md#BList_FirstItem "FirstItem\(\)"), [`LastItem()`](BList.md#BList_LastItem "LastItem\(\)")

#### Items()

`void* Items() const;`

Returns a pointer to the `BList`'s list. You can index directly into the list if you're certain that the index is in range:
    
    
    myType* item = (myType*) Items()[index];

#### LastItem()

`void* LastItem() const;`

Returns the last item in the list without removing it. If the list is empty, this function returns `NULL`.

See also: [`RemoveItem()`](BList.md#BList_RemoveItem "RemoveItem\(\) , RemoveItems\(\)"), [`FirstItem()`](BList.md#BList_FirstItem "FirstItem\(\)")

#### MakeEmpty()

`void MakeEmpty();`

Empties the `BList` of all its items, without freeing the data that they point to.

See also: [`IsEmpty()`](BList.md#BList_IsEmpty "IsEmpty\(\)"), [`RemoveItem()`](BList.md#BList_RemoveItem "RemoveItem\(\) , RemoveItems\(\)")

####  RemoveItem() , RemoveItems()

`bool RemoveItem(void* item);` `void* RemoveItem(int32 index);`

`bool RemoveItems(int32 index,  
int32 count);`

`RemoveItem()` removes an item from the list. If passed an `item`, the function looks for the item in the list, removes it, and returns `true`. If it can't find the item, it returns `false`. If the item is in the list more than once, this function removes only its first occurrence.

If passed an `index`, `RemoveItem()` removes the item at that index and returns it. If there's no item at the index, it returns `NULL`.

`RemoveItems()` removes a group of `count` items from the list, beginning with the item at `index`. If the index is out of range, it fails and returns `false`. Otherwise, it removes the items, without checking to be sure that the list actually holds that many items at the index, and returns `true`.

The list is compacted after an item is removed. Because of this, you mustn't try to empty a list (or a range within a list) by removing items at monotonically increasing indices. You should either start with the highest index and move towards the head of the list, or remove at the same index (the lowest in the range) some number of times. As an example of the latter, the following code removes the first five items in the list:
    
    
    for ( int32 i = 0; i < 5; i++ )
       myList->RemoveItem(0);

See also: [`MakeEmpty()`](BList.md#BList_MakeEmpty "MakeEmpty\(\)")

#### SortItems()

`void* SortItems(int (*compareFunc)(const void * , const void * ));`

Rearranges the items in the list. The items are sorted using the `compareFunc` comparison function passed as an argument. This function should return a negative number if the first item is ordered before the second, a positive number if the second is ordered before the first, and 0 if the two items are ordered equivalently.

The arguments passed to the comparison function are declared to be void*; however, they should be regarded as pointers to the items in the list—in other words, as pointers to pointers.

* * *

### Operators

#### = (assignment)

`BList& operator =(const BList& from);`

Copies the contents of one `BList` object into another:
    
    
    BList newList = oldList;

After the assignment, each object has its own independent copy of list data; destroying one of the objects won't affect the other.

Only the items in the list are copied, not the data they point to.

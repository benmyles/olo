<!-- System Overview - The Support Kit - BList -->

## BList

A [`BList`](BList.md "BList") object is a compact, ordered list of data pointers. [`BList`](BList.md "BList") objects can contain pointers to any type of data, including—and especially—objects.

An item assigned to a [`BList`](BList.md "BList") is identified by an index to its position in the list. Indices start at 0 and are neither arbitrary nor permanent. If, for example, you insert an item into the middle of a list, the indices of the items at the tail of the list are incremented (by one). Similarly, removing an item decrements the indices of the following items.

A [`BList`](BList.md "BList") stores its items as type void*, so it's necessary to cast an item to the correct type when you retrieve it. For example, items retrieved from a list of [`BBitmap`](BBitmap.md "BBitmap") objects must be cast as [`BBitmap`](BBitmap.md "BBitmap") pointers:
    
    
    BBitmap *theImage = (BBitmap *)myList->ItemAt(anIndex);

Warning

![Warning](./images/admonitions/Stop_32.png)

There's nothing to prevent you from adding a `NULL` pointer to a [`BList`](BList.md "BList"). However, functions that retrieve items from the list (such as [`ItemAt()`](BList.md#BList_ItemAt "ItemAt\(\)")) return `NULL` when the requested item can't be found. Thus, you can't distinguish between a valid `NULL` item and an invalid attempt to access an item that isn't there.

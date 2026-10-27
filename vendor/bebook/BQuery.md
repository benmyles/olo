<!-- Classes And Methods - The Storage Kit - BQuery -->

## BQuery

| Derived From:| None  
---|---  
Mix-in Classes:| None  
Declared In:| `storage/NodeInfo.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BQuery_Overview.md "BQuery")

  * [Constructor and Destructor](BQuery.md#BQuery_ConstructorDestructor)
  * [Member Functions](BQuery.md#BQuery_Member_Functions)
  * [Constants](BQuery.md#BQuery_Constants)

  
  
* * *

### Constructor and Destructor

#### BQuery()

`BQuery();`

Creates a new `BQuery` object. To use the object, you have to set its predicate and volume, and then tell it to [`Fetch()`](BQuery.md#BQuery_Fetch "Fetch\(\)"). If you want to fetch again, you have to call [`Clear()`](BQuery.md#BQuery_Clear "Clear\(\)") first (and re-set the predicate and volume.)

#### ~BQuery()

`~BQuery();`

Destroys the `BQuery`. If the query is live, the query is shot dead. You stop receiving live query updates when you delete the `BQuery` object.

* * *

### Member Functions

#### Clear()

`status_t Clear();`

Erases the `BQuery`'s predicate, sets the volume and target to `NULL`, and turns off live query updates (if the query is live). You call `Clear()` if you want to [`Fetch()`](BQuery.md#BQuery_Fetch "Fetch\(\)") more than once: You have to `Clear()` before each [`Fetch()`](BQuery.md#BQuery_Fetch "Fetch\(\)") (except the first).

`Clear()` always return `B_OK`.

####  CountEntries(), Rewind()

Warning

![Warning](./images/admonitions/Stop_32.png)

Don't use these functions. They're no-ops for the `BQuery` class.

#### Fetch()

`status_t Fetch();`

Tells the `BQuery` to go fetch the entries that satisfy the predicate. After you've fetched, you can retrieve the set of "static" entries through calls to [`GetNextEntry()`](BQuery.md#BQuery_GetNextEntry "GetNextEntry\(\), GetNextRef\(\), GetNextDirents\(\)"), [`GetNextRef()`](BQuery.md#BQuery_GetNextRef), or [`GetNextDirents()`](BQuery.md#BQuery_GetNextDirents).

If you've set the `BQuery`'s target, then this query is live. The live query update messages start rolling in when you tell the object to `Fetch()`. They stop when you [`Clear()`](BQuery.md#BQuery_Clear "Clear\(\)") or destroy the object.

The fetch fails if the object's predicate or volume isn't set, or if you've already fetched but haven' [`Clear()`](BQuery.md#BQuery_Clear "Clear\(\)")'d since then.

Warning

![Warning](./images/admonitions/Stop_32.png)

Every query must include at least one indexed attribute. If your predicate includes no indexed attributes, `Fetch()` will not balk—it returns `B_OK` (given that it doesn't otherwise fail). However, no entries will have been retrieved, and your subsequent `GetNext…()` call will fail (`B_BAD_VALUE`).

Return Code| Description  
---|---  
`B_OK`.| The fetch is running.  
`B_NO_INIT`.| The volume or predicate isn't set.  
`B_BAD_VALUE`.| The predicate is improper.  
`B_NOT_ALLOWED`.| You've already fetched; [`Clear()`](BQuery.md#BQuery_Clear "Clear\(\)") the object and start again.  
  
####  GetNextEntry(), GetNextRef(), GetNextDirents()

`virtual status_t GetNextEntry([`BEntry`](BEntry.md "BEntry")* entry,  
bool traverse = false);``virtual status_t GetNextRef(entry_ref* ref);``virtual int32 GetNextDirents(dirent* buf,  
size_t bufsize,  
int32 count = INT_MAX);`

These functions return the next entry in the "static" entry list; the list is created when you tell your (well-formed) `BQuery` to [`Fetch()`](BQuery.md#BQuery_Fetch "Fetch\(\)"). You can retrieve the entry as a [`BEntry`](BEntry.md "BEntry"), entry_ref, or dirent structure. The static entry list is the set of entries that initially satisfy the predicate; entries found by the live query mechanism are not included in this list.

When you reach the end of the entry list, the `Get…()` function returns an indicative value:

  * `GetNextRef()` and `GetNextEntry()` return `B_ENTRY_NOT_FOUND`.

  * `GetNextDirents()` returns 0.

You can only cycle over the list once; the [`Rewind()`](BQuery.md#BQuery_Rewind) function is not defined for `BQuery`. See the [`BEntryList`](BEntryList.md "BEntryList") class for more information on these functions.

`GetNextDirents()` returns the number of dirents it retrieved (currently, it can only retrieve one at a time. The other two functions return these codes:

Return Code| Description  
---|---  
`B_OK`.| The entry was retrieved.  
`B_ENTRY_NOT_FOUND`.| You're at the end of the list.  
`B_BAD_VALUE`.| The predicate includes unindexed attributes.  
`B_FILE_ERROR`.| The `BQuery` hasn't fetched.  
  
####  PushAttr() , PushOp(), PushUInt32(), PushInt32(), PushUInt64(), PushInt64(), PushFloat(), PushDouble(), PushString(), query_op

`void PushAttr(const char* attr_name);``void PushOp(query_op operator);``void PushUInt32(uint32 value);``void PushInt32(int32 value);``void PushUInt64(uint64 value);``void PushInt64(int64 value);``void PushFloat(float value);``void PushDouble(double value);``void PushString(const char* attr_name,  
bool case_insensitive = false);`

You use these functions to construct the `BQuery`'s predicate. They create a predicate expression by pushing attribute names, operators, and values in Reverse Polish Notation (post-fix) order.

  * `PushAttr()` pushes an attribute name.

  * `PushOp()` pushes one of the [query_op](BQuery.md#query_op "Query Operation Codes") operators.

  * The rest of the functions push values of the designated types.

For details on how the push method works, see "[Constructing a Predicate](BQuery_Overview.md#BQuery_Constructing_A_Predicate "Constructing a Predicate")."

The predicate that you construct through these functions can be returned as a string through the [`GetPredicate()`](BQuery.md#BQuery_GetPredicate) function.

####  SetTarget(), IsLive()

`status_t SetTarget([`BMessenger`](BMessenger.md "BMessenger") target);``bool IsLive() const;`

Sets the `BQuery`'s target. The target identifies the [`BLooper`](BLooper.md "BLooper")/[`BHandler`](BHandler.md "BHandler") pair (a la the [`BInvoker`](BInvoker.md "BInvoker") target protocol) that will receive subsequent live query update messages. Calling this function declares the query to be live.

If `target` is `NULL`, the `BQuery` is told to be "not live". However, you can only turn off liveness (in this way) before you [`Fetch()`](BQuery.md#BQuery_Fetch "Fetch\(\)"). In other words, if you set the target, and then call [`Fetch()`](BQuery.md#BQuery_Fetch "Fetch\(\)") and then call `SetTarget`(`NULL`), the `BQuery` will think that it (itself) is not live, but it really is.

`IsLive()` tells you if the `BQuery` is live. The "liveness" needn't be actuated yet—live queries don't start operating until you tell the `BQuery` to [`Fetch()`](BQuery.md#BQuery_Fetch "Fetch\(\)"). The live query is killed when you delete or [`Clear()`](BQuery.md#BQuery_Clear "Clear\(\)") the `BQuery` object.

Return Code| Description  
---|---  
`B_OK`.| The `target` was set (including set to `NULL`).  
`B_BAD_VALUE`.| `target` doesn't identify a proper looper/handler pair.  
`B_NOT_ALLOWED`.| You've already [`Fetch()`](BQuery.md#BQuery_Fetch "Fetch\(\)")'d; you need to [`Clear()`](BQuery.md#BQuery_Clear "Clear\(\)").  
  
####  SetPredicate(), GetPredicate(), PredicateLength()

`status_t SetPredicate(const char* expr);``status_t GetPredicate(char* buf,  
size_t length);``size_t PredicateLength();`

`SetPredicate()` sets the `BQuery`'s predicate as a string. Predicate strings can be simple, single comparison expressions:
    
    
    "name = fido"

Or they can be more complex:
    
    
    "((name = fid*) || (size > 500)) && (last_modified < 243567)"

For the complete rules on setting the predicate as a string, see "[Constructing a Predicate](BQuery_Overview.md#BQuery_Constructing_A_Predicate "Constructing a Predicate")."

You can also set the predicate through the `Push…()` functions. You can't combine the methods: Pushing the predicate always takes precedence over `SetPredicate()`, regardless of the order in which the methods are deployed.

`GetPredicate()` copies the predicate into `buf`; `length` gives the length of `buf`, in bytes. If you want to find out how much storage you need to allocate to accommodate the predicate, call `PredicateLength()` first.

If you set the predicate through the `Push…()` functions, `GetPredicate()` converts the pushed construction into a string, and returns a copy of the string to you.

`PredicateLength()` returns the length of the predicate string, regardless of how it's created.

Warning

![Warning](./images/admonitions/Stop_32.png)

`GetPredicate()` and `PredicateLength()` both clear the push stack. This is important, because it means that you can't build up a portion of your predicate, then call `GetPredicate()`, build a little more, look again, build some more, etc. When you call `GetPredicate()`, you're done. Your next step should be a [`Fetch()`](BQuery.md#BQuery_Fetch "Fetch\(\)")

Return Code| Description  
---|---  
`B_OK`.| The predicate was successfully set or gotten.  
`B_NO_INIT`.| (`Get`) The predicate isn't set.  
`B_BAD_VALUE`.| (`Get`) length is shorter than the predicate's length.  
`B_NOT_ALLOWED`.|  (`Set`) You've already [`Fetch()`](BQuery.md#BQuery_Fetch "Fetch\(\)")'d; you have to [`Clear()`](BQuery.md#BQuery_Clear "Clear\(\)").  
`B_NO_MEMORY`.| (`Set`) Not enough memory to store the predicate string.  
  
#### SetVolume()

`status_t SetVolume(const [`BVolume`](BVolume.md "BVolume")* volume);`

A query can only look in one volume at a time. This is where you set the volume that you want to look at.

Warning

![Warning](./images/admonitions/Stop_32.png)

Currently, `SetVolume()` doesn't complain if volume is invalid. However, the subsequent [`Fetch()`](BQuery.md#BQuery_Fetch "Fetch\(\)") will fail (`B_NO_INIT`).

Return Code| Description  
---|---  
`B_OK`.| The volume was set.  
`B_NOT_ALLOWED`.| You've already fetched, you need to [`Clear()`](BQuery.md#BQuery_Clear "Clear\(\)") before you can reset the volume.  
  
* * *

### Constants

####  Query Operation Codes

These constants define the operations that can be used to specify a query. They are used in conjunction with the push functions for constructing a query.

Constant| Operation  
---|---  
`B_EQ`| =  
`B_NE`| !=  
`B_GT`| >  
`B_LT`| <  
`B_GE`| >=  
`B_LE`| <=  
`B_CONTAINS`| string contains value ("*value*")  
`B_BEGINS_WITH`| string begins with value ("value*")  
`B_ENDS_WITH`| string ends with value ("*value")  
`B_AND`| &&  
`B_OR`| ||  
`B_NOT`| !

<!-- Classes And Methods - The Storage Kit - BEntryList -->

## BEntryList

| Derived From:| None  
---|---  
Mix-in Classes:| None  
Declared In:| `storage/EntryList.h`  
Library:|   
Allocation:| –  
[Class Overview](BEntryList_Overview.md "BEntryList")

  * [Member Functions](BEntryList.md#BEntryList_MemberFunctions)

  
  
* * *

### Member Functions

#### CountEntries()

`virtual int32 CountEntries() = 0;`

Returns the number of entries that are in the entry list.

Warning

![Warning](./images/admonitions/Stop_32.png)

For [`BQuery`](BQuery.md "BQuery") this is a no-op. Also, [`BDirectory`](BDirectory.md "BDirectory")'s implementation manipulates the entry list pointer; thus, you shouldn't call `CountEntries()` while you're iterating through the directory's entries.

####  GetNextEntry(), GetNextRef(), GetNextDirents()

`virtual status_t GetNextEntry([`BEntry`](BEntry.md "BEntry")* entry,  
bool traverse = false) = 0;``virtual status_t GetNextRef(entry_ref* ref) = 0;``virtual int32 GetNextDirents(dirent* buf,  
size_t bufsize,  
int32 count = INT_MAX) = 0;`

These functions return the "next" entry in the entry list as a [`BEntry`](BEntry.md "BEntry"), entry_ref, or dirent structure. The end of the list is signalled by…

  * `GetNextEntry()` and `GetNextRef()` return `B_ENTRY_NOT_FOUND`.

  * `GetNextDirents()` returns 0.

If the `traverse` argument is `true`, symlinks are traversed.

See the [class overview](BEntryList_Overview.md "BEntryList") for more information.

#### Rewind()

`virtual status_t Rewind() = 0;`

Rewinds the entry list pointer so it points to the first element in the list.

Warning

![Warning](./images/admonitions/Stop_32.png)

For [`BQuery`](BQuery.md "BQuery") this is a no-op.

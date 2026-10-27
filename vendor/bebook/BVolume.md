<!-- Classes And Methods - The Storage Kit - BVolume -->

## BVolume

| Derived From:| None  
---|---  
Mix-in Classes:| None  
Declared In:| `storage/Volume.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BVolume_Overview.md "BVolume")

  * [Constructor and Destructor](BVolume.md#BVolume_ConstructorDestructor)
  * [Member Functions](BVolume.md#BVolume_Member_Functions)
  * [Operators](BVolume.md#BVolume_Operators)

  
  
* * *

### Constructor and Destructor

#### BVolume()

`BVolume();` `BVolume(dev_t device);` `BVolume(BVolume& volume);`

Creates a new `BVolume` object and initializes it according to the argument. The status of the initialization is recorded by the [`InitCheck()`](BVolume.md#BVolume_InitCheck "InitCheck\(\)") function.

  * The default constructor does nothing and sets [`InitCheck()`](BVolume.md#BVolume_InitCheck "InitCheck\(\)") to `B_NO_INIT`.

  * The device constructor sets the `BVolume` to point to the volume represented by the argument. See the [`SetTo()`](BVolume.md#BVolume_SetTo "SetTo\(\), Unset\(\)") function for status codes.

  * The copy constructor sets the object to point to the same device as does the argument.

#### ~BVolume

`~BVolume();`

Destroys the `BVolume` object.

* * *

### Member Functions

####  Capacity(), FreeBytes()

`off_t Capacity() const;``off_t FreeBytes() const;`

Returns the volume's total storage capacity and the amount of storage that's currently unused. Both measurements are in bytes.

#### Device()

`dev_t Device() const;`

Returns the object's dev_t number.

#### GetIcon()

`status_t GetIcon([`BBitmap`](BBitmap.md "BBitmap")* icon,  
icon_size which) const;`

Returns the volume's icon in `icon`. which specifies the icon to retrieve, either `B_MINI_ICON` (16x16) or `B_LARGE_ICON` (32x32).

See also: [`get_device_icon()`](TheStorageKit_Functions.md#get_device_icon "get_device_icon\(\)")

#### GetName()

`status_t GetName(char* buffer) const;`

Copies the name of the volume into `buffer`.

#### GetRootDirectory()

`status_t GetRootDirectory([`BDirectory`](BDirectory.md "BDirectory")* dir) const;`

Initializes `dir` (which must be allocated) to refer to the volume's "root directory." The root directory stands at the "root" of the volume's file hierarchy. Note that this isn't necessarily the root of the entire file hierarchy.

#### InitCheck()

`status_t InitCheck() const;`

Returns the status of the last initialization (from either the constructor or [`SetTo()`](BVolume.md#BVolume_SetTo "SetTo\(\), Unset\(\)")).

####  IsPersistent(), IsRemovable(), IsReadOnly(), IsShared()

`bool IsPersistent() const;``bool IsRemovable() const;``bool IsReadOnly() const;``bool IsShared() const;`

These functions answer media-related questions about the volume:

  * `IsPersistent()`. Is the storage persistent (such as on a floppy or hard disk)?

  * `IsRemovable()`. Can the media be removed?

  * `IsReadOnly()`. Can it be read but not written to?

  * `IsShared()`. Is it accessed through the network (as opposed to being directly connected to this computer)?

####  KnowsAttr(), KnowsMime(), KnowsQuery()

`bool KnowsAttr() const;``bool KnowsMime() const;``bool KnowsQuery() const;`

These functions answer questions about the file system on the volume:

  * `KnowsAttr()`. Do its files accept attributes?

  * `KnowsMime()`. Does it use MIME to type files?

  * `KnowsQuery()`. Can it respond to queries?

####  SetTo(), Unset()

`status_t SetTo(dev_t dev);``void Unset();`

`SetTo()` initializes the `BVolume` object to represent the volume (device) identified by the argument.

`Unset()` uninitializes the `BVolume`.

* * *

### Operators

#### = (assignment)

`BVolume& operator =(const [`BEntry`](BEntry.md "BEntry")& volume);`

In the expression
    
    
    BVolume a = b;

`BVolume` `a` is initialized to refer to the same volume as `b`. To gauge the success of the assignment, you should call [`InitCheck()`](BVolume.md#BVolume_InitCheck "InitCheck\(\)") immediately afterwards. Assigning a `BVolume` to itself is safe.

Assigning from an uninitialized `BVolume` is "successful": The assigned-to `BVolume` will also be uninitialized (`B_NO_INIT`).

####  == , != (comparison)

`bool operator ==(const BVolume& volume) const;``bool operator !=(const BVolume& volume) const;`

Two `BVolume` objects are said to be equal if they refer to the same volume, or if they're both uninitialized.

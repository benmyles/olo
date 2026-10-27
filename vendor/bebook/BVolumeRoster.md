<!-- Classes And Methods - The Storage Kit - BVolumeRoster -->

## BVolumeRoster

| Derived From:| None  
---|---  
Mix-in Classes:| None  
Declared In:| `storage/VolumeRoster.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BVolumeRoster_Overview.md "BVolumeRoster")

  * [Constructor and Destructor](BVolumeRoster.md#BVolumeRoster_ConstructorDestructor)
  * [Member Functions](BVolumeRoster.md#BVolumeRoster_Member_Functions)

  
  
* * *

### Constructor and Destructor

#### BVolumeRoster()

`BVolumeRoster();`

Creates a new `BVolumeRoster` object. You don't have to "initialize" the object before using it (as you do with most other Storage Kit classes). You can call [`GetNextVolume()`](BVolumeRoster.md#BVolumeRoster_GetNextVolume "GetNextVolume\(\), Rewind\(\)") (or whatever) immediately after constructing.

#### ~BVolumeRoster()

`virtual ~BVolumeRoster();`

Destroys the object. If this `BVolumeRoster` object was watching volumes, the watch is called off.

* * *

### Member Functions

#### GetBootVolume()

`status_t GetBootVolume(BVolume* boot_vol);`

Initializes `boot_vol` to refer to the "boot volume." This is the volume that was used to boot the computer. `boot_vol` must be allocated before you pass it in. If the boot volume can't be found, the argument is uninitialized.

(Currently, this function looks for the volume that's mounted at `/boot`. The only way to fool the system into thinking that there isn't a boot volume is to rename `/boot`—not a smart thing to do.)

Return Code| Description  
---|---  
`B_NO_ERROR`.| The boot volume was successfully retrieved.  
`B_ENTRY_NOT_FOUND`.| The boot volume wasn't found.  
  
####  GetNextVolume(), Rewind()

`status_t GetNextVolume(BVolume* volume);``void Rewind();`

`GetNextVolume()` retrieves the "next" volume from the volume list and uses it to initialize the argument (which must be allocated). When the function return `B_BAD_VALUE`, you've reached the end of the list.

`Rewind()` rewinds the volume list such that the next `GetNextVolume()` will return the first element in the list.

Return Code| Description  
---|---  
`B_NO_ERROR`.| The next volume was successfully retrieved.  
`B_BAD_VALUE`.| You've reached the end of the volume list.  
  
####  StartWatching(), StopWatching(), Messenger()

`status_t StartWatching([`BMessenger`](BMessenger.md "BMessenger")* messenger = be_app_messenger );``void StopWatching();``[`BMessenger`](BMessenger.md "BMessenger") Messenger() const;`

These functions start and stop the `BVolumeRoster`'s volume-watching facility. (This is actually just a convenient cover for the Node Monitor.)

  * `StartWatching()` registers a request for notifications of volume mounts and unmounts. The notifications are sent (as [`BMessage`](BMessage.md "BMessage")s) to the [`BHandler`](BHandler.md "BHandler") / [`BLooper`](BLooper.md "BLooper") pair specified by the argument. There are separate messages for mounting and unmounting; their formats are described below. The caller retains possession of the [`BHandler`](BHandler.md "BHandler") / [`BLooper`](BLooper.md "BLooper") that the [`BMessenger`](BMessenger.md "BMessenger") represents. The volume watching continues until this `BVolumeRoster` object is destroyed, or until you call…

  * `StopWatching()`. This function tells the volume-watcher to stop watching. In other words, notifications of volume mounts and unmounts are no longer sent to the `BVolumeRoster`'s target.

  * `Messenger()` returns a copy of the [`BMessenger`](BMessenger.md "BMessenger") object that was set in the previous `StartWatching()` call.

There are separate notifications ([`BMessage`](BMessage.md "BMessage")s) for volume-mounted and volume-unmounted events. See the `B_DEVICE_MOUNTED` and `B_DEVICE_UNMOUNTED` descriptions in "[The Node Monitor](The_Node_Monitor.md "The Node Monitor")" section of this chapter.

Return Code| Description  
---|---  
`B_NO_ERROR`.| The volume-watcher was successfully started or stopped.  
`B_BAD_VALUE`.| Poorly formed [`BMessenger`](BMessenger.md "BMessenger").  
`B_NO_MEMORY`.| Couldn't allocate resources.

<!-- System Overview - The Storage Kit - BVolumeRoster -->

## BVolumeRoster  
  
The `BVolumeRoster` class keeps track of the volumes that are mounted in the file system hierarchy. It lets you know about volumes in two ways:

  * It lists the volumes that are currently mounted. You can step through the list through iterative calls to the [`GetNextVolume()`](BVolumeRoster.md#BVolumeRoster_GetNextVolume "GetNextVolume\(\), Rewind\(\)") function.

  * It lets you know when new volumes are mounted, and when existing volumes are unmounted. (See [`StartWatching()`](BVolumeRoster.md#BVolumeRoster_StartWatching "StartWatching\(\), StopWatching\(\), Messenger\(\)").)

How you create your `BVolumeRoster` object depends on what you're going to do with it:

  * If you simply want to step through the volume list, then creating a `BVolumeRoster` on the stack is sufficient.

  * However, if you want to watch for volumes being mounted and unmounted, then you must keep your `BVolumeRoster` object around. The watching stops when the object is deleted.

A single `BVolumeRoster` object can perform both functions: You can use it to step through the volume list at the same time that it's watching volumes.

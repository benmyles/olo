<!-- Classes And Methods - The Game Kit - BGameSound -->

## BGameSound

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `game/GameSound.h`  
Library:| `libgame.so`  
Allocation:| Constructor only  
[Class Overview](BGameSound_Overview.md "BGameSound")

  * [Constructor and Destructor](BGameSound.md#BGameSound_ConstructorDestructor)
  * [Member Functions](BGameSound.md#BGameSound_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BGameSound()

`BGameSound(BGameSoundDevice* device = NULL);`

Initializes the sound object.

Note

![Note](./images/admonitions/Info_32.png)

Currently, `device` must always be `NULL`.

#### ~BGameSound

`virtual ~BGameSound();`

Releases any memory used by the `BGameSound`.

If your node has created and set [`BBufferGroup`](BBufferGroup.md "BBufferGroup")s for any producers, you should delete them in the destructor.

* * *

### Member Functions

#### Clone()

`virtual BGameSound* Clone() const = 0;`

Returns a copy of the game sound object. Not implemented in this base class; derived classes must implement it.

#### Device()

`BGameSoundDevice* Device() const;`

Returns a pointer to the `BGameSoundDevice` responsible for playing the sound. This is `NULL` if the default device is being used.

#### Format()

`const [gs_audio_format](TheGameKit_DefinedTypes.md#gs_audio_format "gs_audio_format")& Format() const;`

Returns a [gs_audio_format](TheGameKit_DefinedTypes.md#gs_audio_format "gs_audio_format") structure describing the format of the sound.

#### ID()

`[gs_id](TheGameKit_DefinedTypes.md#gs_id "gs_id") ID() const;`

Returns the ID of the sound attached to the `BGameSound` object. This is 0 if no sound has been selected.

#### Init()

##### protected

`status_t Init([gs_id](TheGameKit_DefinedTypes.md#gs_id "gs_id") soundHandle);`

Specifies the sound to be played by the object.

Return Code| Description  
---|---  
`B_OK`.| The initialization was successful.  
`B_ERROR`.| The constructor failed.  
Other errors.| The player may return other errors.  
  
#### InitCheck()

`status_t InitCheck() const;`

Returns a status_t indicating whether or not the object was successfully initialized. A return value of `B_OK` means everything's fine; any other value means an error occurred in the constructor.

#### IsPlaying()

`virtual bool IsPlaying();`

Returns `true` if the sound is currently playing; otherwise this function returns `false`.

#### LockMemoryPool()

`static status_t LockMemoryPool(bool lockInCore);`

This function lets you specify whether or not the `BGameSound`'s memory pool (from which buffers are drawn) is locked in memory. Locking the pool may increase performance, but will increase your memory requirements and may reduce the ability of the computer to use virtual memory efficiently.

Note

![Note](./images/admonitions/Info_32.png)

You can only set the lock state from within the constructor of a derived class. Once the state has been set, it can't be changed.

Return Code| Description  
---|---  
`B_OK`.|  The pool's lock state has been set.  
`EALREADY`.| The lock state has already been set.  
  
####  SetAttributes(), GetAttributes()

`virtual status_t SetAttributes([gs_attribute](TheGameKit_DefinedTypes.md#gs_attribute "gs_attribute")* inAttributes,  
size_t inAttributeCount);` `virtual status_t GetAttribute([gs_attribute](TheGameKit_DefinedTypes.md#gs_attribute "gs_attribute")* ioAttributes,  
size_t inAttributeCount);`

`SetAttributes()` sets the sound's attributes to match those in the `inAttributes` array; `inAttributeCount` indicates how many attributes are in the list.

`GetAttributes()` returns the sound's current attributes. The `ioAttributes` list contains a list of `inAttributeCount` [gs_attribute](TheGameKit_DefinedTypes.md#gs_attribute "gs_attribute") structures; each entry indicates in its attribute field which attribute is to be stuffed into that slot in the array. On return, these attributes are filled out.

Return Code| Description  
---|---  
`B_OK`.| The attribute changes were recorded.  
`B_ERROR`.| The player wasn't initialized properly.  
  
####  SetGain(), Gain()

`status_t SetGain(float gain,  
bigtime_t rampDuration = 0);``float Gain();`

`SetGain()` sets the sound's `gain` (volume). If `rampDuration` is nonzero, the change in `gain` will take `rampDuration` to occur—the gain will fade between the current value and the new value over the specified number of microseconds. If `rampDuration` is 0, the change will be immediate.

`Gain()` returns the sound's current gain.

The gain ranges from 0.0 (silent) to 1.0 (maximum gain).

Return Code| Description  
---|---  
`B_OK`.| The gain change was recorded.  
`B_ERROR`.| The object hasn't been initialized properly.  
Other errors.| The derived class may return other appropriate errors.  
  
#### SetInitError()

##### protected

`status_t SetInitError(status_t initError);`

Sets the error code that [`InitCheck()`](BGameSound.md#BGameSound_InitCheck "InitCheck\(\)") will return. Derived classes should call this from their constructors.

#### SetMaxSoundCount()

`static int32 SetMaxSoundCount(int32 maxSoundCount);`

This function lets you specify the maximum number of `BGameSound`-derived objects that can be in use at once. This value must be between 32 and 1024; if it's outside this range, it gets pinned to 32 or 1024. The actual value set is returned.

#### SetMemoryPoolSize()

`status_t SetMemoryPoolSize(size_t poolSize);`

Sets the size of the memory pool from which sound buffers can be obtained to the number of bytes specified by `poolSize`. This value must be between 128kB and 4MB, or `B_BAD_VALUE` will be returned.

Note

![Note](./images/admonitions/Info_32.png)

You can't change the size of the memory pool once it's been allocated.

In general you'll only call this function if you're implementing a new `BGameSound` subclass.

Return Code| Description  
---|---  
`B_OK`.| The memory pool size has been set.  
`B_BAD_VALUE`.| The requested pool size is invalid.  
`EALREADY`.| The memory pool has already been allocated; you can't change its size.  
  
####  SetPan(), Pan()

`status_t SetPan(float pan,  
bigtime_t rampDuration = 0);``float Pan();`

`SetPan()` sets the sound's pan. This value, which ranges from -1.0 to 1.0, indicates the apparent position of the sound (from absolute left to absolute right). A value of 0 indicates that the sound should appear to come from directly in front of the user. If `rampDuration` is nonzero, the change in position will take `rampDuration` to occur—the sound will seem to move from the initial position to the new position over the specified number of microseconds. If `rampDuration` is 0, the change will be immediate.

`Pan()` returns the sound's current pan.

Return Code| Description  
---|---  
`B_OK`.| The pan change was recorded.  
`B_ERROR`.| The object hasn't been initialized properly.  
Other errors.| The derived class may return other appropriate errors.  
  
####  StartPlaying(), StopPlaying()

`virtual status_t StartPlaying();``virtual status_t StopPlaying();`

`StartPlaying()` begins playback of the sound. `StopPlaying()` ends playback.

If you call `StartPlaying()` on a sound that's already playing, it stops and restarts from the beginning.

These return `B_OK` if all is well, or an appropriate error code if not; the error depends on the derived class being used.

Return Code| Description  
---|---  
`B_OK`.| No error.  
`EALREADY`.| The sound is already playing (if you called `StartPlaying()`) or not playing (if you called `StopPlaying()`).  
Other errors,| As determined by the subclass.

<!-- Classes And Methods - The Game Kit - BPushGameSound -->

## BPushGameSound

| Derived From:| [BStreamingGameSound](BStreamingGameSound.md "BStreamingGameSound")  
---|---  
Mix-in Classes:| –  
Declared In:| `game/PushGameSound.h`  
Library:| `libgame.so`  
Allocation:| Constructor only  
[Class Overview](BPushGameSound_Overview.md "BPushGameSound")

  * [Constructor and Destructor](BPushGameSound.md#BPushGameSound_ConstructorDestructor)
  * [Member Functions](BPushGameSound.md#BPushGameSound_MemberFunctions)
  * [Constants](BPushGameSound.md#BPushGameSound_Constants)

  
  
* * *

### Constructor and Destructor

#### BPushGameSound()

`BPushGameSound(size_t inBufferFrameCount,  
const [gs_audio_format](TheGameKit_DefinedTypes.md#gs_audio_format "gs_audio_format")* format,  
size_t inBufferCount = 2,  
BGameSoundDevice* device = NULL);`

Prepares the object to play audio pushed by your application. `inBufferFrameCount` specifies the number of frames each audio buffer should be able to hold. `format` indicates the audio format that will be streamed. `inBufferCount` specifies the number of buffers to use, and, as always, `device` is the sound device to use for playback.

Note

![Note](./images/admonitions/Info_32.png)

Currently, `device` must always be `NULL` to indicate that the default playback device should be used.

By default, two audio buffers are used.

Be sure to call [`InitCheck()`](BPushGameSound.md#BPushGameSound_InitCheck "InitCheck\(\)") before using the `BPushGameSound` object.

#### ~BPushGameSound

`virtual ~BPushGameSound();`

Deletes the semaphore used to lock the object.

* * *

### Member Functions

#### InitCheck()

`status_t InitCheck() const;`

Returns a status_t indicating whether or not the object was successfully initialized. A return value of `B_OK` means everything's fine; any other value means an error occurred in the constructor.

####  LockForCyclic(), UnlockCyclic(), CurrentPosition()

`virtual [lock_status](BPushGameSound.md#Enums_lock_status "lock_status") LockForCyclic(void** outBasePtr,  
size_t* outSize);``virtual status_t UnlockCyclic();``virtual size_t CurrentPosition();`

`LockForCyclic()` gives you access to the entire sound buffer; audio playback continues while you have access. Use `CurrentPosition()` to determine where the playback is currently located in the buffer area, and be sure to stay well ahead of it. See the [overview](BPushGameSound_Overview.md "BPushGameSound") for a more in-depth discussion. If `lock_failed` is returned, you can't access the audio area, but if `lock_ok` is returned, you can start pushing audio.

On return, `outBasePtr` points to the first byte of the audio buffer area, and `outSize` indicates the total size of the audio buffer area.

`UnlockCyclic()` releases the audio area.

Return Code| Description  
---|---  
`B_OK`.| `UnlockCyclic()` succeeded.  
  
####  LockNextPage(), UnlockPage()

`virtual [lock_status](BPushGameSound.md#Enums_lock_status "lock_status") LockNextPage(void** outPagePtr,  
size_t* outPageSize);``virtual status_t UnlockPage(void* inPagePtr);`

`LockNextPage()` requests access to the next page of the sound buffer. The [lock_status](BPushGameSound.md#Enums_lock_status "lock_status") return value is `lock_ok` or `lock_ok_frames_dropped` if a valid page has been returned. If either of these values is returned, you can then write up to `outPageSize` bytes of audio data into the audio page returned in `outPagePtr`.

If `lock_failed` is returned, you can't access an audio page.

`UnlockPage()` releases the audio page pointed to by `inPagePtr` and returns `B_OK` if it succeeds in unlocking the page.

* * *

### Constants

####  lock_status

Constant| Description  
---|---  
`lock_failed`| Couldn't get the lock; it's not time to update yet.  
`lock_ok`| Locked; you can update.  
`lock_ok_frames_dropped`| Locked; you can update, but you may have missed some buffers.  
  
These values are returned by the locking functions in the `BPushGameSound` class ( [`LockNextPage()`](BPushGameSound.md#BPushGameSound_LockNextPage "LockNextPage\(\), UnlockPage\(\)") and [`LockForCyclic()`](BPushGameSound.md#BPushGameSound_LockForCyclic "LockForCyclic\(\), UnlockCyclic\(\), CurrentPosition\(\)")). If either function returns `lock_failed`, there isn't a buffer to be filled; your code is pushing buffers too fast.

If `lock_ok` is returned, the next buffer is ready. The `lock_ok_frames_dropped` result is received if a buffer is available but some buffers have been lost; this can happen if you're not pushing fast enough.

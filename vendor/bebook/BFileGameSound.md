<!-- Classes And Methods - The Game Kit - BFileGameSound -->

## BFileGameSound

| Derived From:| [BStreamingGameSound](BStreamingGameSound.md "BStreamingGameSound")  
---|---  
Mix-in Classes:| –  
Declared In:| `game/FileGameSound.h`  
Library:| `libgame.so`  
Allocation:| Constructor only  
[Class Overview](BFileGameSound_Overview.md "BFileGameSound")

  * [Constructor and Destructor](BFileGameSound.md#BFileGameSound_ConstructorDestructor)
  * [Member Functions](BFileGameSound.md#BFileGameSound_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BFileGameSound()

`BFileGameSound(const entry_ref* inFile,  
bool looping = true,  
BGameSoundDevice* device = NULL);` `BFileGameSound(const char* inFile,  
bool looping = true,  
BGameSoundDevice* device = NULL);`

Prepares the object to play the specified sound file, which can be specified either by entry_ref or pathname in the `inFile` argument.

If the `looping` flag is `true` (which it is by default), the sound automatically loops back to the beginning and replays when the end of the sound is reached. This is useful for easily playing background music (for example).

In both cases, `device` specifies the sound device that should be used for playing the sound; `NULL` uses the default sound player.

Note

![Note](./images/admonitions/Info_32.png)

Currently, `device` must always be `NULL`.

After instantiating the `BFileGameSound` object, you should call [`InitCheck()`](BFileGameSound.md#BFileGameSound_InitCheck "InitCheck\(\)") to determine whether or not the sound was successfully created.

#### ~BFileGameSound

`virtual ~BFileGameSound();`

A typical destructor.

* * *

### Member Functions

#### InitCheck()

`status_t InitCheck() const;`

Returns a status_t indicating whether or not the object was successfully instantiated.

Return Code| Description  
---|---  
`B_OK`.| The sound was successfully initialized.  
`B_ERROR`.| Unable to create a sound player.  
`B_NO_MEMORY`.| Can't get enough memory to preload the sound.  
Other errors.| The sound player may return errors.  
  
#### Preload()

`status_t Preload();`

`Preload()` preloads enough of the sound file into memory that starting playback of the sound won't cuase a delay while the first chunk of data is fetched from disk.

Return Code| Description  
---|---  
`B_OK`.| Preloading was successful.  
Port errors.| Unable to communicate with the streaming sound port.  
  
####  SetPaused(), IsPaused()

`status_t SetPaused(bool isPaused,  
bigtime_t rampTime);``int32 IsPaused();`

`SetPaused()` pauses if `isPaused` is `true`, and unpauses if `isPaused` is `false`. If `rampTime` is nonzero, the sound ramps up to speed (or down to stopped) instead of instantly changing state. The number of microseconds specified by `rampTime` indicates how long the change should take to complete.

`IsPaused()` returns value indicating whether or not the sound is paused, or if a pause is in process of being initiated (if ramping is underway). The result is one of these values:

Constant| Description  
---|---  
`B_NOT_PAUSED`| The sound is playing normally.  
`B_PAUSE_IN_PROGRESS`| The sound is ramping toward or away from a paused state.  
`B_PAUSED`| The sound is paused.  
  
Because these constants are members of the `BFileGameSound` class, be sure to refer to them as `BFileGameSound`::`B_NOT_PAUSED` and so forth.

Return Code| Description  
---|---  
`B_OK`.| The pause occurred without error.  
`EALREADY`.| The sound is already in the requested state.

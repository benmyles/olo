<!-- Classes And Methods - The Game Kit - BSimpleGameSound -->

## BSimpleGameSound

| Derived From:| [BGameSound](BGameSound.md "BGameSound")  
---|---  
Mix-in Classes:| –  
Declared In:| `game/SimpleGameSound.h`  
Library:| `libgame.so`  
Allocation:| Constructor only  
[Class Overview](BSimpleGameSound_Overview.md "BSimpleGameSound")

  * [Constructor and Destructor](BSimpleGameSound.md#BSimpleGameSound_ConstructorDestructor)
  * [Member Functions](BSimpleGameSound.md#BSimpleGameSound_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BSimpleGameSound()

`BSimpleGameSound(const entry_ref* inFile,  
BGameSoundDevice* device = NULL);` `BSimpleGameSound(const char* inFile,  
BGameSoundDevice* device = NULL);` `BSimpleGameSound(const void* inData,  
size_t inFrameCount,  
const [gs_audio_format](TheGameKit_DefinedTypes.md#gs_audio_format "gs_audio_format")* format,  
BGameSoundDevice* device = NULL);` `BSimpleGameSound(const BSimpleGameSound& other);`

Prepares the object to play the specified sound. The first form of the constructor preloads the entire sound specified by `inFile` into memory, while the second accepts a `inFile` as a pathname string.

The third form takes `inData` as a pointer to sound data already in memory; this sound data is copied into a buffer owned by the `BSimpleGameSound` object; once the constructor returns, you can delete the original data if you wish. The data is copied since some sound cards have an onboard sound buffer, and the API allows support for these devices (whether or not drivers exist that do this is another story).

`inFrameCount` indicates how many frames of audio are in that buffer, and `format` indicates the format of the audio data. The size of the [Media Kit](TheMediaKit.md "The Media Kit") buffers used by the object is determined by the [gs_audio_format](TheGameKit_DefinedTypes.md#gs_audio_format "gs_audio_format") structure's `buffer_size` field. If the value this field is zero, the [Game Kit](TheGameKit.md "The Game Kit") determines an appropriate buffer size for you.

In both cases, `device` specifies the sound device that should be used for playing the sound; `NULL` uses the default sound player.

Note

![Note](./images/admonitions/Info_32.png)

Currently, `device` must always be `NULL`.

The final form of the constructor duplicates another `BSimpleGameSound` object.

After instantiating the `BSimpleGameSound` object, you should call [InitCheck()](BSimpleGameSound.md#BSimpleGameSound_InitCheck "InitCheck\(\)") to determine whether or not the sound was successfully created.

#### ~BSimpleGameSound

`virtual ~BSimpleGameSound();`

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
  
####  SetIsLooping(), IsLooping()

`status_t SetIsLooping(bool looping);``void IsLooping() const;`

`SetIsLooping()` turns looping of the sound on if `looping` is `true`, or off if `looping` is `false`.

`IsLooping()` returns a flag indicating whether or not looping is currently enabled.

Return Code| Description  
---|---  
`B_OK`.| Looping was turned on or off without error.  
`B_ERROR`.| The player wasn't initialized properly.

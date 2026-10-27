<!-- Classes And Methods - The Game Kit - BStreamingGameSound -->

## BStreamingGameSound

| Derived From:| [BGameSound](BGameSound.md "BGameSound")  
---|---  
Mix-in Classes:| –  
Declared In:| `game/StreamingGameSound.h`  
Library:| `libgame.so`  
Allocation:| –  
[Class Overview](BStreamingGameSound_Overview.md "BStreamingGameSound")

  * [Constructor and Destructor](BStreamingGameSound.md#BStreamingGameSound_ConstructorDestructor)
  * [Member Functions](BStreamingGameSound.md#BStreamingGameSound_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BStreamingGameSound()

`BStreamingGameSound(size_t inBufferFrameCount,  
const [gs_audio_format](TheGameKit_DefinedTypes.md#gs_audio_format "gs_audio_format")* format,  
size_t inBufferCount = 2,  
BGameSoundDevice* device = NULL);` `protected BStreamingGameSound(BGameSoundDevice device);`

Prepares the object to play streamed audio. `inBufferFrameCount` specifies the number of frames each audio buffer should be able to hold. `format` indicates the audio format that will be streamed. `inBufferCount` specifies the number of buffers to use, and, as always, `device` is the sound device to use for playback.

By default, two audio buffers are used.

Note

![Note](./images/admonitions/Info_32.png)

Currently, `device` must always be `NULL` to indicate that the default playback device should be used.

#### ~BStreamingGameSound

`virtual ~BStreamingGameSound();`

Stops playing the sound.

* * *

### Member Functions

#### FillBuffer()

`virtual void FillBuffer(void* inBuffer,  
size_t byteCount);`

Fills the buffer specified by `inBuffer` with `byteCount` bytes of audio data.

In the `BStreamingGameSound` implemenation, this function calls the stream hook, if one exists.

See also: [`SetStreamHook()`](BStreamingGameSound.md#BStreamingGameSound_SetStreamHook "SetStreamHook\(\)")

#### InitCheck()

`status_t InitCheck() const;`

Returns a status_t indicating whether or not the object was successfully instantiated.

Return Code| Description  
---|---  
`B_OK`.| The sound was successfully initialized.  
`B_ERROR`.| Unable to create a sound player.  
Other errors.| The sound player may return errors.  
  
See also: the `BStreamingGameSound` [constructor](BStreamingGameSound.md#BStreamingGameSound_Constructor "BStreamingGameSound\(\)")

####  Lock(), Unlock()

`bool Lock();``void Unlock();`

`Lock()` locks the `BStreamingGameSound` to prevent unpleasant collisions in the land of multithreadedness; it returns `true` if it was able to lock the object, otherwise `false` is returned. `Unlock()` releases the lock.

#### SetAttributes()

`virtual status_t SetAttributes([gs_attribute](TheGameKit_DefinedTypes.md#gs_attribute "gs_attribute")* inAttributes,  
size_t inAttributeCount);`

`SetAttributes()` is implemented to disallow the `B_GS_LOOPING` attribute, since streamed sounds can't loop.

See also: [`BGameSound::SetAttributes()`](BGameSound.md#BGameSound_SetAttributes "SetAttributes\(\), GetAttributes\(\)")

#### SetParameters()

##### protected

`virtual status_t SetParameters(size_t inBufferFrameCount,  
const [gs_audio_format](TheGameKit_DefinedTypes.md#gs_audio_format "gs_audio_format")* fmt,  
size_t inBufferCount);`

Changes the `BStreamingGameSound` object's parameters. This lets you change the buffer size, audio format, and number of buffers after the object has been instantiated.

Return Code| Description  
---|---  
`B_OK`.| No error.  
`B_ERROR`.| Couldn't change the parameters.  
  
#### SetStreamHook()

`virtual status_t SetStreamHook(void  (*hook)(void * cookie, void* inBuffer, size_t byteCount, BStreamingGameSound* object),  
void* cookie);`

Specfies the hook function to be called to fill buffers with audio data.

The inputs to the hook function are:

Parameter| Description  
---|---  
`cookie`| A user-defined value. Indicates the cookie pointer that will be passed to the hook function.  
`inBuffer`| A pointer to the buffer to be filled with audio data.  
`byteCount`| The size of the buffer in bytes.  
`object`| A pointer to the `BStreamingGameSound` object.  
  
And returns...

Return Code| Description  
---|---  
`B_OK`.| The hook function was set without error.

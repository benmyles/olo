<!-- Classes And Methods - The Media Kit - BSoundPlayer -->

## BSoundPlayer

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `media/SoundPlayer.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BSoundPlayer_Overview.md "BSoundPlayer")

  * [Constructor and Destructor](BSoundPlayer.md#BSoundPlayer_ConstructorDestructor)
  * [Member Functions](BSoundPlayer.md#BSoundPlayer_MemberFunctions)
  * [Constants](BSoundPlayer.md#BSoundPlayer_Constants)
  * [Defined Types](BSoundPlayer.md#BSoundPlayer_DefinedTypes)

  
  
* * *

### Constructor and Destructor

#### BSoundPlayer()

`BTimeSource(const char* name = NULL,  
void  (*PlayBuffer) (void* , void* buffer, size_t size, const media_raw_audio_format& format) = NULL,  
void  (*Notifier) (void* , sound_player_notification what) = NULL,  
void* cookie = NULL);` `BTimeSource(const media_raw_audio_format* format,  
const char* name = NULL,  
void  (*PlayBuffer) (void* , void* buffer, size_t size, const media_raw_audio_format& format) = NULL,  
void  (*Notifier) (void* , sound_player_notification what) = NULL,  
void* cookie = NULL);` `BTimeSource(const media_node& toNode,  
const media_multi_audio_format* format = NULL,  
const char* name = NULL,  
const media_input* input = NULL,  
void  (*PlayBuffer) (void* , void* buffer, size_t size, const media_raw_audio_format& format) = NULL,  
void  (*Notifier) (void* , sound_player_notification what) = NULL,  
void* cookie = NULL);`

Initializes the `BSoundPlayer` object. The `name` argument specifies the name to be assigned to the sound player node (if you specify `NULL`, a generic name will be assigned).

The `PlayBuffer` argument specifies a pointer to a member function that processes data and inserts it into buffers for playback; specify `NULL` if you want to use the BSoundPlayer for playing BSounds. The parameters to the `PlayBuffer` function are (in order):

  * Pointer to the cookie.

  * Pointer to the buffer to play.

  * Size of the buffer.

  * Format of the audio data in the buffer.

The `Notifier` parameter specifies a pointer to a member function that receives notifications when events of interest occur, such as playback starting or stopping. Specify `NULL` to use the default notification handler. There are three possible notifications:

Constant| Description  
---|---  
`B_STARTED` and `B_STOPPED`| Indicate that the `BSoundPlayer` was started or stopped. These receive a pointer to the `BSoundPlayer` object as an argument.  
`B_SOUND_DONE`| Indicates that a sound has finished playing. In this case, there are two arguments: the first is a play_id indicating which sound finished playing, and the other is a boolean which is `true` if the sound played at all, and `false` if the sound never played.  
  
Note

![Note](./images/admonitions/Info_32.png)

If the callback handlers are members of a class, they must be static members.

The `cookie` parameter is a pointer that you can use for your own purposes; it's most useful if you're using a custom `PlayBuffer` or `Notifier`.

The second form of the constructor lets you specify in the `format` argument the format of the audio that the `BSoundPlayer` will perform. Since `BSoundPlayer` can only play raw sound formats, this is specified using the [media_raw_audio_format](TheMediaKit_DefinedTypes.md#media_raw_audio_format "media_raw_audio_format") structure.

The third form of the constructor lets you specify a node through which the sound should be played, and also uses a [media_multi_audio_format](TheMediaKit_DefinedTypes.md#media_multi_audio_format "media_multi_audio_format") to specify the sound's format, instead of the older [media_raw_audio_format](TheMediaKit_DefinedTypes.md#media_raw_audio_format "media_raw_audio_format").

You should call [`InitCheck()`](BSoundPlayer.md#BSoundPlayer_InitCheck "InitCheck\(\)") before using your `BSoundPlayer` object; this will let you determine whether or not the object was successfully constructed. One situation in which [`InitCheck()`](BSoundPlayer.md#BSoundPlayer_InitCheck "InitCheck\(\)") might indicate an error is if the user doesn't have a sound card installed.

#### ~BSoundPlayer()

`~BSoundPlayer();`

Stops playback, if sound is playing, releases references to any BSound objects that are in use by the BSoundPlayer, and frees all memory used by the `BSoundPlayer`.

* * *

### Member Functions

####  BufferPlayer(), SetBufferPlayer()

`BufferPlayerFunc BufferPlayer() const;``void SetBufferPlayer(void  (*PlayBuffer) (void* , void* buffer, size_t size, const media_raw_audio_format& format));`

`BufferPlayer()` returns a pointer to the current play buffer function, or `NULL` if the default player is in use.

`SetBufferPlayer()` lets you change the play buffer function.

####  Cookie(), SetCookie()

`void* Cookie() const;``void SetCookie(void* cookie);`

`Cookie()` returns the current cookie assigned to the `BSoundPlayer`.

`SetCookie()` lets you change the cookie assigned to the `BSoundPlayer`.

####  CurrentTime(), PerformanceTime()

`bigtime_t CurrentTime();``bigtime_t PerformanceTime();`

`CurrentTime()` returns the current media time, and `PerformanceTime()` returns the current performance time of the sound player node being used by the `BSoundPlayer`.

`PerformanceTime()` will return `B_ERROR` if the `BSoundPlayer` object hasn't been properly initialized.

####  EventNotifier(), SetEventNotifier()

`EventNotifierFunc EventNotifier() const;``void SetNotifier(void (*Notifier)(void* , sound_player_notification what, ...)));`

`EventNotifier()` returns a pointer to the current event notification handler function, or `NULL` if the default player is in use.

`SetNotifier()` lets you change the event notification handler function.

#### Format()

`media_raw_audio_format Format() const;`

Returns the `BSoundPlayer`'s format. Since the sound is always a raw sound format, the [media_raw_audio_format](TheMediaKit_DefinedTypes.md#media_raw_audio_format "media_raw_audio_format") structure is used.

#### GetVolumeInfo()

`EventNotifierFunc EventNotifier(media_node* outNode,  
int32* outParameter,  
float* outMinDB,  
float* outMaxDB) const;`

Returns information about the `BSoundPlayer`'s volume control. Pass pointers to variables to be filled (`NULL` is not permitted), and on return these values will be set to describe the player as follows:

Parameter| Description  
---|---  
`outNode`| Is the node to which the `BSoundPlayer`'s audio output will be directed.  
`outParameter`| Is the parameter ID for the volume control.  
`outMinDB`| Is the minimum volume in decibels.  
`outMaxDB`| Is the maximum volume in decibels.  
  
####  HasData(), SetHasData()

`bool HasData();``void SetHasData(bool hasData);`

`HasData()` returns `true` if there's sound queued for playback, or `false` otherwise.

`SetHasData()` specifies whether or not there's sound scheduled for playback.

The purpose of these functions is to optimize the `BSoundPlayer`; if there's no data queued for playback, the sound player node is told this, which lets it optimize its performance. If you're using a buffer player function, you must use `SetHasData()` to indicate that there's data to play:
    
    
    SetHasData(true);

#### InitCheck()

`status_t InitCheck();`

Returns the status code resulting from constructing the `BSoundPlayer` object. You should call this after constructing the object; if the returned value is anything other than `B_OK`, you shouldn't use the object.

#### Latency()

`bigtime_t Latency();`

Returns the `BSoundPlayer`'s latency.

#### SetCallbacks()

`void SetCallbacks(void  (*PlayBuffer) (void* , void* buffer, size_t size, const media_raw_audio_format& format) = NULL,  
void  (*Notifier) (void* , sound_player_notification what) = NULL,  
void* cookie = NULL);`

Sets the play buffer handler function, the event notification handler function, and the cookie all in one atomic operation.

#### SetInitError()

##### protected

`void SetInitError(status_t inError);`

Sets the status code that will be returned by [`InitCheck()`](BSoundPlayer.md#BSoundPlayer_InitCheck "InitCheck\(\)").

#### SetSoundVolume()

`status_t SetSoundVolume(play_id sound,  
float volume);`

Sets the volume (from 0.0 to 1.0) of the specified sound.

####  Start(), Stop()

`status_t Start();``void Stop(bool block = true,  
bool flush = true);`

`Start()` activates the `BSoundPlayer` by starting the time source and the sound player node. The `B_STARTED` notification is sent to the `BSoundPlayer`'s notification handler.

`Stop()` deactivates the `BSoundPlayer` by stopping the player node (if `block` is `true`, the `Stop()` function blocks until the node is stopped). If `flush` is `true`, the queued sounds are all deleted from memory.

While the `BSoundPlayer` is running, the play buffer function (if you've specified one) will be called for each buffer that passes through the `BSoundPlayer`'s playback node. This hook function can be used to implement code that performs more advanced playback of sound, such as sound that's generated on-the-fly, or is filtered before playback.

Return Code| Description  
---|---  
`B_OK`| The sound playback was started.  
Other errors.| As returned by the `BMediaRoster`'s [`StartNode()`](BMediaRoster.md#BMediaRoster_StartNode "StartNode\(\), StopNode\(\)") function.  
  
####  StartPlaying(), StopPlaying(), WaitForSound(), IsPlaying()

##### Deprecated

`play_id StartPlaying(BSound* sound,  
bigtime_t atTime = 0);` `play_id StartPlaying(BSound* sound,  
bigtime_t atTime,  
float withVolume);`

`status_t StopPlaying(play_id id);``status_t WaitForSound(play_id id);``bool IsPlaying(play_id id);`

`StartPlaying()` schedules the specified `BSound` to begin playback at the performance time specified by `atTime`; if `atTime` is 0, the sound begins playing immediately (or as soon as `Start()` is called, if the `BSoundPlayer` hasn't been started yet). The play_id returned by this function is used to identify the sound later. If it's negative, an error occurred (see the list below for possible values). The second form of `StartPlaying()` lets you specify a volume at which the sound should play.

`StopPlaying()` stops playing the sound specified by the given `id`.

`WaitForSound()` waits until the specified sound stops playing.

`IsPlaying()` returns `true` if the specified sound is playing; otherwise, it returns `false`.

`B_OK`| No error.  
---|---  
`B_MEDIA_BAD_FORMAT`| The audio isn't in a supported format (`StartPlaying()`).  
`B_BAD_VALUE`| The specified id doesn't exist (`WaitForSound()` and `IsPlaying()`).  
  
####  Volume(), SetVolume()

`float Volume();``void SetVolume(float newVolume);`

`Volume()` returns the current playback volume.

`SetVolume()` changes the playback volume.

This volume is the overall volume of all the sounds being played by the `BSoundPlayer`. To control the volumes of individual sounds, use the SetSoundVolume() function.

Note

![Note](./images/admonitions/Info_32.png)

The volume can range from 0.0 to 1.0, where 0.0 is silent and 1.0 is maximum loudness.

If you'd rather handle the volume using decibels instead of the percentage range, you can use the [`VolumeDB()`](BSoundPlayer.md#BSoundPlayer_VolumeDB "VolumeDB\(\), SetVolumeDB\(\)") and [`SetVolumeDB()`](BSoundPlayer.md#BSoundPlayer_SetVolumeDB) functions.

####  VolumeDB(), SetVolumeDB()

`float VolumeDB();``void SetVolumeDB(float newVolume);`

`VolumeDB()` returns the current playback volume in decibels.

`SetVolumeDB()` sets the playback volume, in decibels.

This volume is the overall volume of all the sounds being played by the `BSoundPlayer`. To control the volumes of individual sounds, use the [`SetSoundVolume()`](BSoundPlayer.md#BSoundPlayer_SetSoundVolume "SetSoundVolume\(\)") function.

Note

![Note](./images/admonitions/Info_32.png)

The possible range of volumes can be obtained by calling [`GetVolumeInfo()`](BSoundPlayer.md#BSoundPlayer_GetVolumeInfo "GetVolumeInfo\(\)").

If you'd rather handle the volume using a percentage range instead of decibels, you can use the [`Volume()`](BSoundPlayer.md#BSoundPlayer_Volume "Volume\(\), SetVolume\(\)") and [`SetVolume()`](BSoundPlayer.md#BSoundPlayer_SetVolume) functions.

* * *

### Constants

#### sound_player_notification

Constant| Description  
---|---  
`B_STARTED`| The `BSoundPlayer` has been started via the Start() function.  
`B_STOPPED`| The `BSoundPlayer`'s Stop() function has been called.  
`B_SOUND_DONE`| A sound has finished playing.  
  
These constants are passed to event notification handler functions to indicate what sort of interesting event has occurred.

* * *

### Defined Types

#### play_id
    
    
    typedef int32 play_id;

Identifies a particular sound that's being played by the `BSoundPlayer`; [`StartPlaying()`](BSoundPlayer.md#BSoundPlayer_StartPlaying "StartPlaying\(\), StopPlaying\(\), WaitForSound\(\), IsPlaying\(\)") returns values of this type.

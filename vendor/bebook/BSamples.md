<!-- Classes And Methods - The Midi Kit - BSamples -->

## BSamples

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `midi/Samples.h`  
Library:| `libmidi.so`  
Allocation:| –  
[Class Overview](BSamples_Overview.md "BSamples")

  * [Constructor and Destructor](BSamples.md#BSamples_ConstructorDestructor)
  * [Member Functions](BSamples.md#BSamples_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BSamples()

`BSamples();`

Creates a new, empty `BSamples` object. Also constructs a [`BSynth`](BSynth.md "BSynth") object and assigns the object to the app-wide `be_synth` variable (if the object doesn't already exist). To initialize and use the `BSamples` object, invoke its [`Start()`](BSamples.md#BSamples_Start "Start\(\), sample_loop_hook, sample_exit_hook") function.

Note

![Note](./images/admonitions/Info_32.png)

Currently, the "samples only" synth mode (`B_SAMPLES_ONLY`) is broken. Unfortunately, this is the mode used by the `BSamples` constructor if it has to create a `be_synth` object. The easiest way around this bug is to construct a [`BMidiSynth`](BMidiSynth.md "BMidiSynth") or [`BMidiSynthFile`](BMidiSynthFile.md "BMidiSynthFile") object. If you don't need the object, you can immediately destroy it; the fix is effected by the object's construction.

#### ~BSamples()

`virtual ~BSamples();`

Stops the object's playback, calls the "exit hook" function (as set by [`Start()`](BSamples.md#BSamples_Start "Start\(\), sample_loop_hook, sample_exit_hook")), and destroys the object.

* * *

### Member Functions

#### EnableReverb()

`void EnableReverb(bool reverb);`

If `reverb` is `true`, the object's sound data is sent through the synthesizer's reverberator. To set the reverberation depth (for all reverberated sound), use [`BSynth::SetReverb()`](BSynth.md#BSynth_SetReverb).

####  SetSamplingRate(), SamplingRate(), SetPlacement(), Placement(), SetVolume(), Volume()

`void SetSamplingRate(double samplingRate);``double SamplingRate() const;``void SetPlacement(double stereoPan);``double Placement() const;``void SetVolume(double volume);``double Volume() const;`

These functions set parameters of the sound data. They can be called while the sound is playing. For details of what these parameters mean, and the values that they accept, see the descriptions of the `samplingRate`, `stereoPan`, and `volume` arguments of the [`Start()`](BSamples.md#BSamples_Start "Start\(\), sample_loop_hook, sample_exit_hook") function.

####  Start(), sample_loop_hook, sample_exit_hook

`void Start(void* samples,  
int32 frameCount,  
int16 sampleSize,  
int16 channelCount,  
double samplingRate,  
int32 loopStart,  
int32 loopEnd,  
double volume,  
double stereoPan,  
int32 hookArg,  
sample_loop_hook loopHook,  
sample_exit_hook exitHook);`
    
    
    typedef bool (*sample_loop_hook)(int32 arg)
    
    
    typedef void (*sample_exit_hook)(int32 arg)

`Start()` passes in a buffer of audio samples that are immediately played. The playback is performed by the synthesizer in its `BSubscriber` thread; the `Start()` function itself returns immediately.

The audio data is assumed to be little-endian linear; other parameters of the data (sample size, channel count, etc.) are variable and are declared in the function's arguments. The arguments are:

Parameter| Description  
---|---  
`samples`| Is a pointer to the data itself.  
`frameCount`| Is the number of frames of audio data in the buffer.  
`sampleSize`| Is the size of a single sample, in bytes. Currently, the synthesizer only accepts 1- and 2-byte samples.  
`channelCount`| Is the number of channels of data (1 or 2).  
`samplingRate`| Is the rate at which you want to the data played back, expressed as frames-per-second; for example, if your sound data was sampled at 44.1kHz and you want to play it back at its "native" pitch, you would set samplingRate to 44100.0. The range of valid sampling rate values is [0, ~65 kHz]. The sampling rate is independent of the data's channel count and sample size, and is independent of `be_synth`'s sampling rate. You can change the object's sampling rate on the fly through [`SetSamplingRate()`](BSamples.md#BSamples_SetSamplingRate "SetSamplingRate\(\), SamplingRate\(\), SetPlacement\(\), Placement\(\), SetVolume\(\), Volume\(\)").  
`loopStart` and `loopEnd`| Specify the first and last frames that are in the "loop section." The loop section can be any valid section of frames within the sound data (i.e. [0, `frameCount` \- 1] inclusive). Everything up to the beginning of the loop section is the "attack section"; everything after the loop section is the "release section." When the sound is played, the attack section is heard, then the loop section is repeated until the object is told to [`Stop()`](BSamples.md#BSamples_Stop "Stop\(\), Pause\(\), Resume\(\), IsPaused\(\), IsPlaying\(\)"), or until the "loop hook" function (defined below) returns `false`, at which point the release section is played. If you don't want the sound to loop, set these loop… arguments to 0.Warning![Warning](./images/admonitions/Stop_32.png)Currently, the release section is automatically faded out over a brief period of time. If your release section is designed to do a slow fade (for example) you probably won't hear it.  
`volume`| Is an amplitude scalar for this sound. The volume can also be set through [`SetVolume()`](BSamples.md#BSamples_SetVolume).  
`stereoPan`| Locates the sound stereophonically, where -1.0 is hard left, 0.0 is center, and 1.0 is hard right. Notice that if this is a stereo sound, a `stereoPan` value of (say) -1.0 completely attenuates the right channel—it doesn't "move" the right channel into the left channel.  
`hookArg`| Is an arbitrary value that's passed to the `loopHook` and `exitHook` functions.  
`loopHook`| Is a hook function that's called each time the loop section is about to repeat. If the function returns `true`, the loop is, indeed, repeated. If it returns `false`, the release section is played and the sound stops. If you don't supply a `loopHook`, the loop is automatically repeated.  
`exitHook`| Is called when the sound is all done playing, regardless of how it stopped (whether through [`Stop()`](BSamples.md#BSamples_Stop "Stop\(\), Pause\(\), Resume\(\), IsPaused\(\), IsPlaying\(\)"), a `hookFunc` return of `false`, or because the `BSamples` object was deleted).  
  
####  Stop(), Pause(), Resume(), IsPaused(), IsPlaying()

`virtual void Stop();``void Pause();``void Resume();``bool IsPlaying() const;``bool IsPaused() const;`

`Stop()` tells the object to stop playing its sound data, and invokes the exit function, as set through [`Start()`](BSamples.md#BSamples_Start "Start\(\), sample_loop_hook, sample_exit_hook").

`Pause()` suspends playback and `Resume()` resumes it from where it was paused.

Warning

![Warning](./images/admonitions/Stop_32.png)

`Pause()` and `Resume()` are backwards. To pause a sound, call `Resume()`. To resume it, call `Pause()`. Sorry about that.

`IsPlaying()` returns `true` if the object is currently playing (paused or not).

`IsPaused()` returns `true` if the sound is currently paused.

Warning

![Warning](./images/admonitions/Stop_32.png)

`IsPaused()` returns `true` if you've called `Pause()`. Thus, since `Pause()` and `Resume()` are backwards, the sense of `IsPaused()` is also backwards.

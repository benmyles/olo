<!-- Classes And Methods - The Midi Kit - BMidiSynthFile -->

## BMidiSynthFile

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `midi/MidiSynthFile.h`  
Library:| `libmidi.so`  
Allocation:| –  
[Class Overview](BMidiSynthFile_Overview.md "BMidiSynthFile")

  * [Constructor and Destructor](BMidiSynthFile.md#BMidiSynthFile_ConstructorDestructor)
  * [Member Functions](BMidiSynthFile.md#BMidiSynthFile_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BMidiSynthFile()

`BMidiSynthFile();`

Creates a new, empty `BMidiSynthFile` object. Also constructs a [`BSynth`](BSynth.md "BSynth") object and assigns the object to the app-wide `be_synth` variable (if the object doesn't already exist). To load a MIDI file into the `BMidiSynthFile` object, you must call [`LoadFile()`](BMidiSynthFile.md#BMidiSynthFile_LoadFile "LoadFile\(\), UnloadFile\(\)") after construction. Unlike plain [`BMidiSynth`](BMidiSynth.md "BMidiSynth") instances, however, you don't have to call [`EnableInput()`](BMidiSynth.md#BMidiSynth_EnableInput "EnableInput\(\), IsInputEnabled\(\)") ([`LoadFile()`](BMidiSynthFile.md#BMidiSynthFile_LoadFile "LoadFile\(\), UnloadFile\(\)") calls it for you).

#### ~BMidiSynthFile()

`virtual ~BMidiSynthFile();`

Disconnects the object from the synthesizer, unloads the object's file (and all its instruments) and destroys the object.

* * *

### Member Functions

####  Duration(), Position(), Seek()

`int32 Duration() const;``status_t Position(int32 ticks);``int32 Seek();`

`Duration()` returns the length of the object's loaded data, measured in 64ths of a MIDI tick.

`Position()` sets the object's current position within the data.

`Seek()` returns the object's current position.

If you want to reposition the "song pointer", you should do it as a percentage of the `Duration()` measurement.

Note

![Note](./images/admonitions/Info_32.png)

A MIDI tick is defined by the "MIDI Division" field at the bginning of every MIDI file; this value can be specified by the file as either "ppgn" (tempo-dependent) or frames:subframes per second (tempo-independent). The `BMidiSynthFile` class assumes that the division is tempo-independent.

RETURN CODES

Currently `Position()` always returns `B_OK`.

#### GetPatches()

`status_t GetPatches(int16* instruments,  
int16* count);`

Returns an array of the instruments numbers that are needed by the loaded MIDI file. The `instruments` array should be 128 elements long (to be on the safe side), and must be allocated before it's passed in. Upon return, the function sets `count` to the number of instrument numbers that it placed in the array. For example:
    
    
    int16 insts[128];
    int16 count;
    midiSynthFile.GetPatches(insts, &count);
    for (int n = 0; n < count; n++)
       printf("The file uses instrument #%d\n", insts[n]);

####  LoadFile(), UnloadFile()

`status_t LoadFile(const entry_ref* midiFileRef);``void UnloadFile() const;`

`LoadFile()` tells the object to load, into the synthesizer, the MIDI data from the `midiFileRef`. The synthesizer caches the data for subsequent playback (which you initiate through Start()). All instruments that are needed to play the file are loaded into the synthesizer, if they're aren't loaded already.

`UnloadFile()` stops playback of the data that was loaded through this object (if it's playing), and flushes the data (removes it from the synthesizer).

Note

![Note](./images/admonitions/Info_32.png)

In certain circumstances, a single `BMidiSynthFile` object can load and play more than one MIDI file at the same time, but you shouldn't rely on this feature.

Return Code| Description  
---|---  
`B_OK`| The file was found and loaded.  
`B_BAD_MIDI_DATA`| `midiFileRef` isn't a MIDI file, or contains corrupt data.  
`B_NO_MEMORY`| Not enough memory to load the file.  
Other POSIX errors.| The file couldn't be opened or read.  
  
####  MuteTrack(), GetMuteMap(), SoloTrack(), GetSoloMap()

Warning

![Warning](./images/admonitions/Stop_32.png)

These functions are broken; don't use them.

####  Start(), Stop(), Fade(), Pause(), Resume(), IsFinished(), EnableLooping(), SetFileHook(), synth_file_hook

`virtual status_t Start();``virtual void Stop();``void Fade();``void Pause();``void Resume();``bool IsFinished() const;``void EnableLooping(bool loop);``void SetFileHook(synth_file_hook fileDone,  
int32 arg);`
    
    
    typedef void (*synth_file_hook)(int32 arg)

These functions control the object's performance.

`Start()` tells the synthesizer to start playing the object's loaded MIDI data beginning at the beginning. Note that `Start()` does not halt a performance in progress; in other words, if the object is already playing its data, you'll get a second, performance while the first continues.

`Stop()` immediately halts the currently playing data. `Fade()` also stops playback, but is a bit more graceful: It fades out the sound before killing it.

Warning

![Warning](./images/admonitions/Stop_32.png)

`BMidiSynthFile`'s `Start()` and `Stop()` replace the functions defined by `BMidi`; in particular, they spawn and control a "run" thread. See "[The Run Thread and Function](BMidiSynthFile_Overview.md#BMidiSynthFile_Overview_TheRunThreadAndFunction "The Run Thread and Function")" for more information.

`Pause()` and `Resume()` do as they say. Note that when you resume playback, "old" notes are not regenerated, but exact timing is respected. For example, lets say you have a MIDI file that contains two notes, one that starts at time 1.0 (seconds) and lasts for 10.0 seconds, and the other starts at time 9.0. You start the file, then `Pause()` at time 3.0; as expected, the first note stops. After awhile, you `Resume()`; there's a six second silence and then the second note plays.

`IsFinished()` returns `false` if the object is currently playing (or paused during play). Note that the function returns `false` before you load a file, and returns `true` after you've loaded a file but before you begin playing it.

`EnableLooping`(`true`) tells the object to replay the file when it reaches the end of the data. If the argument is `false`, the file isn't replayed. `Stop()` always shuts up playback, even if looping is enabled.

`SetFileHook()` registers a function that's called when the object is finished playing (either because it ran out of data or `Stop()` was called. arg is passed to the hook function as its sole argument. Note that the hook function is called when the object is completely finished—it isn't called at the end of each pass through the data while looping is enabled.

`Start()` returns…

Return Code| Description  
---|---  
`B_OK`| Success.  
`B_TOO_MANY_SONGS_PLAYING`| The synthesizer is too busy with other data streams.  
  
####  Tempo(), SetTempo(), ScaleTempoBy()

`int32 Tempo() const;``void SetTempo(int32 beatsPerMinute);``void ScaleTempoBy(double scalar);`

`Tempo()` returns the tempo of the data, as read from the MIDI file (or as set by the other functions), in beats-per-minute. The other two functions set the tempo: `SetTempo()` sets it absolutely in beats-per-minute, and `ScaleTempoBy()` scales the current tempo by the argument (scalar == 2.0 means the data is played twice as fast, scalar == .5 is twice as slow, and so on).

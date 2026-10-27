<!-- Classes And Methods - The Midi Kit - BMidiSynth -->

## BMidiSynth

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `midi/MidiSynth.h`  
Library:| `libmidi.so`  
Allocation:| –  
[Class Overview](BMidiSynth_Overview.md "BMidiSynth")

  * [Constructor and Destructor](BMidiSynth.md#BMidiSynth_ConstructorDestructor)
  * [Member Functions](BMidiSynth.md#BMidiSynth_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BMidiSynth()

`BMidiSynth();`

Creates a new `BMidiSynth` object. Also constructs a [`BSynth`](BSynth.md "BSynth") object and assigns the object to the app-wide `be_synth` variable (if the object doesn't already exist). To turn on your `BMidiSynth` object, you must call [`EnableInput()`](BMidiSynth.md#BMidiSynth_EnableInput "EnableInput\(\), IsInputEnabled\(\)") after construction.

#### ~BMidiSynth()

`virtual ~BMidiSynth();`

Disconnects the object from the synthesizer, and destroys the object.

* * *

### Member Functions

####  EnableInput(), IsInputEnabled()

`status_t EnableInput(bool enable,  
bool loadSynthFile);``bool IsInputEnabled() const;`

`EnableInput()` tells the object to connect and disconnect itself to the synthesizer, as `enable` is `true` or `false`. A disabled `BMidiSynth` doesn't pass on any MIDI messages to the synthesizer.

If `enable` is `true` and `loadSynthFile` is `true`, any instruments in the synth file (as designated by `be_synth`) that haven't already been loaded are immediately loaded. Loading the entire synth file can take a long time (more than a second, less than a minute), and this isn't your last shot at loading instruments, so you may not want to load all at once. If you want to defer loading, you can call [`LoadInstrument()`](BMidiSynth.md#BMidiSynth_LoadInstrument "LoadInstrument\(\), RemapInstrument\(\), UnloadInstrument\(\), FlushInstrumentCache\(\)") after calling `EnableInput()`.

`IsInputEnabled()` tells you if the `BMidiSynth` is currently enabled.

See "[Initializing Your BMidiSynth](BMidiSynth_Overview.md#BMidiSynth_Overview_InitializingYourBMidiSynth "Initializing Your BMidiSynth")" for more on instrument loading, instrument scope, and the use of Channel 10 percussion.

Note

![Note](./images/admonitions/Info_32.png)

[`BMidiSynthFile`](BMidiSynthFile.md "BMidiSynthFile") objects do not have to call `EnableInput()`. Input is automatically enabled when the object loads a file.

Currently, `EnableInput()` always returns `B_OK`.

####  LoadInstrument(), RemapInstrument(), UnloadInstrument(), FlushInstrumentCache()

`status_t LoadInstrument(int16 instrument);``status_t RemapInstrument(int16 from,  
int16 to);``status_t UnloadInstrument(int16 instrument);``void FlushInstrumentCache(bool flush);`

Warning

![Warning](./images/admonitions/Stop_32.png)

These functions affect _all_ `be_synth` clients (they should actually be part of the [`BSynth`](BSynth.md "BSynth") class).

`LoadInstrument()` loads the designated instrument from the synth file, if it isn't loaded already. You have to load an instrument before you can use it; if you've already loaded all instruments (through [`IsInputEnabled()`](BMidiSynth.md#BMidiSynth_IsInputEnabled)), you won't have to call this function. A loaded instrument is available to all `BMidiSynth` objects in your application.

`RemapInstrument()` tells the synthesizer to replace all `from` instrument requests with `to` requests. For example, here we tell the synthesizer to play a gunshot whenever a banjo note is requested:
    
    
    /* Banjo becomes gunshot. A good idea. */
    midiSynth.RemapInstrument(B_BANJO, B_GUNSHOT);

`UnloadInstrument()` removes the designated instrument from the synthesizer.

`FlushInstrumentCache()` has a goofy protocol. If `flush` is `false`, the synthesizer stops playing all current notes (and samples) and unloads all loaded instruments. If `flush` is `true`, the synthesizer's current output is unaffected, but it's prevented from unloading any instruments in the future. In other words, `FlushInstrumentCache`(`true`) tells the synthesizer to ignore all subsequent `UnloadInstrument()` calls. The inability to unload instruments holds until `FlushInstrumentCache`(`false`) is called.

Note

![Note](./images/admonitions/Info_32.png)

See "[Initializing Your BMidiSynth](BMidiSynth_Overview.md#BMidiSynth_Overview_InitializingYourBMidiSynth "Initializing Your BMidiSynth")" for more on instrument loading, instrument scope, and the use of Channel 10 percussion.

Currently, `UnloadInstrument()` always returns `B_OK`.

The other two status_t functions return…

Return Code| Description  
---|---  
`B_OK`| Instrument successfully loaded or remapped.  
`B_BAD_INSTRUMENT`| Illegal instrument specification.  
`B_NO_MEMORY`| Not enough memory to load instrument.  
  
####  MuteChannel(), GetMuteMap(), SoloChannel(), GetSoloMap()

Warning

![Warning](./images/admonitions/Stop_32.png)

These functions are broken; don't use them.

####  SetTransposition(), Transposition()

`void SetTransposition(int16 transpose);``int16 SetTransposition() const;`

`SetTransposition()` sets the amount, in halfsteps, by which subsequently generated notes will be transposed (shifted in pitch). Notes that are already sounding are not transposed. The transposition affects all channels of the invoked-upon object.

`Transposition()` returns the current transposition amount (in halfsteps); the default is 0.

####  SetVolume(), Volume()

`void SetVolume(double volume);``double Volume() const;`

`SetVolume()` scales the object's amplitude (on all channels) by volume. In addition to affecting subsequently-generated notes, the new volume scale affects notes that are currently being generated by this object.

`Volume()` returns the current amplitude scalar value; the default is 1.0.

#### Tick()

`uint32 Tick() const;`

Returns the number of microseconds since the object was created. You should use this measurement for relative values only (for measuring the time between notes, for example). Also, note that the time argument to the MIDI hook functions measures time in milliseconds.

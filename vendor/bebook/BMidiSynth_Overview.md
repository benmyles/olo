<!-- System Overview - The Midi Kit - BMidiSynth -->

## BMidiSynth

The [`BMidiSynth`](BMidiSynth.md "BMidiSynth") class is the MIDI interface to the General MIDI synthesizer. If you want to send MIDI data to the synthesizer, you have to create an instance of [`BMidiSynth`](BMidiSynth.md "BMidiSynth"). You can can send MIDI messages to the object directly:
    
    
    /* Create and initialize a BMidiSynth. */
    BMidiSynth midiSynth;
    midiSynth.EnableInput(true, true);
    
    /* Choose an instrument and play a note. */
    midiSynth.ProgramChange(1, B_ACOUSTIC_GRAND);
    midiSynth.NoteOn(1, 40, 100, B_NOW);
    snooze(1000000);
    midiSynth.NoteOff(1, 40, 100, B_NOW);

Or you can connect the [`BMidiSynth`](BMidiSynth.md "BMidiSynth") to the output of some other [`BMidi`](BMidi.md "BMidi") object, such as a [`BMidiPort`](BMidiPort.md "BMidiPort"):
    
    
    /* Connect the synth to a MIDI port. */
    BMidiPort midiPort;
    char buf[B_OS_NAME_LENGTH];
    
    /* Initialize the BMidiPort. */
    if (midiPort.GetDeviceName(0, buf) == B_OK) {
       midiPort.Open(buf);
    
       /* midiSynth from above -- already created and initialized. */
       midiPort.Connect(midiSynth);
       midiPort.Start();
       ...
    }

The one thing you shouldn't do is connect a [`BFile`](BFile.md "BFile") to a [`BMidiSynth`](BMidiSynth.md "BMidiSynth"). If you want to realize the contents of a MIDI file, you should use an instance of [`BMidiSynthFile`](BMidiSynthFile.md "BMidiSynthFile") instead ([`BMidiSynthFile`](BMidiSynthFile.md "BMidiSynthFile") is derived from [`BMidiSynth`](BMidiSynth.md "BMidiSynth")).

[`BMidiSynth`](BMidiSynth.md "BMidiSynth") doesn't spray MIDI messages, so it doesn't do any good to connect other [`BMidi`](BMidi.md "BMidi") objects to its output. In other words, don't do this:
    
    
    /* --- DON'T DO THIS --- It's meaningless. */
    midiSynth.Connect(someOtherMidiObject);

* * *

### Initializing Your BMidiSynth

When you create a [`BMidiSynth`](BMidiSynth.md "BMidiSynth") object, it creates an instance of [`BSynth`](BSynth.md "BSynth") for you (if the object doesn't already exist). The [`BSynth`](BSynth.md "BSynth") object, which its represented globally in your application as `be_synth`, provides control over some of the synthesizer's global parameters, such as volume and reverberation.

Before you send messages to your [`BMidiSynth`](BMidiSynth.md "BMidiSynth"), you have to call [`EnableInput()`](BMidiSynth.md#BMidiSynth_EnableInput "EnableInput\(\), IsInputEnabled\(\)"). The function enables the object's input and tells the synthesizer whether it should load the synth file. If you tell [`EnableInput()`](BMidiSynth.md#BMidiSynth_EnableInput "EnableInput\(\), IsInputEnabled\(\)") not to load the file, you'll have to load the instruments that you want yourself. For example, here we load a single instrument, then play a note. We also have to send a [`ProgramChange()`](BMidi.md#BMidi_ProgramChange "ProgramChange\(\), SprayProgramChange\(\)") message to tell the [`BMidiSynth`](BMidiSynth.md "BMidiSynth") object use our loaded instrument on the proper channel (i.e. the channel that we're playing the note on):
    
    
    /* Enable input, but don't load the synth file. */
    midiSynth.EnableInput(true, false);
    
    /* Load a couple of instruments. */
    midiSynth.LoadInstrument(B_TINKLE_BELL);
    midiSynth.LoadInstrument(B_VIOLIN);
    
    /* Associate the instrument with a MIDI channel. */
    midiSynth.ProgramChange(1, B_TINKLE_BELL);
    midiSynth.ProgramChange(2, B_VIOLIN);
    
    /* Play. */
    midiSynth.NoteOn(1, 84, 100);
    snooze(10);
    midiSynth.NoteOn(2, 60, 100);
    snooze(1000000);
    midiSynth.NoteOff(1, 84, 100);
    snooze(10);
    midiSynth.NoteOff(2, 60, 100);

The order and number of instruments follow the General MIDI specification, but begin with instrument 0 (some synthesizers and sequencers expect instrument numbers to start at 1). The handy [midi_axe](TheMidiKit_GeneralMIDIInstruments.md#Enums_midi_axe) constants, defined in `midi/MidiDefs.h`, provide the descriptive instrument names used here.

#### Instrument Scope

All of the instrument loading functions ([`EnableInput()`](BMidiSynth.md#BMidiSynth_EnableInput "EnableInput\(\), IsInputEnabled\(\)") and the functions described under [`LoadInstrument()`](BMidiSynth.md#BMidiSynth_LoadInstrument "LoadInstrument\(\), RemapInstrument\(\), UnloadInstrument\(\), FlushInstrumentCache\(\)")) affect all `be_synth` clients (i.e. all [`BMidiSynth`](BMidiSynth.md "BMidiSynth") objects in your application). This can be beneficial: Loading an instrument from one [`BMidiSynth`](BMidiSynth.md "BMidiSynth") object means it doesn't have to be loaded again when, for example, a [`BMidiSynthFile`](BMidiSynthFile.md "BMidiSynthFile") needs it (the synthesizer is smart about reloading: If an instrument is already loaded, it won't try to load it again). As another example, if you have more than one [`BMidiSynth`](BMidiSynth.md "BMidiSynth") object, they can each load all instruments (through [`EnableInput()`](BMidiSynth.md#BMidiSynth_EnableInput "EnableInput\(\), IsInputEnabled\(\)")) without suffering a performance penalty.

However, there's a dark side: If a [`BMidiSynth`](BMidiSynth.md "BMidiSynth") unloads an instrument (through [`UnloadInstrument()`](BMidiSynth.md#BMidiSynth_UnloadInstrument) or `FlushInstrumentCache`(`false`)), that instrument disappears for all other [`BMidiSynth`](BMidiSynth.md "BMidiSynth") objects as well. You can prevent unwanted instrument unloading by calling `FlushInstrumentCache`(`true`) (see the function description for more information).

#### Percussion Instruments

To use the MIDI Channel 10 percussion instruments, you must load all instruments:
    
    
    /* I want percussion, therefore... */
    midiSynth.EnableInput(true, true);

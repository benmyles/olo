<!-- System Overview - The Midi Kit - BMidiSynthFile -->

## BMidiSynthFile

The [`BMidiSynthFile`](BMidiSynthFile.md "BMidiSynthFile") class reads a standard MIDI file and plays it on the General MIDI synthesizer. Each [`BMidiSynthFile`](BMidiSynthFile.md "BMidiSynthFile") object can read (and play) only one file at a time. To use a [`BMidiSynthFile`](BMidiSynthFile.md "BMidiSynthFile"), you create the object, load a MIDI file, and tell the object to [`Start()`](BMidiSynthFile.md#BMidiSynthFile_Start "Start\(\), Stop\(\), Fade\(\), Pause\(\), Resume\(\), IsFinished\(\), EnableLooping\(\), SetFileHook\(\), synth_file_hook"):
    
    
    /* Create and initialize a BMidiSynthFile. */
    BMidiSynthFile midiSynthFile;
    entry_ref midiRef;
    get_ref_for_path("/boot/optional/midi/QuickBrownFox.mid", &midiRef);
    midiSynthFile.LoadFile(&midiRef);
    
    /* Play the file. */
    midiSynthFile.Start();

You should create a different [`BMidiSynthFile`](BMidiSynthFile.md "BMidiSynthFile") object for each MIDI file that you want to mix together into a single performance.

Warning

![Warning](./images/admonitions/Stop_32.png)

In certain circumstances, a single [`BMidiSynthFile`](BMidiSynthFile.md "BMidiSynthFile") object can load and play more than one MIDI file at the same time, but you shouldn't rely on this feature.

* * *

### Loading and Playing

When you call [`LoadFile()`](BMidiSynthFile.md#BMidiSynthFile_LoadFile "LoadFile\(\), UnloadFile\(\)"), the [`BMidiSynthFile`](BMidiSynthFile.md "BMidiSynthFile") object automatically calls…
    
    
    EnableInput(true, false)

It then loads the file's MIDI data into the synthesizer, which loads all the instruments that are needed by the file. If the file uses a lot of different instruments, loading the file can take some time.

When the file is finished playing (either because it's reached the end, or because you called [`Stop()`](BMidiSynthFile.md#BMidiSynthFile_Stop)) the instruments are not unloaded. This cuts the overhead if you play the file a second time.

* * *

### The Run Thread and Function

[`BMidiSynthFile`](BMidiSynthFile.md "BMidiSynthFile") is different from other [`Start()`](BMidiSynthFile.md#BMidiSynthFile_Start "Start\(\), Stop\(\), Fade\(\), Pause\(\), Resume\(\), IsFinished\(\), EnableLooping\(\), SetFileHook\(\), synth_file_hook")-able [`BMidi`](BMidi.md "BMidi") objects in that it doesn't have a run loop. The MIDI data is parsed and realized in the synthesizer's subscriber thread (the thread that dumps data into the DAC stream). The lack of a run loop shouldn't affect the way you write your code, but you should be aware that the thread isn't there so you won't go looking for it while you're developing your app

Furthermore, [`BMidiSynthFile`](BMidiSynthFile.md "BMidiSynthFile") doesn't implement the [`Run()`](BMidi.md#BMidi_Run "Run\(\)") function. Starting and stopping the object's performance (activities that are normally handled in the Run() function) are handled by the synthesizer in its subscriber thread. If you create a [`BMidiSynthFile`](BMidiSynthFile.md "BMidiSynthFile") subclass, don't try to resurrect the [`Run()`](BMidi.md#BMidi_Run "Run\(\)") function—leave it as a no-op.

As with the [`BMidiSynth`](BMidiSynth.md "BMidiSynth") class, the [`BMidiSynthFile`](BMidiSynthFile.md "BMidiSynthFile") MIDI hook implementations don't call the spray functions. This means that you can't, for example, connect a [`BMidiSynthFile`](BMidiSynthFile.md "BMidiSynthFile") to a [`BMidiPort`](BMidiPort.md "BMidiPort"). If you want to play a MIDI file out a MIDI port, use [`BMidiStore`](BMidiStore.md "BMidiStore") to represent and play the file.

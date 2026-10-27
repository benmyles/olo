<!-- System Overview - The Midi Kit - BSynth -->

## BSynth  
  
The BeOS includes a 16-channel General MIDI software synthsizer designed by HeadSpace Inc. The [`BSynth`](BSynth.md "BSynth") class is the interface to the synthesizer itself. Any application that wants to use the synthesizer must include a [`BSynth`](BSynth.md "BSynth") object; however, most applications won't need to create the object directly: The [`BMidiSynth`](BMidiSynth.md "BMidiSynth"), [`BMidiSynthFile`](BMidiSynthFile.md "BMidiSynthFile"), and [`BSamples`](BSamples.md "BSamples") classes create a [`BSynth`](BSynth.md "BSynth") object for you. Furthermore, since [`BSynth`](BSynth.md "BSynth") doesn't inherit from [`BMidi`](BMidi.md "BMidi"), it doesn't have any API for actually playing MIDI data. To play MIDI data, you need an instance of [`BMidiSynth`](BMidiSynth.md "BMidiSynth") or [`BMidiSynthFile`](BMidiSynthFile.md "BMidiSynthFile").

An application can have only one [`BSynth`](BSynth.md "BSynth") object at a time. The object is represented globally (within your app) as `be_synth`. The classes that create a [`BSynth`](BSynth.md "BSynth") for you ([`BMidiSynth`](BMidiSynth.md "BMidiSynth") and so on) won't clobber an existing `be_synth`, but the [`BSynth`](BSynth.md "BSynth") constructor will.

When it's created, the `be_synth` object tries to find an instrument definition (or "synth") file. This is a file that contains the data (samples and instructions) for creating General MIDI instruments. The BeOS provides two such files (both designed by HeadSpace, and both stored in `B_SYNTH_DIRECTORY`):

Constant| Description  
---|---  
`B_BIG_SYNTH_FILE`  
(`/boot/beos/etc/synth/big_synth.sy`)| Contains 16-bit, 22 kHz data. It takes about 5 Megs of memory when fully loaded.  
`B_LITTLE_SYNTH_FILE`  
(`/boot/beos/etc/synth/little_synth.sy`)| Is 8-bit, 11 kHz data. It's a quarter the size of the big synth, but lacks the big file's fidelity.  
  
The instrument data is read from the file as it's needed. To "pre-load" the entire synth file, use the [`BMidiSynth::EnableInput()`](BMidiSynth.md#BMidiSynth_EnableInput "EnableInput\(\), IsInputEnabled\(\)") function.

* * *

### BSynth and the Audio Server

The synthesizer produces sound by taking over the Audio Server's DAC stream. It resets the size and number of buffers in the stream, sets the sampling rate, and adds a `BSubscriber` to the front of the stream. If you want to mix sound files into the MIDI synthesis, you should use the [`BSamples`](BSamples.md "BSamples") object rather than add your own DAC stream subscribers. However, if you really want to add your own sample-generating subscribers, don't add them to the front of the DAC stream after the `be_synth` subscriber has been added—your subscriber's samples will be clobbered.

The interaction between the synthesizer and the Media Kit will be cleaned up in a subsequent release.

The DAC stream's previous settings are restored when `be_synth` is destroyed.

* * *

### Synthesis Capacity

The synthesizer can generate up to 32 voices at a time, where a "voice" is either an individual (synthesized) note, or a stream of samples from a [`BSamples`](BSamples.md "BSamples") object. By default, it apportions 28 voice "slots" for synthesis and 4 for samples. You can change the settings through the [`SetVoiceLimits()`](BSynth.md#BSynth_SetVoiceLimits "SetVoiceLimits\(\), MaxSynthVoices\(\), MaxSampleVoices\(\), LimiterThreshhold\(\)") function.

If you ask for more voices than there are voice slots (for example, if you ask for a 29'th note when there are already 28 singing), the synthesizer will try to kill an old note in order to make room for the new note.

There's no guarantee that the synthesizer and DAC stream will have enough time to generate and process everything you ask for, even if you're running below the 32 voice limit. On a lightly loaded, reasonably fast machine, you shouldn't hear any glitches, but a heavy MIDI command stream (for example) could bog it down.

* * *

### Recording a Performance

There's no API for automatically writing the synthesizer's output to a file. To record a synthesizer performance you have to create your own `BSubscriber`, add it to the DAC stream (downstream of the synthesizer), and write out the samples that it receives. (See the Media Kit for more information.)

In some cases, the act of recording can be enough of a CPU drag that the synthesizer falls behind realtime (actually, it's the synthesizer's `BSubscriber` that's getting behind). It may not sound great while you're monitoring the recording, but the data that's written to the file probably won't be affected—the glitches won't be written to the file.

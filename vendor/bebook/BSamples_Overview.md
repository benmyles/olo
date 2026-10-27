<!-- System Overview - The Midi Kit - BSamples -->

## BSamples

The [`BSamples`](BSamples.md "BSamples") class lets you add a stream of audio samples into the General MIDI mix. The main functionality of the class (such as how you define the sound you want to play, whether it should repeat, what speed it should play at, and so on) is embedded in the [`Start()`](BSamples.md#BSamples_Start "Start\(\), sample_loop_hook, sample_exit_hook") function; the class' other functions control and fine-tune the sound as it's playing. To learn how to use a [`BSamples`](BSamples.md "BSamples") object, go to the [`Start()`](BSamples.md#BSamples_Start "Start\(\), sample_loop_hook, sample_exit_hook") function.

Theoretically, you can create and play (at the same time) as many [`BSamples`](BSamples.md "BSamples") as you want. In practice, you are limited to the sample voice limit set through [`BSynth`](BSynth.md "BSynth")'s [`SetVoiceLimits()`](BSynth.md#BSynth_SetVoiceLimits "SetVoiceLimits\(\), MaxSynthVoices\(\), MaxSampleVoices\(\), LimiterThreshhold\(\)") function; by default the limit is set to four.

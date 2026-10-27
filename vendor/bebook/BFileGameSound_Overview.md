<!-- System Overview - The Game Kit - BFileGameSound -->

## BFileGameSound  
  
If you want to play back audio from a disk file, such as background music, or low-priority sound effects, [`BFileGameSound`](BFileGameSound.md "BFileGameSound") is for you.

Keep in mind that if the sound needs to play at precise moments, or latency is an issue, that [`BFileGameSound`](BFileGameSound.md "BFileGameSound") may not be appropriate for your needs.

Using [`BFileGameSound`](BFileGameSound.md "BFileGameSound") is easy, and it supports automatically looping sounds. The following code snippet starts up the theme music for the hot new game "Mystery Warriors From the Doomed Planet Z":
    
    
    BFileGameSound themeSong("music/theme.aif", true);
    themeSong.StartPlaying();

This starts up a looped sound from the file `music/theme.aif`. If you want the theme to only play through once, just specify `false` instead of `true` for the looping argument to the [`BFileGameSound`](BFileGameSound.md "BFileGameSound") constructor.

You can pause the [`BFileGameSound`](BFileGameSound.md "BFileGameSound") by calling [`SetPaused()`](BFileGameSound.md#BFileGameSound_SetPaused "SetPaused\(\), IsPaused\(\)"):
    
    
    themeSong.SetPaused(true, 0);

This pauses the sound, effective immediately. The pause can optionally be ramped, so that the sound slows down or speeds up to reach the new setting. For example:
    
    
    themeSong.SetPaused(true, 2);

This causes the sound to slow down over the course of two seconds, until it's stopped.

The inverse is also possible:
    
    
    themeSong.SetPaused(false, 0);

This resumes playback immediately. You can ramp the resume by specifying a non-zero value for the ramp time.

[`BFileGameSound`](BFileGameSound.md "BFileGameSound") uses the [`BMediaFile`](BMediaFile.md "BMediaFile") class to access the sound file. If [`BMediaFile`](BMediaFile.md "BMediaFile") can't identify the sound, the file is assumed to contain raw 44.1kHz, 16-bit stereo.

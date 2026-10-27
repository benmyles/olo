<!-- System Overview - The Game Kit - BSimpleGameSound -->

## BSimpleGameSound  
  
The [`BSimpleGameSound`](BSimpleGameSound.md "BSimpleGameSound") class represents simple sound effects that don't change, and remain in memory.

Using [`BSimpleGameSound`](BSimpleGameSound.md "BSimpleGameSound") is, well, simple:
    
    
    BSimpleGameSound* mysound = new BSimpleGameSound("soundfile.wav");
    ...
    mysound.StartPlaying();

This snippet uses [`BSimpleGameSound`](BSimpleGameSound.md "BSimpleGameSound") to create an object that can be used to play the sound effect in the file "soundfile.wav". Playing the sound is as simple as calling [`StartPlaying()`](BGameSound.md#BGameSound_StartPlaying "StartPlaying\(\), StopPlaying\(\)").

Note

![Note](./images/admonitions/Info_32.png)

In the current version of the BeOS, when you clone a [`BSimpleGameSound`](BSimpleGameSound.md "BSimpleGameSound"), the sound data buffer is also cloned, so you'll have multiple copies of the sound effect in memory. Keep this in mind as you write your code, as you can rapidly use a lot of memory this way.

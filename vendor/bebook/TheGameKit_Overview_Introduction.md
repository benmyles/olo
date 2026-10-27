<!-- System Overview - The Game Kit - Introduction -->

## Introduction

The Game Kit provides features that game developers will find particularly useful:

There are two types of functionality provided by the Game Kit:

  * Low-level graphics access

  * High-performance and convenient sound playback

There's also one global function, [`set_mouse_position()`](TheGameKit_Functions.md#set_mouse_position "set_mouse_position\(\)"), which lets you move the mouse cursor programatically.

Although designed with games in mind, nothing in the Game Kit is restricted to game applications, except that the user will have to deposit another 50 cents every three minutes.

* * *

### Low-Level Graphics Access

There are two classes provided for direct access to the underlying graphics hardware: [`BWindowScreen`](BWindowScreen.md "BWindowScreen") and [`BDirectWindow`](BDirectWindow.md "BDirectWindow"). These two classes both give you direct access to the graphics card's frame buffer; the difference between them is that [`BWindowScreen`](BWindowScreen.md "BWindowScreen") always takes over the entire screen, bypassing the Application Server, while [`BDirectWindow`](BDirectWindow.md "BDirectWindow") can draw in a window on most graphics hardware.

Although [`BDirectWindow`](BDirectWindow.md "BDirectWindow") can do everything [`BWindowScreen`](BWindowScreen.md "BWindowScreen") can do, [`BWindowScreen`](BWindowScreen.md "BWindowScreen") can be a little easier to use.

* * *

### High-Performance Audio

Several classes are provided for high-performance audio playback:

[`BGameSound`](BGameSound.md "BGameSound")
    

Is a base class and isn't used directly; it's the foundation for all the other audio classes in the Game Kit.

[`BFileGameSound`](BFileGameSound.md "BFileGameSound")
    

Is used to represent a sound effect stored in a disk file, and provides functions for playing the sound. It can be used for playing music loops from audio files, or for playing the occasional large sound effect.

[`BSimpleGameSound`](BSimpleGameSound.md "BSimpleGameSound")
    

On the other hand, preloads the sound into memory, so that it can be played back with extremely low latency and low overhead (at the expense of memory space).

[`BPushGameSound`](BPushGameSound.md "BPushGameSound")s
    

Are used to let you fill buffers flowing to the speakers (or headphones, or whatever audio output device is being used). Their missions are the same, but their methods are different; [`BPushGameSound`](BPushGameSound.md "BPushGameSound") also provides a way to play sound in a cyclic loop, keeping ahead of the playback point in the buffer. This is the extreme in high-performance, low-latency audio, but does require some extra work on the programmer's part.

[`BStreamingGameSound`](BStreamingGameSound.md "BStreamingGameSound")
    

Requires magic powers to use; its functionality is closely tied to the implementation of the Kit, especially when hardware sound acceleration is involved. So don't touch it, we beg you.

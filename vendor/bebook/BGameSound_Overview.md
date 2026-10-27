<!-- System Overview - The Game Kit - BGameSound -->

## BGameSound  
  
The [`BGameSound`](BGameSound.md "BGameSound") class is the base class for other game sound classes. You never use this class directly; instead, you should use the derived classes.

* * *

### The Sound of Explosions in the Morning

Nothing makes the player happy like the sound of lots of explosions, weird alien things, and other thrilling audio pleasures. The various [`BGameSound`](BGameSound.md "BGameSound")-derived classes make this a snap.

#### Choosing a Player Class

Playing a sound is as simple as instantiating an object derived from the most appropriate class and calling [`StartPlaying()`](BGameSound.md#BGameSound_StartPlaying "StartPlaying\(\), StopPlaying\(\)") on it. How do you determine the most appropriate class? Here are some hints:

  * If you have a sound effect that's short and is likely to be played often, or needs to be played with minimal latency, use the [`BSimpleGameSound`](BSimpleGameSound.md "BSimpleGameSound") class. An arcade game's explosions definitely fit into this class.

  * If you have a sound effect that's very long, or doesn't need to be played very often, or whose latency doesn't matter much, use the [`BFileGameSound`](BFileGameSound.md "BFileGameSound") class. Music loops can take advantage of this class.

  * If you want to be able to push audio buffers at the system, instead of having a callback that fills buffers, use the [`BPushGameSound`](BPushGameSound.md "BPushGameSound") class.

Most games' needs will be filled by the [`BSimpleGameSound`](BSimpleGameSound.md "BSimpleGameSound") and [`BFileGameSound`](BFileGameSound.md "BFileGameSound") classes. And, of course, you can write your own derived classes if none of these classes meet your requirements.

#### Polyphony

Sometimes the player needs to hear multiple copies of the same sound at the same time; for instance, if they shoot a three-way splitting weapon and hit two targets, they need to hear two explosions at once. Each [`BGameSound`](BGameSound.md "BGameSound") (or [`BGameSound`](BGameSound.md "BGameSound")-derived) object can play only once at a time, so you'll need to use cloned copies of the sound, one for each channel of polyphony you allow.

This can be done with ease using a simple class like this:
    
    
    class MultipleEffect {
    public:
       MultipleEffect(BGameSound* sound, int polyphony) {
          m_fx = new BGameSound *[polyphony];
          m_fx[0] = sound;
          for (int ix=1; ix<polyphony; ix++) {
             m_fx[ix] = sound->Clone();
          }
          m_current = 0;
          m_polyphony = polyphony;
       }
       void StartPlaying() {
          int id = atomic_add(&m_current, 1) % m_polyphony;
          m_fx[id]->StartPlaying();
       }
    
    private:
       int32 m_current;
       int32 m_polyphony;
       BGameSound* m_fx;
    };

This class' constructor lets you specify a [`BGameSound`](BGameSound.md "BGameSound")-derived sound and the maximum number of times it can be playing at once. It creates the appropriate number of clones, and its [`StartPlaying()`](BGameSound.md#BGameSound_StartPlaying "StartPlaying\(\), StopPlaying\(\)") implementation automatically selects the oldest one and reuses it. Since [`StartPlaying()`](BGameSound.md#BGameSound_StartPlaying "StartPlaying\(\), StopPlaying\(\)") restarts a sound from the beginning if it's called on a playing sound, this works out well—if polyphony is 3, and there are already three sounds playing, the oldest one will be reset and played from the beginning.

Note

![Note](./images/admonitions/Info_32.png)

In the current version of the BeOS, when you clone a [`BSimpleGameSound`](BSimpleGameSound.md "BSimpleGameSound"), the sound data buffer is also cloned, so you'll have multiple copies of the sound effect in memory. Keep this in mind as you write your code, as you can rapidly use a lot of memory this way.

#### Performance Verification

There are a wide variety of performance concerns, and of course you want your game to work on as many systems as possible. Be recommends that you test your application in at least these three situations:

  * Real-time audio disabled, using an ISA sound card (such as an AWE64 or equivalent card).

  * Real-time audio enabled, using a PCI sound card (SoundBlaster PCI 64 or equivalent).

  * Real-time audio enabled, using an E-mu 10k sound card. This sound card has its own node, so its behavior is a little different, and is therefore worth special testing attention.

This isn't intended to be an exhaustive list; you should always test your software in as many configurations as possible.

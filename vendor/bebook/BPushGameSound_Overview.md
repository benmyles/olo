<!-- System Overview - The Game Kit - BPushGameSound -->

## BPushGameSound

The [`BPushGameSound`](BPushGameSound.md "BPushGameSound") class lets you push buffers of audio data, instead of waiting to be asked for them.

* * *

### How It Works

The [`BPushGameSound`](BPushGameSound.md "BPushGameSound") class uses a single sound buffer, consisting of multiple pages, which play continuously in a loop. Each page is used to construct an audio [`BBuffer`](BBuffer.md "BBuffer") that eventually gets played, and is then recycled and reused again later.

For example, if the sound buffer is 256 kilobytes, and each page is 4 kilobytes, there are 64 pages of audio. When you start the [`BPushGameSound`](BPushGameSound.md "BPushGameSound") object, playback begins with the first page. A [`BBuffer`](BBuffer.md "BBuffer") is constructed using that page, then played, and then the buffer is recycled, and the next page is used to create another [`BBuffer`](BBuffer.md "BBuffer"), and so forth. This continues to the 64th page. Once that page is played, playback loops back to the first page again.

Your code pushes audio data into these audio pages. There are two ways you can do this.

#### Exclusive Access

The first way is to ask the [`BPushGameSound`](BPushGameSound.md "BPushGameSound") class to give you a page to fill with audio data. This is done by calling [`LockNextPage()`](BPushGameSound.md#BPushGameSound_LockNextPage "LockNextPage\(\), UnlockPage\(\)"). This gives you exclusive access to the next audio page that needs to be filled; you can fill it with whatever sound you want to push, then call [`UnlockPage()`](BPushGameSound.md#BPushGameSound_UnlockPage) to release it. It won't be played while it's locked, so you need to stuff your sound into it and release it as quickly as possible.

#### The Neverending Story

The second way takes better planning, but can give you lower overhead. Call [`LockForCyclic()`](BPushGameSound.md#BPushGameSound_LockForCyclic "LockForCyclic\(\), UnlockCyclic\(\), CurrentPosition\(\)") to request access to the entire sound buffer area. This doesn't give you exclusive access, so playback never stops—it keeps looping the entire time, while you write into it. The [`CurrentPosition()`](BPushGameSound.md#BPushGameSound_CurrentPosition) function tells you where in the buffer area playback is currently occurring.

Your mission (should you choose to accept it) is to stuff audio into the buffer, keeping ahead of this position far enough that playback never catches up to you. As a general rule, you should try to stay at least a page ahead of the current playback position. Keep in mind that when you reach the end of the buffer area, you need to wrap back to the beginning.

This takes more careful effort on your part, but once you have your code properly tuned, you can get very low overhead audio playback this way.

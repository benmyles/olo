<!-- System Overview - The Application Kit - BMessageQueue -->

## BMessageQueue

The [`BMessageQueue`](BMessageQueue.md "BMessageQueue") class completes the implementation of [`BLooper`](BLooper.md "BLooper") by providing a first-in/first-out stack in which the looper can place in-coming [`BMessage`](BMessage.md "BMessage")s. In general, the message dispatching mechanism of [`BLooper`](BLooper.md "BLooper") should suffice. However, if you ever need to manipulate a [`BMessage`](BMessage.md "BMessage") queue directly, you can do so.

<!-- System Overview - The Media Kit - BSmallBuffer -->

## BSmallBuffer

The [`BSmallBuffer`](BSmallBuffer.md "BSmallBuffer") class provides small, efficient buffers for MIDI and other small packet-based media types.

To use [`BSmallBuffer`](BSmallBuffer.md "BSmallBuffer")s, create a [`BSmallBuffer`](BSmallBuffer.md "BSmallBuffer"), then fill it with up to [`SmallBufferSizeLimit()`](BSmallBuffer.md#BSmallBuffer_SmallBufferSizeLimit "BroadcastTimeWarp\(\)") bytes of data. Then call [`SendBuffer()`](BBufferProducer.md#BBufferProducer_SendBuffer "SendBuffer\(\)"). You can then immediately reuse the buffer.

How is this possible? [`BSmallBuffer`](BSmallBuffer.md "BSmallBuffer")s don't go in buffer groups. The receiver immediately either processes the buffer (if it's time) or caches it until it's needed, then recycles the buffer.

Note

![Note](./images/admonitions/Info_32.png)

If you're writing a node, and receive a buffer with the `B_SMALL_BUFFER` flag set, you must recycle the buffer in your [`BufferReceived()`](BBufferConsumer.md#BBufferConsumer_BufferReceived "BufferReceived\(\)") function.

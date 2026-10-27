<!-- System Overview - The Media Kit - BBuffer -->

## BBuffer

A [`BBuffer`](BBuffer.md "BBuffer") references a chunk of shared memory where media data can be passed between applications and nodes. The control header for these buffers is passed through the use of a safer IPC mechanism, but the actual data these headers represent typically require a high data rate, and for the sake of maximizing performance, shared memory is the method by which the buffers are managed.

[`BBuffer`](BBuffer.md "BBuffer")s originate at some [`BBufferProducer`](BBufferProducer.md "BBufferProducer"), which has a [`BBufferGroup`](BBufferGroup.md "BBufferGroup") that acts as a source of [`BBuffer`](BBuffer.md "BBuffer")s (see [`BBufferConsumer::SetOutputBuffersFor()`](BBufferConsumer.md#BBufferConsumer_SetOutputBuffersFor "SetOutputBuffersFor\(\)")). [`BBuffer`](BBuffer.md "BBuffer")s are sent through all participating nodes, possibly being sent down a long chain—much like a bucket brigade—until finally the node arrives at a node that chooses not to pass along the [`BBuffer`](BBuffer.md "BBuffer") any further, at which the buffer's [`Recycle()`](BBuffer.md#BBuffer_Recycle "Recycle\(\)") function is called to return the buffer to the [`BBufferGroup`](BBufferGroup.md "BBufferGroup")'s store, where it gets reused by another batch of data.

An application or custom node can set up [`BBuffer`](BBuffer.md "BBuffer")s that reference a specific area of memory, such as a low-level driver buffer, a [`BBitmap`](BBitmap.md "BBitmap"), or a [`BDirectWindow`](BDirectWindow.md "BDirectWindow")'s area of a frame buffer. This ability can provide for great optimization by avoiding unnecessary copy operations.

The [`BSmallBuffer`](BSmallBuffer.md "BSmallBuffer") class, derived from [`BBuffer`](BBuffer.md "BBuffer"), is used for very small buffers; they don't go in [`BBufferGroup`](BBufferGroup.md "BBufferGroup")s, and get handled specially to optimize performance.

Warning

![Warning](./images/admonitions/Stop_32.png)

[`BBuffer`](BBuffer.md "BBuffer") should never be subclassed, since they're mostly owned and managed by the Media Server. Even if you create your own [`BBuffer`](BBuffer.md "BBuffer"), once you've called [`BBufferConsumer::SetOutputBuffersFor()`](BBufferConsumer.md#BBufferConsumer_SetOutputBuffersFor "SetOutputBuffersFor\(\)"), the "same" buffer received back by your [`BBufferConsumer`](BBufferConsumer.md "BBufferConsumer") may actually be another instance of the [`BBuffer`](BBuffer.md "BBuffer") class.

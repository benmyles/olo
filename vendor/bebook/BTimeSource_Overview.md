<!-- System Overview - The Media Kit - BTimeSource -->

## BTimeSource  
  
The [`BTimeSource`](BTimeSource.md "BTimeSource") class represents a clock to which nodes can be slaved. By slaving all your nodes to a single master time source, they can be kept in sync with each other.

If a node can, either by design or as a side benefit of the underlying hardware, provide reliable timing services, it might make sense for it to be derived from [`BTimeSource`](BTimeSource.md "BTimeSource") as well as from whatever other classes it might be derived from, such as [`BBufferProducer`](BBufferProducer.md "BBufferProducer") or [`BBufferConsumer`](BBufferConsumer.md "BBufferConsumer"). Doing so will allow other nodes to be slaved to your node's conception of time.

Note that although a [`BTimeSource`](BTimeSource.md "BTimeSource") is implemented as a real object (and is therefore not a purely abstract class), other nodes won't call your [`BTimeSource`](BTimeSource.md "BTimeSource")'s member functions directly—instead, your [`BTimeSource`](BTimeSource.md "BTimeSource") will provide data that other nodes will then read.

There are invisible system implementations of the [`BTimeSource`](BTimeSource.md "BTimeSource") protocol that serve as stand-ins for other nodes, so if you call [`BMediaRoster::SetTimeSourceFor()`](BMediaRoster.md#BMediaRoster_SetTimeSourceFor "SetTimeSourceFor\(\)") to make one of your nodes (which is derived from [`BTimeSource`](BTimeSource.md "BTimeSource")) a time source for some other node, the other node might see a system stand-in object, not the actual [`BTimeSource`](BTimeSource.md "BTimeSource")-derived object.

This abstraction layer serves a valuable purpose: it enforces the desire to prevent any two nodes from having to know anything about each other beyond the Media Kit protocols defined in this chapter; this sort of low-level interdependency is discouraged, because it decreases interoperability.

* * *

### Keeping Time

Although it can be confusing at first, keep in mind that a node derived from both [`BTimeSource`](BTimeSource.md "BTimeSource") and [`BBufferProducer`](BBufferProducer.md "BBufferProducer") (or [`BBufferConsumer`](BBufferConsumer.md "BBufferConsumer"))—which is therefore a time source, as well as a producer or consumer of buffers—has to deal with two different time concepts. As a [`BTimeSource`](BTimeSource.md "BTimeSource"), it needs to understand requests in real time, while as a [`BBufferProducer`](BBufferProducer.md "BBufferProducer") or [`BBufferConsumer`](BBufferConsumer.md "BBufferConsumer"), it needs to accept requests in performance time.

Real time refers to the actual passage of time, as reported by [`system_time()`](TheKernelKit_Time.md#system_time "system_time\(\)") or the [`BTimeSource::RealTime()`](BTimeSource.md#BTimeSource_RealTime "RealTime\(\)") function. It's measured in microseconds.

Performance time runs in "time units" which aren't necessarily directly related to real time. Since your code will have to deal with both kinds of time, you need to be sure to convert between the two time systems when it's necessary to do so. Use the [`BTimeSource::RealTimeFor()`](BTimeSource.md#BTimeSource_RealTimeFor) function to do this.

For example, to calculate a timeout value, given a desired performance time, and an estimated latency on the connection, you might use the following code:
    
    
    bigtime_t timeout = TimeSource()->RealTimeFor(performance_time,
             estimated_latency) - TimeSource()->RealTime();

This code converts the `performance_time` into the driving time source's units, then subtracts the current real time, which results in the desired timeout value.

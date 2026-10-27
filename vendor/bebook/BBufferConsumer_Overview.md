<!-- System Overview - The Media Kit - BBufferConsumer -->

## BBufferConsumer  
  
[`BBufferConsumer`](BBufferConsumer.md "BBufferConsumer") is the counterpart to the [`BBufferProducer`](BBufferProducer.md "BBufferProducer") class—it receives [`BBuffer`](BBuffer.md "BBuffer")s from the [`BBufferProducer`](BBufferProducer.md "BBufferProducer")s that are connected to it, manipulates them in some fashion (either by altering the contents of the buffer or by playing the buffer's data to the speakers or to the screen), and possibly then passes them along to another [`BBufferConsumer`](BBufferConsumer.md "BBufferConsumer") (if the node also inherits from [`BBufferProducer`](BBufferProducer.md "BBufferProducer")).

Note

![Note](./images/admonitions/Info_32.png)

The functions in this class aren't called by applications or by other nodes; they're called exclusively by the Media Kit to control and obtain information about a buffer consumer.

A [`BBufferConsumer`](BBufferConsumer.md "BBufferConsumer") publishes certain inputs, identified by [media_destination](TheMediaKit_DefinedTypes.md#media_destination "media_destination") structures, on which connections may be requested by a client application.

* * *

### Dealing With Multiple Time Sources

Sometimes you'll find that the producer that's sending buffers to your [`BBufferConsumer`](BBufferConsumer.md "BBufferConsumer") is using a different time source than you are. It's relatively easy to cope with this situation—you just have to be aware that it can occur and compensate when it does:
    
    
    bigtime_t time;
    media_node producerNode;
    BTimeSource *producerTimeSource;
    media_node_id producerTSID = buffer->Header()->time_source;
    
    if (producerTSID != myTSID) {
       roster->GetNodeFor(producerTSID, &producerNode);
       producerTimeSource = roster->MakeTimeSourceFor(&producerNode);
       time = buffer->Header()->start_time;
       time = producerTimeSource->RealTimeFor(time, myLatency);
       time = myTimeSource->PerformanceTimeFor(time);
    }
    else {
       time = buffer->Header()->start_time;

After this code has executed, time contains the start time for the buffer, in your node's time base.

You should probably, for performance's sake, cache the producer's time source the first time [`BufferReceived()`](BBufferConsumer.md#BBufferConsumer_BufferReceived "BufferReceived\(\)") is called.

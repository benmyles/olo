<!-- System Overview - The Media Kit - BMediaNode -->

## BMediaNode  
  
The [`BMediaNode`](BMediaNode.md "BMediaNode") class is the superclass of all participant nodes in the Media Kit. However, you'll never derive directly from [`BMediaNode`](BMediaNode.md "BMediaNode"); instead, you'll derive from one of the system interface classes which in turn are derived from [`BMediaNode`](BMediaNode.md "BMediaNode"). Look at the documentation for those other classes (such as [`BBufferProducer`](BBufferProducer.md "BBufferProducer") and [`BBufferConsumer`](BBufferConsumer.md "BBufferConsumer")) for details on how they're used, or see "[Creating New Node Classes](TheMediaKit_Overview_Introduction.md#TheMediaKit_CreatingNewNodeClasses "Creating New Node Classes")" for discussion on how this is done.

Because of the quirks of virtual inheritance (required by the use of multiple inheritance), your node's constructor will have to call the [`BMediaNode`](BMediaNode.md "BMediaNode") constructor. See "[About Multiple Virtual Inheritance](TheMediaKit_Overview_Introduction.md#TheMediaKit_AboutMultipleVirtualInheritance "About Multiple Virtual Inheritance")".

Note

![Note](./images/admonitions/Info_32.png)

Applications shouldn't call a node's member functions directly; instead, you call the [`BMediaRoster`](BMediaRoster.md "BMediaRoster") with a reference to the node and let the request come to the node through the control port. The only exception is if the node is subclassed directly within the application, in which case [`Acquire()`](BMediaNode.md#BMediaNode_Acquire "Acquire\(\), Release\(\)"), [`ID()`](BMediaNode.md#BMediaNode_ID "ID\(\)"), [`Node()`](BMediaNode.md#BMediaNode_Node "Node\(\)"), and [`Release()`](BMediaNode.md#BMediaNode_Release) can be called directly once the node is registered—if you do this, be sure you don't call them after the node is destroyed.

Calling a node's member functions directly from outside the node itself can result in the chain of functions involved in coordinating nodes to be called out of order. Worse, deadlock can result. So just don't do it, even if you think you've found a safe way to pull it off.

* * *

### Creating Your Own Node

#### Realtime Allocators and Thread Locking

Media nodes are highly timing-sensitive creatures. The slightest delay in performing their work can cause drastic problems in media playback or recording quality. Virtual memory, normally of great benefit to users, can work against them when doing media work. A poorly-timed virtual memory hit can cause breaks in media performances.

The realtime memory allocation and locking functions provide a means for nodes to lock down their memory to prevent it from being cached to disk by the virtual memory system. This avoids situations in which the node has to pause while it or its memory is fetched back from the swap file.

The user can use the Media preference application to configure what types of nodes should use locked memory. Nodes should typically use the realtime memory allocation functions instead of `malloc()` and `free()`. [`rtm_alloc()`](TheMediaKit_RealTimeFunctions.md#rtm_alloc "rtm_alloc\(\), rtm_realloc\(\)") will automatically handle locking the memory if the `B_MEDIA_REALTIME_ALLOCATOR` flag is set, so your node doesn't have to worry about it.

In addition, if the realtime flag corresponding to the type of node you're writing is set, your node should also call [`media_realtime_init_thread()`](TheMediaKit_Functions.md#media_realtime_init_thread "media_realtime_init_thread\(\)") to lock down the stacks of its threads. Properly-written nodes can always call [`media_realtime_init_thread()`](TheMediaKit_Functions.md#media_realtime_init_thread "media_realtime_init_thread\(\)"), without checking the realtime flags, because this function will return `B_MEDIA_REALTIME_DISABLED` if the corresponding flag isn't set. You can simply ignore the error and move on.

For example:
    
    
    int32 *myThreadData = rtm_alloc(4096);
    myThread = spawn_thread(myThreadFunction, "Node Thread",
    B_NORMAL_PRIORITY, &myThreadData);
    status_t err = media_realtime_init_thread(myThread, 32768,
             B_MEDIA_REALTIME_VIDEO);
    if (err != B_OK && err != B_MEDIA_REALTIME_DISABLED) {
       printf("Can't lock down the thread.n");
    }
    ...

If your node requires realtime performance from an add-on or shared library, you can use the [`media_realtime_init_image()`](TheMediaKit_Functions.md#media_realtime_init_image "media_realtime_init_image\(\)") function to lock down that image in memory. Note, however, that any uses of `malloc()` by that image won't allocate locked memory; you can't control that. Still, locking down the image itself can help performance even further.

Note

![Note](./images/admonitions/Info_32.png)

Standard BeOS system libraries are Be's responsibility. If it's appropriate for them to be locked, they're locked for you. Don't lock them yourself. Both `libmedia.so` and `libroot.so` have [`media_realtime_init_image()`](TheMediaKit_Functions.md#media_realtime_init_image "media_realtime_init_image\(\)") called on them.

#### Negotiating a Connection

Establishing a connection between two nodes is a multi-step process. The nodes need to agree upon a data format they both support before the connection can even be established.

##### Special Considerations

If the node you're writing could be connected by the system mixer (using the Audio preference application, for example) as the default output, the node needs to be as flexible as possible in terms of the formats it accepts on its free inputs in the [`GetNextInput()`](BBufferConsumer.md#BBufferConsumer_GetNextInput "GetNextInput\(\)") function. The format your node returns from [`GetNextInput()`](BBufferConsumer.md#BBufferConsumer_GetNextInput "GetNextInput\(\)") will be used as the starting poing in the negotiation process; the more wildcards you support, the better.

An application that wants to establish a connection between some other node and your node will determine the format from the inputs into your node and the outputs from the other node, then call [`BMediaRoster::Connect()`](BMediaRoster.md#BMediaRoster_Connect "Connect\(\), Disconnect\(\)") with that format.

If there are any wildcards in the format passed to `BMediaRoster::Format()`, the media roster will call `BBufferProducer::ProposeFormat()` in the node being connected to your output node; the producer will specialize the wildcards to construct the least-specific format that will guarantee that any remaining wildcards can be specialized by your node without becoming incompatible with the producer.

The resulting format may have some wildcards left (or, if the producer is particularly picky, there may be none at all). The media roster will then pass this format to your consumer node's [`BBufferConsumer::AcceptFormat()`](BBufferConsumer.md#BBufferConsumer_AcceptFormat "AcceptFormat\(\)") function. This function should be implemented to specialize the remaining wildcards and return this format, which should describe a specific format. This format will be used to establish the connection.

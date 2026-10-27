<!-- Classes And Methods - The Media Kit - BMediaNode -->

## BMediaNode  
  
| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `media/MediaNode.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BMediaNode_Overview.md "BMediaNode")

  * [Constructor and Destructor](BMediaNode.md#BMediaNode_ConstructorDestructor)
  * [Member Functions](BMediaNode.md#BMediaNode_MemberFunctions)
  * [Constants](BMediaNode.md#BMediaNode_Constants)

  
  
* * *

### Constructor and Destructor

#### BMediaNode()

##### protected

`explicit BMediaNode(const char* name);`

Call this from your derived node's constructor.

The node is created with a reference count of 1; the count is incremented each time the [`Acquire()`](BMediaNode.md#BMediaNode_Acquire "Acquire\(\), Release\(\)") call is issued, and decremented each time [`Release()`](BMediaNode.md#BMediaNode_Release) is called. When the reference count becomes zero, the node is deleted.

#### ~BMediaNode()

`virtual ~BMediaNode();`

You may never delete a `BMediaNode` because you don't know for certain when the Media Server is done with it. Instead, the Media Server maintains a reference count for the node, and when it is no longer in use, the node will be deleted automatically.

* * *

### Member Functions

####  Acquire(), Release()

`BMediaNode* Acquire();``BMediaNode* Release();`

`Acquire()` returns a pointer to the node, after incrementing the node's reference count.

`Release()` releases the node by decrementing its reference count. If the count reaches zero, the node is deleted and `NULL` is returned; otherwise, a pointer to the node is returned.

Note

![Note](./images/admonitions/Info_32.png)

Although you usually can't call node member functions directly from within an application, you can call `Acquire()` and `Release()` directly if the node is subclassed within the application itself (rather than in an add-on).

#### AddNodeKind()

`void AddNodeKind(uint64 kind);`

Adds a kind to the set of kinds supported by the node. This lets the system know what types of node interfaces are supported by the node's implementation. Possible values include `B_BUFFER_PRODUCER` (which indicates that the node implements the [`BBufferProducer`](BBufferProducer.md "BBufferProducer") protocol) and `B_PHYSICAL_INPUT` (which indicates that the node implements a physical input, such as a sound digitizing input device). For a complete list of kind values, see [node_kind](TheMediaKit_Constants.md#Enums_node_kind "node_kind").

Note

![Note](./images/admonitions/Info_32.png)

In general, you don't need to call this function. The base system classes call `AddNodeKind()` automatically to set up the node type flags; for example, a [`BBufferProducer`](BBufferProducer.md "BBufferProducer") automatically calls `AddNodeKind`(`B_BUFFER_PRODUCER`). The only time it's necessary go call `AddNodeKind()` is if the node you're implementing is a physical device or a mixer, in which case you need to add the `B_PHYSICAL_INPUT`, `B_PHYSICAL_OUTPUT`, or `B_SYSTEM_MIXER` flag.

#### AddOn()

`virtual BMediaAddOn* AddOn(int32* outInternalID) const = 0;`

Implement this function to return a pointer to the [`BMediaAddOn`](BMediaAddOn.md "BMediaAddOn") that instantiated the node. If the node lives in an application (rather than in an add-on), return `NULL`. If the node is in an add-on, `outInternalID` should be changed to contain the internal ID number of the node within the add-on.

####  AddTimer(), TimerExpired()

`virtual status_t AddTimer(bigtime_t toPerformanceTime,  
int32 cookie);``void TimerExpired(bigtime_t notifyPoint,  
int32 cookie,  
status_t error = B_OK);`

Your node should implement the `AddTimer()` function to remember the `cookie` and time given. When the time `toPerformanceTime` is reached, your node should call `TimerExpired()` with the corresponding `cookie` value, passing the recorded `toPerformanceTime` value as the `notifyPoint` argument. This will, in turn, cause the [`BMediaRoster::SyncToNode()`](BMediaRoster.md#BMediaRoster_SyncToNode "SyncToNode\(\)") call that instigated the timer to return to the caller.

Your implementation of `AddTimer()` should return `B_OK` if all is well; otherwise it should return an appropriate error code.

#### ControlPort()

`virtual port_id ControlPort()  const = 0;`

Returns the port_id of the port to which the node listens for requests. Your node must implement this to return a valid Kernel Kit port.

#### DeleteHook()

##### protected

`virtual status_t DeleteHook(BMediaNode* node);`

The `DeleteHook()` function is called to delete the `BMediaNode` object. You may augment this if you need to perform additional work before the node is deleted, but you should always either include the line:
    
    
    delete this;

or you should call through to the inherited form of the function. Return `B_OK` if the node was deleted successfully, otherwise return an appropriate error code.

#### GetNodeAttributes()

`virtual status_t GetNodeAttributes(media_node_attribute* outAttributes,  
size_t inMaxCount);`

Implement this function to fill the `outAttributes` array (which has room for `inMaxCount` attributes) with your node's attributes.

Return `B_OK` if all is well, or return an appropriate error code.

#### HandleBadMessage()

`void HandleBadMessage(int32 message,  
const void* data,  
size_t size);`

If your node receives a message that neither the node, nor any interface from which the node is derived, understands the message, pass the message along to this function, which will work magic to deal with the problem one way or another. All arguments received by the [`HandleMessage()`](BMediaNode.md#BMediaNode_HandleMessage "HandleMessage\(\)") function should be passed directly through to `HandleBadMessage()`.

#### HandleMessage()

`virtual status_t HandleMessage(int32 message,  
const void* data,  
size_t size);`

Given a message received on the control port, this function dispatches the message to the appropriate `BMediaNode` hook function. If the message doesn't correspond to a hook function, `B_ERROR` is returned.

When you implement a media node of your own (derived from [`BBufferConsumer`](BBufferConsumer.md "BBufferConsumer"), [`BBufferProducer`](BBufferProducer.md "BBufferProducer"), etc), you always need to call through to `BMediaNode::HandleMessage()` from your node's implementation of `HandleMessage()`. This is crucial, to be sure that every ancestor of your node gets to look at the message and attempt to process it.

For example, if your node inherits from both [`BBufferProducer`](BBufferProducer.md "BBufferProducer") and [`BBufferConsumer`](BBufferConsumer.md "BBufferConsumer"), you should call [`BBufferProducer::HandleMessage()`](BBufferProducer.md#BBufferProducer_HandleMessage "HandleMessage\(\)") and [`BBufferConsumer::HandleMessage()`](BBufferConsumer.md#BBufferConsumer_HandleMessage "HandleMessage\(\)"), `then BMediaNode::HandleMessage()`, like this:
    
    
    virtual status_t MyBufferProducerConsumer::HandleMessage(int32 message,
             const void* data, size_t size) {
       if (message == SOME_THING_I_DO) {
          DoWhatever();
       }
       else if (BBufferConsumer::HandleMessage(message, data, size) &&
                BBufferProducer::HandleMessage(message, data, size) &&
                BMediaNode::HandleMessage(message, data, size)) {
          BMediaNode::HandleBadMessage(message, data, size);
       }
    }

Note that [`BMediaNode::HandleBadMessage()`](BMediaNode.md#BMediaNode_HandleBadMessage "HandleBadMessage\(\)") is called if none of the `HandleMessage()` implementations accept the message.

Values of message between 0x60000000 and 0x7FFFFFFF are available for use by applications. Values below 0x60000000 are reserved for use by the Media Kit, and typically correspond to specific virtual hook functions within your node. If you can show just cause for needing to know the message value for a particular hook, you can try emailing devsupport@be.com and see if we agree with you, in which case we may share that information.

Don't reverse-engineer the message values; if you really need to know, ask us. Otherwise, we won't know that a particular message code number shouldn't be changed. In general, it's a bad idea to rely on specific values, although there may be cases in which it's necessary.

Return Code| Description  
---|---  
`B_OK`| The message was dispatched.  
`B_ERROR`| The message couldn't be dispatched, possibly because it doesn't correspond to a hook function.  
  
#### ID()

`media_node_id ID() const;`

Returns the [media_node_id](TheMediaKit_DefinedTypes.md#media_node_id "media_node_id") assigned to the node by the Media Server. The result is 0 if the node hasn't been registered yet, and negative if an error occurred while attempting to register the node.

#### Kinds()

`uint64 Kinds() const;`

Returns a bit mask indicating what interfaces the node implements. See [node_kind](TheMediaKit_Constants.md#Enums_node_kind "node_kind") for a list of valid interface kinds.

#### Name()

`const char* Name() const;`

Returns a human-readable string specifying the node's name. This pointer is only valid until you [`Release()`](BMediaNode.md#BMediaNode_Release) the node; after that, the pointer may point into empty space.

#### NewChangeTag()

##### protected

`static int32 NewChangeTag();`

This function, intended primarily for use by [`BBufferConsumer`](BBufferConsumer.md "BBufferConsumer") nodes, creates and returns a new change tag value.

#### Node()

`media_node Node() const;`

Returns the [media_node](TheMediaKit_DefinedTypes.md#media_node "media_node") structure that will be used by an application when accessing this node via the media roster.

#### NodeRegistered()

`virtual void NodeRegistered();`

The Media Server calls this hook function after the node has been registered.

#### NodeStopped()

`status_t NodeStopped(bigtime_t whenPerformanceTime) const;`

When you've finished handling a stop request (buffers will no longer be flowing), call this function. If anyone is listening for stop notifications from you, they'll be notified. The `whenPerformanceTime` argument should be the performance time of the stop command that was handled.

Anyone listening for node stop messages will be notified; this lets applications running in offline (rendering) mode know when the node has actually completed its work.

If your node is a [`BBufferProducer`](BBufferProducer.md "BBufferProducer"), downstream consumers will be notified that your node stopped (automatically, no less) through the `BBufferConsumer`::`ProducerDataStatus`(`B_PRODUCER_STOPPED`) call. This lets offline rendering nodes know when each of their inputs have no more data to send for the current roll.

Note

![Note](./images/admonitions/Info_32.png)

This is especially important for nodes that can be run in `B_OFFLINE` mode.

Return Code| Description  
---|---  
`B_OK`| No error.  
Other errors.| Unable to communicate with the Media Server, or an error occurred communicating with other nodes.  
  
#### Preroll()

##### protected

`virtual void Preroll();`

This hook function may be called before your node receives a [`Start()`](BMediaNode.md#BMediaNode_Start "Start\(\)") message if the application using the node calls [`BMediaRoster::PrerollNode()`](BMediaRoster.md#BMediaRoster_PrerollNode "PrerollNode\(\)"). This gives the node a chance to prepare the media so that when the media is started, the response is as fast as possible.

#### ReportError()

##### protected

`status_t ReportError(node_error whichError,  
const BMessage* info = NULL);`

Transmits the error code specified by `whichError` to anyone that's receiving notifications from this node (see [`BMediaRoster::StartWatching()`](BMediaRoster.md#BMediaRoster_StartWatching "StartWatching\(\), StopWatching\(\)") and [`BMediaRoster::StopWatching()`](BMediaRoster.md#BMediaRoster_StopWatching) on ). If `info` isn't `NULL`, it's used as a model message for the error notification message.

Return Code| Description  
---|---  
`B_OK`| The error report was sent without error.  
[`BMessage`](BMessage.md "BMessage") errors.| The message couldn't be sent.  
  
#### RequestCompleted()

##### protected

`virtual status_t RequestCompleted(const media_request_info& info = NULL);`

This function is called whenever a request issued by the node is completed. The `info` structure describes the results of the request.

The `change_tag` field in the `info` structure identifies the request that has been completed; this is the same value passed into the function that initiated the request.

Return `B_OK` if you're happy, otherwise return an appropriate error code.

####  RunMode(), SetRunMode()

##### protected

`run_mode RunMode() const;`

`RunMode()` returns the node's current [run_mode](BMediaNode.md#Enums_run_mode "run_mode") setting.

The `SetRunMode()` hook function is called when someone requests that your node's run mode be changed.

#### Seek()

##### protected

`virtual void Seek(bigtime_t mediaTime,  
bigtime_t performanceTime);`

This hook function is called when a node is asked to seek to the specified `mediaTime` by a call to the [`BMediaRoster`](BMediaRoster.md "BMediaRoster").

The specified `performanceTime`, the time at which the node should begin the seek operation, may be in the future.

Note

![Note](./images/admonitions/Info_32.png)

Your node is required to queue at least one each of start, stop, and seek requests, so that applications can establish, for example, both the start and stop time without having to monitor your node's progress. The actual size of these three queues is up to you. When the specified time arrives, the request should be filled.

A `mediaTime` value of 0 indicates the beginning of the media data.

####  SetTimeSource(), TimeSource()

`virtual void SetTimeSource(BTimeSource* timeSource);``BTimeSource* TimeSource() const;`

The `SetTimeSource()` hook function is called when someone has requested that the node be slaved to a new time source. Augment this function to make whatever adjustments you need to make to operate at the new time scale.

`TimeSource()` returns a pointer to the [`BTimeSource`](BTimeSource.md "BTimeSource") to which the node is currently slaved. If no time source has been explicitly requested, the system time source is in use, and that's what gets returned.

Warning

![Warning](./images/admonitions/Stop_32.png)

The [`BTimeSource`](BTimeSource.md "BTimeSource") object returned by `TimeSource()` is only valid until the next call to [`HandleMessage()`](BMediaNode.md#BMediaNode_HandleMessage "HandleMessage\(\)") on that object. Therefore, if your node runs more than one thread, you need to serialize calls to `TimeSource()` (as well as usage of the returned objects) with calls to [`HandleMessage()`](BMediaNode.md#BMediaNode_HandleMessage "HandleMessage\(\)") This isn't a problem if you follow the recommended policy of running a single thread that monitors the service port with [`read_port_etc()`](TheKernelKit_Ports.md#read_port_etc) and calls [`HandleMessage()`](BMediaNode.md#BMediaNode_HandleMessage "HandleMessage\(\)") only when a message is actually received.

#### Start()

##### protected

`virtual void Start(bigtime_t performanceTime);`

This hook function is called when a node is started by a call to the [`BMediaRoster`](BMediaRoster.md "BMediaRoster").

The specified `performanceTime`, the time at which the node should start running, may be in the future.

Note

![Note](./images/admonitions/Info_32.png)

Your node is required to queue at least one each of start, stop, and seek requests, so that applications can establish, for example, both the start and stop time without having to monitor your node's progress. The actual size of these three queues is up to you. When the specified time arrives, the request should be filled.

#### Stop()

##### protected

`virtual void Stop(bigtime_t performanceTime,  
bool immediate);`

This hook function is called when a node is stopped by a call to the [`BMediaRoster`](BMediaRoster.md "BMediaRoster").

The specified `performanceTime`, the time at which the node should stop, may be in the future.

If `immediate` is `true`, your node should ignore the `performanceTime` value and synchronously stop performance. When `Stop()` returns, you're promising not to write into any [`BBuffer`](BBuffer.md "BBuffer")s you may have received from your downstream consumers, and you promise not to send any more buffers until [`Start()`](BMediaNode.md#BMediaNode_Start "Start\(\)") is called again.

Note

![Note](./images/admonitions/Info_32.png)

Your node is required to queue at least one each of start, stop, and seek requests, so that applications can establish, for example, both the start and stop time without having to monitor your node's progress. The actual size of these three queues is up to you. When the specified time arrives, the request should be filled.

Nodes must recycle all buffers they may be holding onto when they're stopped.

#### TimeWarp()

##### protected

`virtual void TimeWarp(bigtime_t atRealTime,  
bigtime_t newPerformanceTime);`

This hook function is called when the time source to which the node is slaved is repositioned (via a seek operation) such that there will be a sudden jump in the performance time progression as seen by the node. The `newPerformanceTime` argument indicates the new performance time; the change should occur at the real time specified by the `atRealTime` argument.

The node should respond to this call by preparing for this change, so a serious stutter, failure, or acceleration in performance doesn't occur. Appropriate measures should be taken to minimize the impact on the performance quality; for example, a segment of the sound could be looped or skipped smoothly.

Your implementation of `TimeWarp()` should call through to `BMediaNode::TimeWarp()` as well as all other inherited forms of `TimeWarp()`.

#### WaitForMessage()

##### protected

`status_t WaitForMessage(bigtime_t waitUntil,  
uint32 flags = 0,  
void* _reserved_ = NULL);`

This function waits until either real time specified by `waitUntil` or a message is received on the control port.. The `flags` are currently unused and should be 0.

When a message is received, the appropriate [`HandleMessage()`](BMediaNode.md#BMediaNode_HandleMessage "HandleMessage\(\)") calls are made given the class derivation of the node:

  * [`BMediaNode::HandleMessage()`](BMediaNode.md#BMediaNode_HandleMessage "HandleMessage\(\)") is always called first.

  * If the node is derived from [`BBufferProducer`](BBufferProducer.md "BBufferProducer"), and the message hasn't been handled yet, [`BBufferProducer::HandleMessage()`](BBufferProducer.md#BBufferProducer_HandleMessage "HandleMessage\(\)") is called.

  * If the node is derived from [`BBufferConsumer`](BBufferConsumer.md "BBufferConsumer"), and the message hasn't been handled yet, [`BBufferConsumer::HandleMessage()`](BBufferConsumer.md#BBufferConsumer_HandleMessage "HandleMessage\(\)") is called.

  * If the node is derived from [`BFileInterface`](BFileInterface.md "BFileInterface"), and the message hasn't been handled yet, [`BFileInterface::HandleMessage()`](BFileInterface.md#BFileInterface_HandleMessage "HandleMessage\(\)") is called.

  * If the node is derived from [`BControllable`](BControllable.md "BControllable"), and the message hasn't been handled yet, [`BControllable::HandleMessage()`](BControllable.md#BControllable_HandleMessage "HandleMessage\(\)") is called.

  * If the node is derived from [`BTimeSource`](BTimeSource.md "BTimeSource"), and the message hasn't been handled yet, [`BTimeSource::HandleMessage()`](BTimeSource.md#BTimeSource_HandleMessage "HandleMessage\(\)") is called.

  * If the message still hasn't been handled, the most-derived interface's `HandleMessage()` function is called.

  * If the message hasn't been handled, [`HandleBadMessage()`](BMediaNode.md#BMediaNode_HandleBadMessage "HandleBadMessage\(\)") is called.

Once this has been done, `WaitForMessage()` returns. As you can see, this can be called from your control port to handle much of the work of processing received messages.

Return Code| Description  
---|---  
`B_OK`| A message has occurred within the given time period.  
`B_TIMED_OUT`| The time `waitUntil` has arrived without a message being received.  
  
* * *

### Constants

####  node_error

Declared in: `media/MediaNode.h`

The node_error type defines the errors a node can transmit to [`BMessenger`](BMessenger.md "BMessenger")s that have registered to watch the node.

Constant| Description  
---|---  
`B_NODE_FAILED_START`| The node failed on a [`Start()`](BMediaNode.md#BMediaNode_Start "Start\(\)") request.  
`B_NODE_FAILED_STOP`| The node failed on a [`Stop()`](BMediaNode.md#BMediaNode_Stop "Stop\(\)") request.  
`B_NODE_FAILED_SEEK`| The node failed on a [`Seek()`](BMediaNode.md#BMediaNode_Seek "Seek\(\)") request.  
`B_NODE_FAILED_SET_RUN_MODE`| The node's [run_mode](BMediaNode.md#Enums_run_mode "run_mode") couldn't be set.  
`B_NODE_FAILED_TIME_WARP`| The node couldn't fulfill a time warp request.  
`B_NODE_FAILED_PREROLL`| The node failed on a [`Preroll()`](BMediaNode.md#BMediaNode_Preroll "Preroll\(\)") request.  
`B_NODE_FAILED_SET_TIME_SOURCE_FOR`| The node's time source couldn't be changed.  
`B_NODE_IN_DISTRESS`| The node is suffering in general.  
  
####  run_mode

Declared in: `media/MediaNode.h`

The run_mode type indicates how a node should cope if its performance rate deviates from the desired rate.

Constant| Description  
---|---  
`B_OFFLINE`| Keep data accurate, even if the performance lags or runs too fast. This is typically used when rendering to disk.When in offline mode the node doesn't need to worry about processing buffers at any particular time. Each buffer's performance time should be derived from the time stamped on the buffer, rather than from a [`BTimeSource`](BTimeSource.md "BTimeSource"). In fact, you'll usually want to call [`set_thread_priority()`](TheKernelKit_ThreadsAndTeams.md#set_thread_priority "set_thread_priority\(\), suggest_thread_priority\(\)") to set your node's processing threads to a low priority while the node is in offline mode. This lets software render media to disk in an efficient manner, letting the user continue to work while the render occurs in the background.  
`B_RECORDING`| Time-stamped buffers are being received from a node capturing them from the real world; these buffers are guaranteed to have a time stamp in the past (they're always "late").Recording mode should be used when data is being sampled from a physical input device. These devices always deliver buffers whose time stamps are in the past (they're stamped with the time at which they were sampled, which is of course in the past, unless you've stolen a time machine from a professor from the 27th century, in which case you're probably running BeOS R127.1 and this book is woefully obsolete).Using `B_RECORDING` mode serves to warn other nodes that the time stamps will be in the past.  
`B_DECREASE_PRECISION`| If the performance starts to lag, try to catch up.In `B_DECREASE_PRECISION` mode, your node should attempt to catch up if it falls behind, by playing buffers of media data faster than normal. For audio, this might mean playing back at a higher sampling rate; for video, the frame rate might be temporarily boosted.  
`B_INCREASE_LATENCY`| If the performance starts to lag, increase playout delay so buffers are delivered with less time to spare before they're needed.If your node gets behind in the `B_INCREASE_LATENCY` run mode, your node should increase its internal latency measurement and send call the [`LateNoticeReceived()`](BBufferProducer.md#BBufferProducer_LateNoticeReceived "LateNoticeReceived\(\)") function in anyone above your node in the media stream.Your node should then try to produce each buffer earlier before the buffer's performance time from that point on, so there's more time for the buffers to reach their destination.This mode is intended to compensate for data streams in which throughput can vary over time. For example, if media data is being streamed over a network, traffic fluctuations may require your node to adapt by adding more buffering (latency).  
`B_DROP_DATA`| If the performance starts to lag, skip data.When in `B_DROP_DATA` mode, your node should simply skip buffers if if falls behind. Note that you still receive the buffers, but you should ignore any that you must in order to keep playing as many buffers as possible at the correct performance times.

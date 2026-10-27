<!-- Classes And Methods - The Media Kit - BMediaEventLooper -->

## BMediaEventLooper

| Derived From:| [`BMediaNode`](BMediaNode.md "BMediaNode")  
---|---  
Mix-in Classes:| –  
Declared In:| `media/MediaEventLooper.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BMediaEventLooper_Overview.md "BMediaEventLooper")

  * [Constructor and Destructor](BMediaEventLooper.md#BMediaEventLooper_ConstructorDestructor)
  * [Member Functions](BMediaEventLooper.md#BMediaEventLooper_MemberFunctions)
  * [Constants](BMediaEventLooper.md#BMediaEventLooper_Constants)

  
  
* * *

### Constructor and Destructor

#### BMediaEventLooper()

##### protected

`explicit BMediaEventLooper(uint32* apiVersion = B_BEOS_VERSION);`

You need to override this function to handle your node's initialization needs. The `apiVersion` argument indicates the version of the BeOS API the object was written for; you should let the default value be used—this will cause your node to inherit the API version of the BeOS version you're compiling under.

#### ~BMediaEventLooper()

##### protected

`virtual ~BMediaEventLooper();`

Calls [`Quit()`](BMediaEventLooper.md#BMediaEventLooper_Quit "Quit\(\)") to stop the control thread and free allocated memory.

* * *

### Member Functions

#### CleanUpEvent()

##### protected

`virtual void CleanUpEvent(const [media_timed_event](BTimedEventQueue.md#media_timed_event "media_timed_event")* event);`

Implement this function to clean up after custom events you've created and added to your queue. It's called when a custom event is removed from the queue, to let you handle any special tidying-up that the event might require.

#### ControlLoop()

##### protected

`virtual void ControlLoop();`

This function waits for messages, pops events off the queue, and calls [`DispatchEvent()`](BMediaEventLooper.md#BMediaEventLooper_DispatchEvent "DispatchEvent\(\)"). It's called automatically when the `BMediaEventLooper` is [`Run()`](BMediaEventLooper.md#BMediaEventLooper_Run "Run\(\)").

Warning

![Warning](./images/admonitions/Stop_32.png)

If you choose to reimplement this function, be very careful; it's very easy to cause Bad Things to happen.

#### ControlThread()

`thread_id ControlThread();`

Returns the control thread's [thread_id](TheKernelKit_ThreadsAndTeams.md#thread_id).

#### DispatchEvent()

##### protected

`void DispatchEvent(const [media_timed_event](BTimedEventQueue.md#media_timed_event "media_timed_event")* event,  
bigtime_t lateness,  
bool realTimeEvent = false);`

Calls [`HandleEvent()`](BMediaEventLooper.md#BMediaEventLooper_HandleEvent "HandleEvent\(\)") to let your code handle the specified event. If your code doesn't handle it, this function may have a default handler to process it. In general, you won't call this function.

`BMediaEventLooper` compensates your performance time by adding the event latency (see [`SetEventLatency()`](BMediaEventLooper.md#BMediaEventLooper_SetEventLatency "SetEventLatency\(\), EventLatency\(\)")) and the scheduling latency (or, for real-time events, only the scheduling latency).

It's the control loop's job to remove the event from the queue; this function doesn't do that.

####  EventQueue(), RealTimeQueue()

##### protected

`BTimedEventQueue* EventQueue();``BTimedEventQueue* RealTimeQueue();`

`EventQueue()` returns a pointer to the [`BTimedEventQueue`](BTimedEventQueue.md "BTimedEventQueue") used by the message handler. `RealTimeQueue()` returns a pointer to the [`BTimedEventQueue`](BTimedEventQueue.md "BTimedEventQueue") used to queue real-time events.

Note

![Note](./images/admonitions/Info_32.png)

Events in the real-time queue are handled according to their real time, while events in the normal event queue are handled based on their performance time. So the values of the time stamps on events in these two queues are handled differently; keep this in mind if you need to peek into the queues yourself.

#### HandleEvent()

##### protected

`virtual void ControlLoop(const [media_timed_event](BTimedEventQueue.md#media_timed_event "media_timed_event")* event,  
bigtime_t lateness,  
bool realTimeEvent = false);`

Implement this function to handle incoming media events. The `event` argument references a [media_timed_event](BTimedEventQueue.md#media_timed_event "media_timed_event") structure that describes the event. `lateness` indicates how late the event is, and `realTimeEvent` is `true` if the event needs to be handled in real time.

The `BMediaEventLooper` will call this function from the [`DispatchEvent()`](BMediaEventLooper.md#BMediaEventLooper_DispatchEvent "DispatchEvent\(\)") function. It's the control loop's job to remove the event from the queue.

#### NodeRegistered()

##### protected

`virtual void NodeRegistered();`

The Media Server calls this hook function after the node has been registered. This is derived from [`BMediaNode`](BMediaNode.md "BMediaNode"); `BMediaEventLooper` implements it to call [`Run()`](BMediaEventLooper.md#BMediaEventLooper_Run "Run\(\)") automatically when the node is registered; if you implement `NodeRegistered()` you should call through to [`BMediaNode::NodeRegistered()`](BMediaNode.md#BMediaNode_NodeRegistered "NodeRegistered\(\)") after you've done your custom operations.

#### Quit()

`void Quit();`

Closes the node's control port and closes the control thread. Blocks until the control thread is gone.

#### Run()

`void Run();`

Spawns and runs the control thread; this is called automatically by the default [`NodeRegistered()`](BMediaEventLooper.md#BMediaEventLooper_NodeRegistered "NodeRegistered\(\)") implementation. If you override [`NodeRegistered()`](BMediaEventLooper.md#BMediaEventLooper_NodeRegistered "NodeRegistered\(\)"), be sure you call through to the default implementation, or call `Run()`.

#### SchedulingLatency()

##### protected

`bigtime_t SchedulingLatency() const;`

Returns the scheduling latnecy, in microseconds, of the node.

####  SetBufferDuration(), BufferDuration()

##### protected

`void SetBufferDuration(bigtime_t duration);``bigtime_t BufferDuration() const;`

`SetBufferDuration()` sets the duration of the node's buffers. The duration is clamped to 0 if it's less than 0.

`BufferDuration()` returns the duration of the nodes' buffers.

####  SetEventLatency(), EventLatency()

##### protected

`void SetEventLatency(bigtime_t latency);``bigtime_t EventLatency() const;`

`SetEventLatency()` sets the event latency. The event latency is the upstream and downstream algorithmic latency of your node—but should not include scheduling latency. This latency is taken into account by the `BMediaEventLooper` when deciding when to pop events off the queue for you to process.

`EventLatency()` returns the event latency.

####  SetOfflineTime(), OfflineTime()

##### protected

`void SetOfflineTime(bigtime_t offlineTime);``virtual bigtime_t OfflineTime();`

`SetOfflineTime()` sets the time that `OfflineTime()` will return.

Augment `OfflineTime()` to compute the node's current time; it's called by the Media Kit when it's in offline mode. Update any appropriate internal information as well, then call through to the `BMediaEventLooper` implementation.

####  SetPriority(), Priority()

##### protected

`void SetPriority(int32 priority);``int32 Priority() const;`

`SetPriority()` sets the control thread's priority. Values less than 0 are clamped to 0, and values greater than 120 are clamped to 120.

`Priority()` returns the control thread's current priority.

####  SetRunState(), RunState()

##### protected

`void SetRunState(run_state state);``run_state RunState() const;`

`SetRunState()` sets the node's current run state.

`RunState()` returns the current run state.

* * *

### Constants

#### run_state

Declared in: `media/MediaEventLooper.h`

Constant| Description  
---|---  
`B_IN_DISTRESS`.| The node is experiencing problems.  
`B_UNREGISTERED`.| The node hasn't been registered with the Media roster yet.  
`B_STOPPED`.| The node isn't running.  
`B_STARTED`.| The node is running.  
`B_QUITTING`.| The node's in the process of shutting down.  
`B_TERMINATED`.| The node has been terminated.  
`B_USER_RUN_STATES`.| Base for user-defined run states.

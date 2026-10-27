<!-- Classes And Methods - The Media Kit - BTimeSource -->

## BTimeSource

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `media/TimeSource.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BTimeSource_Overview.md "BTimeSource")

  * [Constructor and Destructor](BTimeSource.md#BTimeSource_ConstructorDestructor)
  * [Member Functions](BTimeSource.md#BTimeSource_MemberFunctions)
  * [Constants](BTimeSource.md#BTimeSource_Constants)
  * [Defined Types](BTimeSource.md#BTimeSource_DefinedTypes)

  
  
* * *

### Constructor and Destructor

#### BTimeSource()

##### protected

`BTimeSource();`

The standard `BTimeSource` constructor. You should never directly instantiate a `BTimeSource`; instead, you should create a node class derived from `BTimeSource` (and possibly other [`BMediaNode`](BMediaNode.md "BMediaNode")-derived classes as well) and use the [`BMediaRoster`](BMediaRoster.md "BMediaRoster") to instantiate the node as desired.

* * *

### Member Functions

#### BroadcastTimeWarp()

`void BroadcastTimeWarp(bigtime_t atRealTime,  
bigtime_t newPerformanceTime);`

Whenever time jumps suddenly (for instance, when a seek operation occurs), call this function to inform all slaved nodes that time has jumped in a discontinuous manner. You should also call this function if the time from which your time source derives its time jumps.

At the real time specified by `atRealTime`, the performance time will instantaneously jump to `newPerformanceTime`.

#### GetStartLatency()

`status_t GetStartLatency(bigtime_t* outLatency);`

Returns in `outLatency` the amount of time, in microseconds, needed for the time source to start up, including the time needed to start up any slaved nodes that are started.

Return Code| Description  
---|---  
`B_OK`| The latency value was returned successfully.  
`B_NO_INIT`| The time source has not been initialized.  
  
#### GetTime()

`status_t GetTime(bigtime_t* performanceTime,  
bigtime_t* realTime,  
float* drift);`

Returns the most recently published time information for this time source; this information specifies the performance time and real time of the last published time stamp, as well as the `drift` value, which can be used to interpolate the true current performance time, given a more accurate real time, as follows:
    
    
    bigtime_t performanceTime;
    bigtime_t realTime;
    float drift;
    
    while (GetTime(&performanceTime, &realTime, &drift) != B_OK);
    performanceTime = performanceTime + (RealTime() - realTime) * drift;

Note, however, that the resulting `performanceTime` is the same value you would have gotten by calling `PerformanceTimeFor()`, which you should use instead.

Return Code| Description  
---|---  
`B_OK`| The returned information is accurate.  
Other values.| The returned information is unreliable.  
  
#### HandleMessage()

##### protected

`status_t HandleMessage(int32 code,  
const void* message,  
size_t size);`

Given a `message` received on the control port, this function dispatches the message to the appropriate `BTimeSource` hook function. If the message doesn't correspond to a hook function, an appropriate error be returned.

If your node derives from `BTimeSource`, your implementation of `HandleMessage()` should call all inherited forms of `HandleMessage()`.

Return Code| Description  
---|---  
`B_OK`| The message was dispatched.  
Other values.| The message couldn't be dispatched, possibly because it doesn't correspond to a hook function.  
  
#### IsRunning()

`bool IsRunning();`

Returns `true` if the `BTimeSource` is currently progressing through time or `false` if it's stopped.

####  Now(), PerformanceTimeFor(), RealTimeFor()

`bigtime_t Now();``bigtime_t PerformanceTimeFor(bigtime_t realTime);``bigtime_t PerformanceTimeFor(bigtime_t performanceTime,  
bigtime_t withLatency);`

`Now()` returns an approximation of what the current performance time is.

`PerformanceTimeFor()` returns an estimate of the performance time represented by the specified real time (as returned by [`BTimeSource::RealTime()`](BTimeSource.md#BTimeSource_RealTime "RealTime\(\)")).

`RealTimeFor()`, given a performance time, returns an approximation of the corresponding real time, adjusted by the given latency.

#### PublishTime()

##### protected

`void PublishTime(bigtime_t performanceTime,  
bigtime_t realTime,  
float drift);`

While your time source is running, you should repeatedly call this function in order to constantly refresh the mapping between real time and performance time. When your time source is stopped, you should call this function once with values of zero for all three arguments.

The arguments have the following meanings:

Parameter| Description  
---|---  
`performanceTime`.| Is the precise current performance time.  
`realTime`.| Is the current real time.  
`drift`.| Indicates the value which indicates the rate at which the performance time changes compared to real time.  
  
The `drift` value makes it possible to interpolate intermediate values. For instance, if playback of a video source is progressing at normal speed, the `drift` value would be 1.0, indicating that performance time and real time progress at the same rate.

However, if the movie is playing at half-speed, `drift` would be 0.5, so that for every one unit of real time that passes, only a half-unit of performance time would pass. This information is used to compute times without having to query the time source each time an update must occur.

The data furnished by this function (which you should try to call about 20 times a second, although variations in frequency are acceptable as long as the drift value doesn't change much) may be the only information the rest of the world sees from your node.

#### RealTime()

`static bigtime_t RealTime();`

Returns the current absolute real time reference that all time sources measure themselves against. This is the only call that you should rely upon to obtain this value (don't use the Kernel Kit's [`system_time()`](TheKernelKit_Time.md#system_time "system_time\(\)") function).

As of this time, `RealTime()` and [`system_time()`](TheKernelKit_Time.md#system_time "system_time\(\)") do return the same value; however, you shouldn't rely upon this relationship. When doing media stuff, you should always use `RealTime()`.

#### Seek()

##### protected

`virtual void Seek(bigtime_t performanceTime,  
bigtime_t atRealTime);`

Implement this function to handle a seek request. When a `BTimeSource`'s performance time is adjusted, it needs to broadcast the change to all nodes slaved to it; call [`BroadcastTimeWarp()`](BTimeSource.md#BTimeSource_BroadcastTimeWarp "BroadcastTimeWarp\(\)") to do this.

Be sure to queue at least one seek request, so seek operations can be requested in advance. The seek request should occur at the real time specified by `atRealTime`.

#### SendRunMode()

##### protected

`void SendRunMode(run_mode mode);`

This function transmits the specified mode to all the nodes slaved to this node, so they know that their time source's run mode has changed. This function is called by [`SetRunMode()`](BTimeSource.md#BTimeSource_SetRunMode "SetRunMode\(\)"); it may or may not make sense to call it elsewhere, depending on your `BTimeSource` implementation.

#### SetRunMode()

##### protected

`virtual void SetRunMode([run_mode](BMediaNode.md#Enums_run_mode "run_mode") mode);`

This hook function is called when someone requests that your node's run mode be changed. Be sure to call through to either [`BMediaNode::SetRunMode()`](BMediaNode.md#BMediaNode_SetRunMode) or `BTimeSource::SetRunMode()`.

#### SnoozeUntil()

##### protected

`virtual void SnoozeUntil(bigtime_t performanceTime,  
bigtime_t withLatency = 0,  
bool retrySignals = false);`

Waits until the specified `performanceTime` (specified in performance time) arrives in real time. If `withLatency` is non-zero, `SnoozeUntil()` waits until `withLatency` microseconds (in real time) prior to the specified performance time; this lets you have time for setup prior to starting some operation.

Note

![Note](./images/admonitions/Info_32.png)

Because `performanceTime` is specified in performance time, and `withLatency` is specified in real time, you can't just subtract `withLatency` microseconds from `performanceTime` and snooze until that time, because `performanceTime` may be passing at a rate faster than or slower than real time (if, for example, the time source is running at double-speed).

If `retrySignals` is `false` (as is the default), `SnoozeUntil()` returns `B_INTERRUPTED` if it's interrupted by a signal before `performanceTime`; if `retrySignals` is `true`, the function will go right back to sleep and not return until the time has actually arrived.

Return Code| Description  
---|---  
`B_OK`| No error; the snooze has completed without incident.  
`B_MEDIA_TIME_SOURCE_STOPPED`| The time source is stopped; snoozing on a stopped time source is a bad thing.  
  
#### Start()

##### protected

`virtual void Start(bigtime_t realTime);`

This function is called when someone wants the `BTimeSource` to begin running; the `realTime` argument indicates the real time at which the time source should start. Implement the function to queue the request as necessary.

Be sure to queue at least one start request, so that start requests can be filed in advance.

Note

![Note](./images/admonitions/Info_32.png)

A `BTimeSource` measures start and stop times in real time, not in performance time, because `BTimeSource`s are the objects used to perform the mapping between real time and performance time.

#### Stop()

##### protected

`virtual void Stop(bigtime_t realTime);`

`Stop()` is called when someone wants the `BTimeSource` to stop; the `realTime` argument indicates the real time at which the time source should stop running. Implement the function to queue the request.

Be sure to queue at least one stop request, so stop requests can be filed in advance.

Note

![Note](./images/admonitions/Info_32.png)

A `BTimeSource` measures start and stop times in real time, not in performance time, because `BTimeSource`s are the objects used to perform the mapping between real time and performance time.

#### TimeSourceOp()

`virtual status_t TimeSourceOp(const [time_source_op_info](BTimeSource.md#time_source_op_info "time_source_op_info")& op,  
void* _reserved = 0);`

This function is called by the [`BMediaNode::Start()`](BMediaNode.md#BMediaNode_Start "Start\(\)"), [`BMediaNode::Stop()`](BMediaNode.md#BMediaNode_Stop "Stop\(\)"), and [`BMediaNode::Seek()`](BMediaNode.md#BMediaNode_Seek "Seek\(\)") functions to perform the requested activities.

You must implement this function to handle these requests.

Return `B_OK` if the request was handled successfully; otherwise, return an appropriate error code.

* * *

### Constants

#### time_source_op

Constant| Description  
---|---  
`B_TIMESOURCE_START`| Start the time source.  
`B_TIMESOURCE_STOP`| Stop the time source.  
`B_TIMESOURCE_STOP_IMMEDIATELY`| Stop the time source immediately.  
`B_TIMESOURCE_SEEK`| Seek the time source.  
  
These constants identify the various operations [`TimeSourceOp()`](BTimeSource.md#BTimeSource_TimeSourceOp "TimeSourceOp\(\)") will be called upon to handle.

* * *

### Defined Types

#### time_source_op_info
    
    
    struct time_source_op_info {
       time_source_op op;
       int32 _reserved1;
       bigtime_t real_time;
       bigtime_t performance_time;
       int32 _reserved2[6];
    };

Describes a request to [`TimeSourceOp()`](BTimeSource.md#BTimeSource_TimeSourceOp "TimeSourceOp\(\)").

Field| Description  
---|---  
`op`| Indicates the operation to be performed.  
`real_time`| Indicates the time at which the request is to be filled (this is the `atRealTime` argument passed to the [`BMediaRoster`](BMediaRoster.md "BMediaRoster") functions [`SeekTimeSource()`](BMediaRoster.md#BMediaRoster_SeekTimeSource "SeekTimeSource\(\)"), [`StopTimeSource()`](BMediaRoster.md#BMediaRoster_StopTimeSource), or [`StartTimeSource()`](BMediaRoster.md#BMediaRoster_StartTimeSource "StartTimeSource\(\), StopTimeSource\(\)")).  
`performance_time`| Is the `newPerformanceTime` argument passed to [`SeekTimeSource()`](BMediaRoster.md#BMediaRoster_SeekTimeSource "SeekTimeSource\(\)").

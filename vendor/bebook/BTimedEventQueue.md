<!-- Classes And Methods - The Media Kit - BTimedEventQueue -->

## BTimedEventQueue

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `media/TimedEventQueue.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BTimedEventQueue_Overview.md "BTimedEventQueue")

  * [Constructor and Destructor](BTimedEventQueue.md#BTimedEventQueue_ConstructorDestructor)
  * [Member Functions](BTimedEventQueue.md#BTimedEventQueue_MemberFunctions)
  * [Constants](BTimedEventQueue.md#BTimedEventQueue_Constants)
  * [Defined Types](BTimedEventQueue.md#BTimedEventQueue_DefinedTypes)

  
  
* * *

### Constructor and Destructor

#### BTimedEventQueue()

##### protected

`BTimedEventQueue();`

Initializes a new, empty, timed event queue.

#### ~BTimedEventQueue

`virtual ~BTimedEventQueue();`

Flushes the queue and deallocates memory allocated by the object.

* * *

### Member Functions

#### AddEvent()

`status_t AddEvent(const media_timed_event& event);`

Adds the specified event to the queue.

Return Code| Description  
---|---  
`B_OK`| The event was added.  
`B_BAD_VALUE`|  Unknown or invalid event type (you can't use `B_NO_EVENT` or `B_ANY_EVENT`).  
`B_NO_INIT`| The queue hasn't been initialized.  
  
#### DoForEach()

`status_t DoForEach(for_each_hook hook,  
void* context,  
bigtime_t time,  
time_direction direction,  
bool inclusive = true,  
int32 event = B_ANY_EVENT);`

For each event in the queue matching the specified parameters, the hook function is called. The `context` pointer is passed through to the hook function, and may point to anything your hook function requires.

  * Setting `direction` to `B_ALWAYS` indicates that all events of the type indicated by `event` should be processed. The `time` and `inclusive` arguments are ignored.

  * Setting `direction` to `B_BEFORE_TIME` indicates that all matching events occurring before the specified `time` should be processed. If `inclusive` is `true`, events occurring at `time` are also processed.

  * Setting `direction` to `B_AT_TIME` processes all matching events scheduled at the specified `time`. The `inclusive` argument is ignored.

  * If `direction` is `B_AFTER_TIME`, all matching events scheduled to occur after the specified `time` are processed. If `inclusive` is `true`, events scheduled to occur at `time` are also processed.

This provides a means for you to scan through the queue and perform a particular act on every node (or all nodes of a certain type, or that occur at certain times). If you want to process all events, you can specify a `time` of `B_INFINITE_TIMEOUT` and a `direction` of `B_BEFORE_TIME`.

The hook function should return a [queue_action](BTimedEventQueue.md#Enums_queue_action "queue_action") value.
    
    
    queue_action MyHook(media_timed_event* event, void* context) {
       if (event->data == 0) {
          /* countdown finished, do something */
          return BTimedEventQueue::B_REMOVE_EVENT;
       }
       event->data--;
       return BTimedEventQueue::B_NO_ACTION;
    }

In this example, the hook function processes the event (and indicates on return that it should be removed from the queue) if the `data` field of the event structure is zero; otherwise `data` is decremented and the event is left alone.

Return Code| Description  
---|---  
`B_OK`| The events were processed.  
`B_ERROR`| An undefined error occurred.  
  
#### EventCount()

`int32 EventCount() const;`

Returns the number of events in the queue.

#### FindFirstMatch()

`const media_timed_event* FindFirstMatch(bigtime_t eventTime,  
time_direction direction,  
bool inclusive = true,  
int32 event = B_ANY_EVENT);`

Searches the event queue for the first event matching the given specifications. The search begins at the time specified by `eventTime`, and progresses in the specified `direction`.

  * Setting `direction` to `B_ALWAYS` indicates that all events of the type indicated by `event` should be scanned. The `eventTime` and `inclusive` arguments are ignored.

  * Setting `direction` to `B_BEFORE_TIME` indicates that all matching events occurring before the specified `eventTime` should be scanned. If `inclusive` is `true`, events occurring at `eventTime` are also scanned.

  * Setting `direction` to `B_AT_TIME` scans all matching events scheduled at the specified `eventTime`. The `inclusive` argument is ignored.

  * If `direction` is `B_AFTER_TIME`, all matching events scheduled to occur after the specified `eventTime` are scanned. If `inclusive` is `true`, events scheduled to occur at `eventTime` are also scanned.

If you want to scan all events, you can specify a time of `B_INFINITE_TIMEOUT` and a direction of `B_BEFORE_TIME`.

If no matching event is found, `NULL` is returned.

####  FirstEvent(), FirstEventTime()

`const media_timed_event* FirstEvent() const;``bigtime_t FirstEventTime() const;`

`FirstEvent()` returns the first event in the queue, without removing it from the queue.

`FirstEventTime()` returns the first event's time, in microseconds.

Return Code| Description  
---|---  
`B_OK`| No error occurred.  
`B_NO_INIT`| The queue hasn't been initialized.  
`B_INFINITE_TIMEOUT`| The queue is empty (`FirstEventTime()` only).  
  
#### FlushEvents()

`status_t FlushEvents(bigtime_t time,  
time_direction direction,  
bool inclusive = true,  
int32 event = B_ANY_EVENT) const;`

Flushes the specified events from the queue. You specify which events to flush by indicating a `time` from which events should be flushed, a `direction` in which to search for events to flush, and the type of events to flush:

  * Setting `direction` to `B_ALWAYS` indicates that all events of the type indicated by `event` should be flushed. The `eventTime` and `inclusive` arguments are ignored.

  * Setting `direction` to `B_BEFORE_TIME` indicates that all matching events occurring before the specified `eventTime` should be flushed. If `inclusive` is `true`, events occurring at `eventTime` are also flushed.

  * Setting `direction` to `B_AT_TIME` flushes all matching events scheduled at the specified `eventTime`. The `inclusive` argument is ignored.

  * If `direction` is `B_AFTER_TIME`, all matching events scheduled to occur after the specified `eventTime` are flushed. If `inclusive` is `true`, events scheduled to occur at `eventTime` are also flushed.

If you want to flush all events, you can specify a time of `B_INFINITE_TIMEOUT` and a direction of `B_BEFORE_TIME`.

Return Code| Description  
---|---  
`B_OK`| The events were flushed.  
`B_ERROR`| An undefined error occurred.  
  
#### HasEvents()

`bool HasEvents();`

Returns `true` if there are events in the queue, otherwise returns `false`.

####  LastEvent(), LastEventTime()

`const media_timed_event* LastEvent() const;``bigtime_t LastEventTime() const;`

`LastEvent()` returns the last event in the queue, without removing it from the queue.

`LastEventTime()` returns the last event's time, in microseconds.

Return Code| Description  
---|---  
`B_OK`| No error occurred.  
`B_NO_INIT`| The queue hasn't been initialized.  
`B_INFINITE_TIMEOUT`| The queue is empty (`LastEventTime()` only).  
  
####  RemoveEvent(), RemoveFirstEvent()

`status_t RemoveEvent(const media_timed_event* event);``status_t RemoveFirstEvent(const media_timed_event* event = NULL);`

`RemoveEvent()` removes the specified event from the queue.

`RemoveFirstEvent()` removes the first event from the queue. If `outEvent` isn't `NULL`, the specified buffer is filled with a copy of the event that's been removed.

Return Code| Description  
---|---  
`B_OK`| The event was removed.  
`B_NO_INIT`| The queue hasn't been initialized.  
  
#### SetCleanupHook()

`void SetCleanupHook(cleanup_hook hook,  
void* context);`

Sets up the cleanup hook function specified by hook to be called for events as they're removed from the queue. The hook will be called only for events with [cleanup_flag](BTimedEventQueue.md#Enums_cleanup_flag "cleanup_flag") values of `B_DELETE` or `B_USER_CLEANUP` or greater.

* * *

### Constants

#### cleanup_flag

Declared in: `media/TimedEventQueue.h`

Constant| Description  
---|---  
`B_NO_CLEANUP`| Don't clean up when the event is removed from the queue.  
`B_RECYCLE_BUFFER`| `BTimedEventQueue` should recycle the buffer when the buffer event is removed from the queue.  
`B_EXPIRE_TIMER`| Call [`TimerExpired()`](BMediaNode.md#BMediaNode_TimerExpired) on the event's data field.  
`B_USER_CLEANUP`| Base value for user-defined cleanup types.  
  
These values define how `BTimedEventQueue` should handle removing events from the queue. If the flag is `B_USER_CLEANUP` or greater, the cleanup hook function is called when the event is removed.

#### event_type

Declared in: `media/TimedEventQueue.h`

Constant| Description  
---|---  
`B_NO_EVENT`| Don't push buffers of this type. It's a return value only.  
`B_ANY_EVENT`| Don't push buffers of this type; it's used in searches only.  
`B_START`| A start event.  
`B_STOP`| A stop event.  
`B_SEEK`| A seek event.  
`B_WARP`| A warp event.  
`B_TIMER`| A timer event.  
`B_HANDLE_BUFFER`| Represents a buffer queued to be handled. The event's pointer is a pointer to a [`BBuffer`](BBuffer.md "BBuffer") object.  
`B_DATA_STATUS`| Represents a data status event.  
`B_HARDWARE`| A hardware event.  
`B_PARAMETER`| The buffer contains changes to parameter values; pass it to [`BControllable::ApplyParameterData()`](BControllable.md#BControllable_ApplyParameterData).  
`B_USER_EVENT`| Base value for user-defined events.  
  
These values describe type types of events that a `BTimedEventQueue` can handle.

#### queue_action

Declared in: `media/TimedEventQueue.h`

Constant| Description  
---|---  
`B_DONE`| End the [`DoForEach()`](BTimedEventQueue.md#BTimedEventQueue_DoForEach "DoForEach\(\)") pass.  
`B_NO_ACTION`| Do nothing for this event.  
`B_REMOVE_EVENT`| Remove the event.  
`B_RESORT_QUEUE`| Sort the queue again to ensure that events are still in the right order.  
  
These queue action values are returned by the hook function used by [`DoForEach()`](BTimedEventQueue.md#BTimedEventQueue_DoForEach "DoForEach\(\)"); these values indicate what [`DoForEach()`](BTimedEventQueue.md#BTimedEventQueue_DoForEach "DoForEach\(\)") should do to the event after the hook has processed it.

#### time_direction

Declared in: `media/TimedEventQueue.h`

Constant| Description  
---|---  
`B_ALWAYS`| Matches events occurring at any time.  
`B_BEFORE_TIME`| Matches events occurring before a specified time.  
`B_AT_TIME`| Matches events occurring at a specified time.  
`B_AFTER_TIME`| Matches events occurring after a specified time.  
  
These values are used to indicate a search direction through the time continuum.

* * *

### Defined Types

#### cleanup_hook

Declared in: `media/TimedEventQueue.h`
    
    
    typedef queue_action (*cleanup_hook)(media_timed_event* event,
                                         void* context);

The cleanup_hook type is used to define a hook function called while removing an event from the queue; it's set by calling [`SetCleanupHook()`](BTimedEventQueue.md#BTimedEventQueue_SetCleanupHook "SetCleanupHook\(\)").

#### for_each_hook

Declared in: `media/TimedEventQueue.h`
    
    
    typedef queue_action (*for_each_hook)(media_timed_event* event,
                                          void* context);

The for_each_hook type is used to define a hook function called by [`DoForEach()`](BTimedEventQueue.md#BTimedEventQueue_DoForEach "DoForEach\(\)").

#### media_timed_event

Declared in: `media/TimedEventQueue.h`
    
    
    struct media_timed_event {
       media_timed_event();
       media_timed_event(bigtime_t inTime, int32 inType);
       media_timed_event(bigtime_t inTime, int32 inType, void* inPointer,
                uint32 inCleanup);
       media_timed_event(bigtime_t inTime, int32 inType,
                void* inPointer, uint32 inCleanup,
                int32 inData, int64 inBigdata,
                char* inUserData, size_t dataSize = 0);
    
       media_timed_event(const media_timed_event& clone);
       void operator=(const media_timed_event& clone);
    
       ~media_timed_event();
    
       bigtime_t event_time;
       int32 type;
       void* pointer;
       uint32 cleanup;
       int32 data;
       int64 bigdata;
       char user_data[64];
       uint32 _reserved_media_timed_event_[8];
    };

Describes a media event:

Field| Description  
---|---  
`event_time`| Indicates the time at which the event is scheduled to occur.  
`type`| Indicates the type of event, as an event_type.  
`pointer`| Is a pointer to event-specific data owned by the event.  
`cleanup`| Is the cleanup_flag value for the event.  
`data`| Is an event-specific 32-bit value.  
`bigdata`| Is an event-specific 64-bit value.  
`user_data`| Is user-defined data.

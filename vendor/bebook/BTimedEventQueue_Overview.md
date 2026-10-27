<!-- System Overview - The Media Kit - BTimedEventQueue -->

## BTimedEventQueue  
  
The [`BTimedEventQueue`](BTimedEventQueue.md "BTimedEventQueue") class provides an easy way to queue a sequence of events within your node. You can use it to queue up start, stop, and seek requests, and to queue up incoming data buffers in preparation for handling them. Each queue element is tagged with the time at which the event should be processed, and functions are provided for locating the next event that should be handled.

Note

![Note](./images/admonitions/Info_32.png)

Although you shouldn't need to subclass [`BTimedEventQueue`](BTimedEventQueue.md "BTimedEventQueue"), there's no reason you can't do it.

* * *

### Cleaning Up After Nodes

Each event has a cleanup flag associated with it that indicates what sort of special action needs to be performed when the event is removed from the queue. If this value is `B_NO_CLEANUP`, nothing is done. If it's `B_RECYCLE`, and the event is a `B_HANDLE_BUFFER` event, [`BTimedEventQueue`](BTimedEventQueue.md "BTimedEventQueue") will automatically recycle the buffer associated with the event.

If the cleanup flag is `B_DELETE` or is `B_USER_CLEANUP` or greater, the cleanup hook function will be called. You can implement and establish a cleanup hook function to handle deleting event data yourself. The hook function is of type cleanup_hook:
    
    
    typedef void (*cleanup_hook)(void *context, bigtime_t time, int32 what,
             void *pointer, uint32 pointerCleanup, int64 data);

You specify the cleanup hook function by calling [`SetCleanupHook()`](BTimedEventQueue.md#BTimedEventQueue_SetCleanupHook "SetCleanupHook\(\)"), like this:
    
    
    SetCleanupHook(MyCleanupFunction, contextPtr);

The `contextPtr` is a pointer that your cleanup hook function uses, and can contain whatever data you require.

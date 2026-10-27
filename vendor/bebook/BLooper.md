<!-- Classes And Methods - The Application Kit - BLooper -->

## BLooper

| Derived From:| [`BHandler`](BHandler.md "BHandler")  
---|---  
Mix-in Classes:| –  
Declared In:| `app/Looper.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BLooper_Overview.md "BLooper")

  * [Constructor and Destructor](BLooper.md#BLooper_ConstuctorDestructor)
  * [Hook Functions](BLooper.md#BLooper_HookFunctions)
  * [Member Functions](BLooper.md#BLooper_MemberFunctions)
  * [Static Functions](BLooper.md#BLooper_StaticFunctions)
  * [Constants](BLooper.md#BLooper_Constants)

  
  
* * *

### Constructor and Destructor

#### BLooper()

`BLooper(const char* name = NULL,  
int32 priority = B_NORMAL_PRIORITY,  
int32 portCapacity = B_LOOPER_PORT_DEFAULT_CAPACITY);` `BLooper(BMessage* archive);`

Assigns the `BLooper` object a `name` and then locks it (by calling [`Lock()`](BLooper.md#BLooper_Lock "Lock\(\), LockWithTimeout\(\), Unlock\(\)")). `priority` is a value that describes the amount of CPU attention the message loop will receive once it starts running, and `portCapacity` is the number of messages the `BLooper` can hold in its "message port" (this is _not_ the message queue, as explained below).

After you construct the `BLooper`, you have to tell it to [`Run()`](BLooper.md#BLooper_Run "Run\(\)"). Because the object is locked, [`Run()`](BLooper.md#BLooper_Run "Run\(\)") can only be called from the thread that constructed the object. It's legal to invoke [`Run()`](BLooper.md#BLooper_Run "Run\(\)") from within a subclass implementation of the constructor.

##### Priority

A set of priority values are defined in `kernel/OS.h`; from lowest to highest, they are:

Constant| Description  
---|---  
`B_LOW_PRIORITY`| For threads running in the background that shouldn't interrupt other threads.  
`B_NORMAL_PRIORITY`| For all ordinary threads, including the main thread.  
`B_DISPLAY_PRIORITY`| For threads associated with objects in the user interface, including window threads.  
`B_URGENT_DISPLAY_PRIORITY`| For interface threads that deserve more attention than ordinary windows.  
`B_REAL_TIME_DISPLAY_PRIORITY`| For threads that animate the on-screen display.  
`B_URGENT_PRIORITY`| For threads performing time-critical computations.  
`B_REAL_TIME_PRIORITY`| For threads controlling real-time processes that need unfettered access to the CPUs.  
  
##### Port Capacity

Messages that are sent to a `BLooper` first show up in a port (as the term is defined by the [Kernel Kit](TheKernelKit.md "The Kernel Kit")), and then are moved to the [`BMessageQueue`](BMessageQueue.md "BMessageQueue"). The capacity of the [`BMessageQueue`](BMessageQueue.md "BMessageQueue") is virtually unlimited; the capacity of the port is not. Although messages are moved from the port to the queue as quickly as possible, the port can fill up. A full port will block subsequent message senders.

The default port capacity (100), should be sufficient for most apps, but you can fiddle with it through the `portCapacity` argument.

#### ~BLooper()

`virtual ~BLooper();`

Frees the message queue and all pending messages and deletes the message loop. [`BHandler`](BHandler.md "BHandler")s that have been added to the `BLooper` are not deleted, but [`BMessageFilter`](BMessageFilter.md "BMessageFilter") objects added as common filters are.

In general, you should never delete your `BLooper` objects: With the exception of the [`BApplication`](BApplication.md "BApplication") object, `BLooper`s are destroyed by the [`Quit()`](BLooper.md#BLooper_Quit "Quit\(\)") function.

Warning

![Warning](./images/admonitions/Stop_32.png)

If you create a `BLooper`-derived class that uses multiple inheritance, make sure the first class your mixin class inherits from is `BLooper`; otherwise, you'll crash when you try to close the window. This happens because of an interaction between the window thread how C++ deletes objects of a multiply-inherited class. In other words:
    
    
    class myClass : public BLooper, public OtherClass {
       ...
    };

is safe, whilst
    
    
    class myClass : public OtherClass, public BLooper {
       ...
    };

is not.

* * *

### Hook Functions

#### DispatchMessage()

`virtual void DispatchMessage(BMessage* message,  
BHandler* target);`

`DispatchMessage()` is the `BLooper`'s central message-processing function. It's called automatically as messages arrive in the looper's queue, one invocation per message. You never invoke `DispatchMessage()` yourself.

The default implementation passes `message` to `handler` by invoking the latter's [`MessageReceived()` ](BHandler.md#BHandler_MessageReceived "MessageReceived\(\)"):
    
    
    target->MessageReceived(message);

The only exception is where `message`.what is [`B_QUIT_REQUESTED`](TheApplicationKit_MessageConstants.md#B_QUIT_REQUESTED "B_QUIT_REQUESTED") and `handler` is the looper itself; in this case, the object invokes its own [`QuitRequested()`](BLooper.md#BLooper_QuitRequested "QuitRequested\(\)") function.

You can override this function to dispatch the messages that your own application defines or recognizes. All unhandled messages should be passed to the base class version, as demonstrated below:
    
    
    void MyLooper::DispatchMessage(BMessage *msg,
                                   BHandler *target)
    {
       switch ( msg->what ) {
       case MY_MESSAGE1:
          ...
          break;
       case MY_MESSAGE2:
          ...
          break;
       default:
          _baseClass_ ::DispatchMessage(msg, target);
          break;
       }
    }

Also, note that you mustn't delete `message`; it's deleted for you..

The system locks the `BLooper` before calling `DispatchMessage()` and keeps it locked for the duration of the function.

#### QuitRequested()

`virtual bool QuitRequested();`

Hook function that's invoked when the `BLooper` receives a [`B_QUIT_REQUESTED`](TheApplicationKit_MessageConstants.md#B_QUIT_REQUESTED "B_QUIT_REQUESTED") message. You never invoke this function directly. Derived classes implement this function to return `true` if it's okay to quit this `BLooper`, and `false` if not. Note that this function does not actually quit the object—the code that handles the [`B_QUIT_REQUESTED`](TheApplicationKit_MessageConstants.md#B_QUIT_REQUESTED "B_QUIT_REQUESTED") message does that.

`BLooper`'s default implementation of `QuitRequested()` always returns `true`.

* * *

### Member Functions

####  AddCommonFilter(), RemoveCommonFilter(), SetCommonFilterList(), CommonFilterList()

`virtual void AddCommonFilter(BMessageFilter* filter);``virtual bool RemoveCommonFilter(BMessageFilter* filter);``virtual void SetCommonFilterList(BList* filters);``BList* CommonFilterList() const;`

These functions manage the `BLooper`'s list of [`BMessageFilter`](BMessageFilter.md "BMessageFilter")s. Message filters are objects that screen in-coming messages. In the case of `BLooper`, each message is passed through all filters in the list before it's passed on to [`DispatchMessage()`](BLooper.md#BLooper_DispatchMessage "DispatchMessage\(\)"). The order of the filters in the list is determinate. See the [`BMessageFilter`](BMessageFilter.md "BMessageFilter") class for details on how message filters work.

`AddCommonFilter()` adds `filter` to the end of the filter list (creating a [`BList`](BList.md "BList") container if necessary).

`RemoveCommonFilter()` removes `filter` from the list, but doesn't free the filter. It returns `true` if successful, and `false` if it can't find the specified filter.

`SetCommonFilterList()` deletes the current filter list and its contents, and replaces it with `filters`. All elements in `filters` must be [`BMessageFilter`](BMessageFilter.md "BMessageFilter") pointers. The `BLooper` takes ownership of all objects in `filters`, as well as `filters` itself. If `filters` is `NULL`, the current list is deleted without a replacement.

`CommonFilterList()` returns a pointer to the current list. You can examine the list but you shouldn't modify or delete it.

Warning

![Warning](./images/admonitions/Stop_32.png)

For all but `CommonFilterList()`, the `BLooper` must be locked.

####  AddHandler(), RemoveHandler(), HandlerAt(), CountHandlers(), IndexOf()

`void AddHandler(BHandler* handler);``bool RemoveHandler(BHandler* handler);``BHandler* HandlerAt(int32 index) const;``int32 CountHandlers() const;``int32 IndexOf(BHandler* handler) const;`

`AddHandler()` adds `handler` to the `BLooper`'s list of [`BHandler`](BHandler.md "BHandler") objects, and `RemoveHandler()` removes it. Only [`BHandler`](BHandler.md "BHandler") that have been added to the list are eligible to respond to the messages the `BLooper` dispatches.

`AddHandler()` fails if the handler already belongs to a `BLooper`; a [`BHandler`](BHandler.md "BHandler") can belong to no more than one `BLooper` at a time. It can change its affiliation from time to time, but must be removed from one `BLooper` before it can be added to another. `RemoveHandler()` returns `true` if it succeeds in removing the [`BHandler`](BHandler.md "BHandler") from the `BLooper`, and `false` if not or if the handler doesn't belong to the `BLooper` in the first place.

`AddHandler()` also calls the handler's [`SetNextHandler()`](BHandler.md#BHandler_SetNextHandler "SetNextHandler\(\), NextHandler\(\)") function to assign it the `BLooper` as its default next handler. `RemoveHandler()` calls the same function to set the handler's next handler to `NULL`.

`HandlerAt()` returns the [`BHandler`](BHandler.md "BHandler") object currently located at `index` in the `BLooper`'s list of eligible handlers, or `NULL` if the index is out of range. Indices begin at 0 and there are no gaps in the list. `CountHandlers()` returns the number of objects currently in the list; the count should always be at least 1, since the list automatically includes the `BLooper` itself. `IndexOf()` returns the index of the specified `handler`, or `B_ERROR` if that object isn't in the list.

For any of these functions to work, the `BLooper` must be locked.

See also: [`BHandler::Looper()`](BHandler.md#BHandler_Looper "Looper\(\)"), [`SetNextHandler()`](BHandler.md#BHandler_SetNextHandler "SetNextHandler\(\), NextHandler\(\)"), [`PostMessage()`](BLooper.md#BLooper_PostMessage "PostMessage\(\)"), the [`BMessenger`](BMessenger.md "BMessenger") class

####  CurrentMessage(), DetachCurrentMessage()

`BMessage* CurrentMessage() const;``BMessage* DetachCurrentMessage();`

The message that a `BLooper` passes to its handler(s) is called the "current message." These functions access the current message; they're meaningless (they return `NULL`) when called from outside the message processing loop.

`CurrentMessage()` simply returns a pointer to the current message without affecting the [`BMessage`](BMessage.md "BMessage") object itself. This is particularly useful to functions that respond to system messages (such as `MouseDown()` and `ScreenChanged()`), but that aren't sent the full [`BMessage`](BMessage.md "BMessage") object that initiated the response.

`DetachCurrentMessage()` removes the current message from the message queue and passes ownership of it to the caller; deleting the message is the caller's responsibility. This is useful if you want to delay the response to the message without tying up the `BLooper`. But be careful—if the message sender is waiting for a synchronous reply, detaching the message and holding on to it will block the sender.

####  Lock(), LockWithTimeout(), Unlock()

`bool Lock();``status_t LockWithTimeout(bigtime_t timeout);``void Unlock();`

`Lock()` locks the `BLooper`. Locks are held within the context of a thread; while a `BLooper` is locked, no other thread can invoke its most important functions ( [`AddHandler()`](BLooper.md#BLooper_AddHandler "AddHandler\(\), RemoveHandler\(\), HandlerAt\(\), CountHandlers\(\), IndexOf\(\)"), [`DispatchMessage()`](BLooper.md#BLooper_DispatchMessage "DispatchMessage\(\)"), etc.)

If the looper is already locked (by some other thread), `Lock()` blocks until the looper is unlocked. To set a timeout for the block, use `LockWithTimeout()` instead. `timeout` is measured in microseconds; if it's 0, the function returns immediately (with or without the lock); if it's `B_INFINITE_TIMEOUT`, it blocks without limit.

`Unlock()` unlocks a locked looper. It can only be called by the thread that currently holds the lock.

Calls to `Lock()`/`LockWithTimeout()` and `Unlock()` can be nested, but locking and unlocking must always be balanced. A single `Unlock()` will not undo a series of `Lock()`'s.

[`BHandler`](BHandler.md "BHandler") defines "smart" versions of these functions that find the handler's looper and then locks it (or unlocks it) in a pseudo-atomic operation (see [`BHandler::LockLooper()` ](BHandler.md#BHandler_LockLooper "LockLooper\(\), LockLooperWithTimeout\(\), UnlockLooper\(\)")). You should always use the [`BHandler`](BHandler.md "BHandler") versions, if possible, rather than retrieving the handler's looper and locking it yourself.

`Lock()` returns `true` if it was able to lock the looper, or if it's already locked by the calling thread, and `false` otherwise.

`LockWithTimeout()` returns:

Return Code| Description  
---|---  
`B_OK`.| The looper was successfully locked.  
`B_TIMED_OUT`.| The call timed out without locking the looper.  
`B_BAD_VALUE`.| This looper was deleted while the function was blocked.  
  
####  LockingThread(), IsLocked(), CountLocks(), CountLockRequests(), Sem()

`thread_id LockingThread() const;``bool IsLocked() const;``int32 CountLocks() const;``int32 CountLockRequests() const;``sem_id Sem() const;`

These functions may be useful while debugging a `BLooper`.

`LockingThread()` returns the thread that currently has the `BLooper` locked, or 1 if the `BLooper` isn't locked.

`IsLocked()` returns `true` if the calling thread currently has the `BLooper` locked (if it's the locking thread) and `false` if not (if some other thread is the locking thread or the `BLooper` isn't locked).

`CountLocks()` returns the number of times the locking thread has locked the `BLooper`the number of [`Lock()`](BLooper.md#BLooper_Lock "Lock\(\), LockWithTimeout\(\), Unlock\(\)") (or [`LockWithTimeout()`](BLooper.md#BLooper_Lock "Lock\(\), LockWithTimeout\(\), Unlock\(\)")) calls that have not yet been balanced by matching [`Unlock()`](BLooper.md#BLooper_Lock "Lock\(\), LockWithTimeout\(\), Unlock\(\)") calls.

`CountLockRequests()` returns the number of threads currently trying to lock the `BLooper`. The count includes the thread that currently has the lock plus all threads currently waiting to acquire it.

`Sem()` returns the sem_id for the semaphore that the `BLooper` uses to implement the locking mechanism.

See also: [`Lock()`](BLooper.md#BLooper_Lock "Lock\(\), LockWithTimeout\(\), Unlock\(\)")

#### MessageReceived()

`virtual void MessageReceived(BMessage* message);`

Simply calls the inherited function. For the current release, the `BLooper` implementation of this function does nothing of importance.

See also: [`BHandler::MessageReceived()`](BHandler.md#BHandler_MessageReceived "MessageReceived\(\)")

#### MessageQueue()

`BMessageQueue* MessageQueue() const;`

Returns the queue that holds messages delivered to the `BLooper`'s thread. You rarely need to examine the message queue directly; it's made available so you can cheat fate by looking ahead.

See also: the [`BMessageQueue`](BMessageQueue.md "BMessageQueue") class

#### PostMessage()

`status_t PostMessage(BMessage* message);` `status_t PostMessage(uint32 command);` `status_t PostMessage(BMessage* message,  
BHandler* handler,  
BHandler* replyHandler = NULL);` `status_t PostMessage(uint32 command,  
BHandler* handler,  
BHandler* replyHandler = NULL);`

`PostMessage()` is similar to [`BMessenger::SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)"). The [`BMessenger`](BMessenger.md "BMessenger") version is preferred (it's a bit safer than `PostMessage()`).

Places a message at the far end of the `BLooper`'s message queue. The message will be processed by [`DispatchMessage()` ](BLooper.md#BLooper_DispatchMessage "DispatchMessage\(\)") when it comes to the head of the queue.

The message can be a full [`BMessage`](BMessage.md "BMessage") object (`message`), or just a command constant (`command`). In the former case, the message is copied and the caller retains ownership of the argument, which can be deleted as soon as `PostMessage()` returns. In the latter case, a [`BMessage`](BMessage.md "BMessage") is created (and deleted) for you.

`handler` is the designated handler for the message, and must be part of this `BLooper`'s handler chain. If handler is (literally) `NULL`, the designated handler is the `BLooper`'s preferred handler at the time [`DispatchMessage()` ](BLooper.md#BLooper_DispatchMessage "DispatchMessage\(\)") is called. In the versions of `PostMessage()` that don't have a handler argument, the designated handler is the `BLooper` object itself.

Replies to the message are delivered to `replyHandler`. If a `replyHandler` isn't specified, replies are sent to `be_app_messenger`.

A `BLooper` should never post a message to itself from within its own message loop thread.

Return Code| Description  
---|---  
`B_OK`.| The message was successfully posted.  
`B_MISMATCHED_VALUES`.| `handler` doesn't belong to this `BLooper`.  
Other errors.| See the return values for [`BMessenger::SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)").  
  
#### Quit()

`virtual void Quit();`

Shuts down the message loop (if it's running), and deletes the `BLooper`. The object must be locked.

When `Quit()` is called from the `BLooper`'s thread, the message loop is immediately stopped and any messages in the message queue are deleted (without being processed). Note that, in this case, `Quit()` doesn't return since the calling thread is dead.

When called from another thread, `Quit()` waits until all messages currently in the queue have been handled before it kills the message loop. It returns after the `BLooper` has been deleted.

If you're quitting a `BLooper` from some other thread, you should send the object a [`B_QUIT_REQUESTED`](TheApplicationKit_MessageConstants.md#B_QUIT_REQUESTED "B_QUIT_REQUESTED") message rather than calling `Quit()` directly.

#### Run()

`virtual thread_id Run();`

Spawns the message loop thread and starts it running. `Run()` expects the `BLooper` to be locked (once only!) when it's called; it unlocks the object before it returns. Keep in mind that a `BLooper` is locked when it's constructed.

Caution

![Caution](./images/admonitions/Warning_32.png)

Calling `Run()` on a `BLooper` that's already running will dump you into the debugger.

Return Code| Description  
---|---  
Positive values.| The thread was successfully spawned and started; this is the thread_id for the thread.  
Thread errors.| See [`spawn_thread()`](TheKernelKit_ThreadsAndTeams.md#spawn_thread "spawn_thread\(\)") and [`resume_thread()`](TheKernelKit_ThreadsAndTeams.md#resume_thread "resume_thread\(\)").  
Port errors.| See [`create_port()`](TheKernelKit_Ports.md#create_port "create_port\(\)").  
  
####  SetPreferredHandler(), PreferredHandler()

`void SetPreferredHandler(BHandler* handler) const;``BHandler* PreferredHandler();`

These functions set and return the `BLooper`'s preferred handler—the [`BHandler`](BHandler.md "BHandler") object that should handle messages not specifically targetted to another [`BHandler`](BHandler.md "BHandler").

To designate the current preferred handler, whatever object that may be, as the target of a message, pass `NULL` for the target handler to [`PostMessage()`](BLooper.md#BLooper_PostMessage "PostMessage\(\)") or to the [`BMessenger constructor`](BMessenger.md#BMessenger_Constructor "BMessenger\(\)").

Posting or sending messages to the preferred handler can be useful. For example, in the [Interface Kit](TheInterfaceKit.md "The Interface Kit"), [`BWindow`](BWindow.md "BWindow") objects name the current focus view as the preferred handler. This makes it possible for other objects, such as [`BMenuItem`](BMenuItem.md "BMenuItem")s and [`BButtons`](BButton.md "BButton"), to target messages to the [`BView`](BView.md "BView") that's currently in focus, without knowing what view that might be. For example, by posting its messages to the window's preferred handler, a Cut menu item can make sure that it always acts on whatever view contains the current selection. See the chapter on the [Interface Kit](TheInterfaceKit.md "The Interface Kit") for information on windows, views, and the role of the focus view.

By default, `BLooper`s don't have a preferred handler; until one is set, `PreferredHandler()` returns `NULL`. Note however, that messages targeted to the preferred handler are dispatched to the `BLooper` whenever the preferred handler is `NULL`. In other words, the `BLooper` acts as default preferred handler, even though the default is formally `NULL`.

See also: [`BInvoker::SetTarget()`](BInvoker.md#BInvoker_SetTarget "SetTarget\(\), Target\(\), IsTargetLocal\(\), Messenger\(\)"), [`PostMessage()`](BLooper.md#BLooper_PostMessage "PostMessage\(\)")

####  Thread(), Team()

`thread_id Thread() const;``team_id Team() const;`

These functions identify the thread that runs the message loop and the team to which it belongs. `Thread()` returns `B_ERROR` if [`Run()`](BLooper.md#BLooper_Run "Run\(\)") hasn't yet been called to spawn the thread and begin the loop. `Team()` always returns the application's team_id.

* * *

### Static Functions

#### LooperForThread()

`static BLooper* LooperForThread(thread_id thread);`

Returns the `BLooper` object that spawned the specified thread, or `NULL` if the thread doesn't belong to a `BLooper`.

* * *

### Constants

#### B_LOOPER_PORT_DEFAULT_CAPACITY
    
    
    #define B_LOOPER_PORT_DEFAULT_CAPACITY 100

The default capacity of the port that holds incoming messages before they're placed in the `BLooper`'s [`BMessageQueue`](BMessageQueue.md "BMessageQueue"). The capacity is set in the `BLooper` [constructor](BLooper.md#BLooper_Constructor "BLooper\(\)").

<!-- System Overview - The Application Kit - BLooper -->

## BLooper

A [`BLooper`](BLooper.md "BLooper") object creates a "message loop" that receives messages that are sent or posted to the [`BLooper`](BLooper.md "BLooper"). The message loop runs in a separate thread that's spawned (and told to run) when the [`BLooper`](BLooper.md "BLooper") receives a [`Run()`](BLooper.md#BLooper_Run "Run\(\)") call. If you're creating your own [`BLooper`](BLooper.md "BLooper"), you can invoke [`Run()`](BLooper.md#BLooper_Run "Run\(\)") from within the [constructor](BLooper.md#BLooper_Constructor "BLooper\(\)").

You tell the loop to stop by sending the [`BLooper`](BLooper.md "BLooper") a [`B_QUIT_REQUESTED`](TheApplicationKit_MessageConstants.md#B_QUIT_REQUESTED "B_QUIT_REQUESTED") message, which invokes the object's [`Quit()`](BLooper.md#BLooper_Quit "Quit\(\)") function. You can also call [`Quit()`](BLooper.md#BLooper_Quit "Quit\(\)") directly, but you have to [`Lock()`](BLooper.md#BLooper_Lock "Lock\(\), LockWithTimeout\(\), Unlock\(\)") the object first ([`BLooper`](BLooper.md "BLooper") locking is explained later). [`Quit()`](BLooper.md#BLooper_Quit "Quit\(\)") deletes the [`BLooper`](BLooper.md "BLooper") for you.

Note

![Note](./images/admonitions/Info_32.png)

The [`BApplication`](BApplication.md "BApplication") class, the most important [`BLooper`](BLooper.md "BLooper") subclass, bends the above description in one of two ways:

  1. A [`BApplication`](BApplication.md "BApplication") takes over the main thread, it doesn't spawn a new one.

  2. You do have to delete `be_app`; you can't just [`Quit()`](BLooper.md#BLooper_Quit "Quit\(\)") it.

* * *

### Messages and Handlers

You can deliver messages to a [`BLooper`](BLooper.md "BLooper")'s thread by…

  * Posting them directly by calling [`BLooper`](BLooper.md "BLooper")'s [`PostMessage()`](BLooper.md#BLooper_PostMessage "PostMessage\(\)") function.

  * Sending them through [`BMessenger`](BMessenger.md "BMessenger")'s [`SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)") or [`BMessage`](BMessage.md "BMessage")'s [`SendReply()`](BMessage.md#BMessage_SendReply "SendReply\(\)") function.

As messages arrive, they're added to the [`BLooper`](BLooper.md "BLooper")'s [`BMessageQueue`](BMessageQueue.md "BMessageQueue") object. The [`BLooper`](BLooper.md "BLooper") takes messages from the queue in the order that they arrived, and calls [`DispatchMessage()`](BLooper.md#BLooper_DispatchMessage "DispatchMessage\(\)") for each one. [`DispatchMessage()`](BLooper.md#BLooper_DispatchMessage "DispatchMessage\(\)") locks the [`BLooper`](BLooper.md "BLooper") and then hands the message to a [`BHandler`](BHandler.md "BHandler") object by invoking the handler's [`MessageReceived()`](BHandler.md#BHandler_MessageReceived "MessageReceived\(\)") function. But which [`BHandler`](BHandler.md "BHandler") does the [`BLooper`](BLooper.md "BLooper") hand the message to? Here's the path:

  * If an incoming message targets a specific [`BHandler`](BHandler.md "BHandler"), and if that [`BHandler`](BHandler.md "BHandler") is one of the [`BLooper`](BLooper.md "BLooper")'s eligible handlers (as set through the [`AddHandler()`](BLooper.md#BLooper_AddHandler "AddHandler\(\), RemoveHandler\(\), HandlerAt\(\), CountHandlers\(\), IndexOf\(\)") function), the [`BLooper`](BLooper.md "BLooper") uses that [`BHandler`](BHandler.md "BHandler"). (See the [`BMessage`](BMessage.md "BMessage") and [`BMessenger`](BMessenger.md "BMessenger") classes for instructions on how to target a [`BHandler`](BHandler.md "BHandler").)

  * Otherwise it hands the message to its preferred handler, as set through [`SetPreferredHandler()`](BLooper.md#BLooper_SetPreferredHandler "SetPreferredHandler\(\), PreferredHandler\(\)").

  * If no preferred handler is set, the [`BLooper`](BLooper.md "BLooper") itself handles the message (its own implementation of [`MessageReceived()` ](BLooper.md#BLooper_MessageReceived "MessageReceived\(\)") is invoked).

After the handler is finished (when its [`MessageReceived()` ](BLooper.md#BLooper_MessageReceived "MessageReceived\(\)") returns), the [`BMessage`](BMessage.md "BMessage") is automatically deleted and the [`BLooper`](BLooper.md "BLooper") is unlocked.

* * *

### Locking

Access to many [`BLooper`](BLooper.md "BLooper") functions (and some [`BHandler`](BHandler.md "BHandler") functions) is proteced by a lock. To invoke a lock-protected functions (or groups of functions), you must first call [`Lock()`](BLooper.md#BLooper_Lock "Lock\(\), LockWithTimeout\(\), Unlock\(\)"), and then call [`Unlock()`](BLooper.md#BLooper_Lock "Lock\(\), LockWithTimeout\(\), Unlock\(\)") when you're done. The lock is scoped to the calling thread: [`Lock()`](BLooper.md#BLooper_Lock "Lock\(\), LockWithTimeout\(\), Unlock\(\)")/[`Unlock()`](BLooper.md#BLooper_Lock "Lock\(\), LockWithTimeout\(\), Unlock\(\)") calls can be nested within the thread. Keep in mind that each [`Lock()`](BLooper.md#BLooper_Lock "Lock\(\), LockWithTimeout\(\), Unlock\(\)") must balanced by an [`Unlock()`](BLooper.md#BLooper_Lock "Lock\(\), LockWithTimeout\(\), Unlock\(\)").

The [`BLooper`](BLooper.md "BLooper") [constructor](BLooper.md#BLooper_Constructor "BLooper\(\)") automatically locks the object. It's unlocked when [`Run()`](BLooper.md#BLooper_Run "Run\(\)") is invoked. This means that the [`Run()`](BLooper.md#BLooper_Run "Run\(\)") function and any other lock-protected functions that you call before you call [`Run()`](BLooper.md#BLooper_Run "Run\(\)") must be called from the thread that constructed the [`BLooper`](BLooper.md "BLooper").

* * *

### Allocation

Because they delete themselves when told to quit, [`BLooper`](BLooper.md "BLooper")s can't be allocated on the stack; you have to construct them with `new`.

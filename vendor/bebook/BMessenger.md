<!-- Classes And Methods - The Application Kit - BMessenger -->

## BMessenger

| Derived From:| –  
---|---  
Mix-in Classes:| [`BArchivable`](BArchivable.md "BArchivable")  
Declared In:| `app/Messenger.h`  
Library:| `libbe.so`  
Allocation:| Stack or constructor  
[Class Overview](BMessenger_Overview.md "BMessenger")

  * [Constructor and Destructor](BMessenger.md#BMessenger_ConstructorDestructor)
  * [Member Functions](BMessenger.md#BMessenger_MemberFunctions)
  * [Operators](BMessenger.md#BMessenger_Operators)

  
  
* * *

### Constructor and Destructor

#### BMessenger()

`BMessenger(const BHandler* handler,  
const BLooper* looper = NULL,  
status_t* error = NULL);` `BMessenger(const char* signature,  
team_id team = -1,  
status_t* error = NULL);` `BMessenger(const BMessenger& messenger);` `BMessenger();`

Creates a new `BMessenger` and sets its target to a local `looper`/`handler`, to the (running) application identified by `signature` or `team`, or to the target of some other `messenger`.

Parameter| Description  
---|---  
`looper`/`handler`.| To target a looper, supply a `looper` and pass a `NULL` `handler`. When the messenger sends a message, the message will be handled by `looper`'s preferred handler. If you want the message to be sent to a specific handler within a looper, supply a `handler` and pass a `NULL` `looper`. The handler must already be attached to a looper, and can't switch loopers after this `BMessenger` is constructed.  
`signature` or `team`.| If you supply a `signature` but leave `team` as -1, the messenger targets an app with that signature. (The app must already be running; in the case of multiple instances of a running app, the exact instance is indeterminate) If you supply a `team` but no `signature`, you target exactly that `team`, regardless of `signature`. By supplying both a `team` and a `signature`, you can specify a specific instance of an app. In this case, `team` must be an app that has the proper `signature`.  
  
Messages sent to a remote target are received and handled by the remote application's [`BApplication`](BApplication.md "BApplication") object.

The `BMessenger` doesn't own its target.

The constructor places an error code in `error` (if provided).

Return Code| Description  
---|---  
`B_OK`.| The target was properly set.  
`B_BAD_VALUE`.| The application identified by `signature` couldn't be found, or both `handler` and `looper` are invalid.  
`B_BAD_TEAM_ID`.| Invalid `team`.  
`B_MISMATCHED_VALUES`.| `team` isn't a `signature` app, or `handler` is associated with a [`BLooper`](BLooper.md "BLooper") other than `looper`.  
`B_BAD_HANDLER`.| `handler` isn't associated with a [`BLooper`](BLooper.md "BLooper")  
  
#### ~BMessenger()

`~BMessenger();`

Frees the `BMessenger`; the target isn't affected.

* * *

### Member Functions

#### IsValid()

`bool IsValid() const;`

Returns `true` if the target looper, whether local or remote, still exists.

Warning

![Warning](./images/admonitions/Stop_32.png)

This function doesn't tell you whether the looper is actually ready to receive messages, or whether the handler (if it was specified in the constructor) exists. In other words, a valid `BMessenger` is no guarantee that a message will actually get to the target.

####  LockTarget(), LockTargetWithTimeout()

`bool LockTarget() const;``status_t LockTargetWithTimeout(bigtime_t timeout) const;`

Important

![Important](./images/admonitions/Warning_32.png)

These functions apply to local targets only.

These functions attempt to lock the target looper in the manner of the similarly named [`BLooper`](BLooper.md "BLooper") functions (see [`BLooper::Lock()`](BLooper.md#BLooper_Lock "Lock\(\), LockWithTimeout\(\), Unlock\(\)") ). In addition to the error codes reported there, these functions return `false` and `B_BAD_VALUE` (respectively) if the target isn't local, or if the looper is otherwise invalid.

#### SendMessage()

`status_t SendMessage([`BMessage`](BMessage.md "BMessage")* message,  
[`BMessage`](BMessage.md "BMessage")* reply,  
bigtime_t deliveryTimeout = B_INFINITE_TIMEOUT,  
bigtime_t replyTimeout = B_INFINITE_TIMEOUT) const;` `status_t SendMessage([`BMessage`](BMessage.md "BMessage")* message,  
[`BHandler`](BHandler.md "BHandler")* replyHandler = NULL,  
bigtime_t deliveryTimeout = B_INFINITE_TIMEOUT) const;` `status_t SendMessage([`BMessage`](BMessage.md "BMessage")* message,  
[`BMessenger`](BMessenger.md "BMessenger")* replyMessenger,  
bigtime_t deliveryTimeout = B_INFINITE_TIMEOUT) const;` `status_t SendMessage(uint32 command,  
[`BMessage`](BMessage.md "BMessage")* reply) const;` `status_t SendMessage(uint32 command,  
[`BHandler`](BHandler.md "BHandler")* replyHandler = NULL) const;`

Sends a copy of `message` (or a [`BMessage`](BMessage.md "BMessage") based on a `command` constant) to the object's target. The caller retains ownership of `message`. The function doesn't return until the message has been delivered; if you're sending a `message` (as opposed to a `command` constant) you can set a microsecond delivery timeout through `deliveryTimeout`.

The target can respond to the message:

  * If you supply a `reply` [`BMessage`](BMessage.md "BMessage"), the response is synchronous, with an optional timeout (`replyTimeout`) that starts ticking after the original message has been delivered. If the response times out, or the target deletes the original message without responding, the `reply`->`what` is set to `B_NO_REPLY`. The caller is responsible for allocating and freeing `reply`. `message` and `reply` can be the same object.

Warning

![Warning](./images/admonitions/Stop_32.png)

Use caution when requesting a synchronous reply: If you call `SendMessage()` from the target looper's thread, you'll deadlock (or, at best, time out).

  * If you supply a reply target (`replyMessenger` or `replyHandler`), the response is asynchronous, and is sent to the reply target.

  * If you supply neither a reply message nor a reply target, the target's response is sent to `be_app_messenger`.

Return Code| Description  
---|---  
`B_OK`.| The message was delivered (and the synchronous reply was received, if applicable).  
`B_TIMED_OUT`.| `deliveryTimeout` expired; the message never made it to the target.  
`B_WOULD_BLOCK`.| You requested a 0 `deliveryTimeout`, and the target's message queue is full.  
`B_BAD_PORT_ID`.| The messenger's target is invalid, or the reply port was deleted while waiting for a reply (synchronous response requests only).  
`B_NO_MORE_PORTS`.| You asked for a synchronous reply, but there are no more reply ports.  
  
Warning

![Warning](./images/admonitions/Stop_32.png)

If you specified a `handler` when you constructed your `BMessenger`, and if that handler has since changed loopers, `SendMessage()` won't deliver its message, but it doesn't complain (it returns `B_OK`).

####  Target(), IsTargetLocal(), Team()

`[`BHandler`](BHandler.md "BHandler") Target([`BLooper`](BLooper.md "BLooper")** looper) const;``bool IsTargetLocal() const;``inline team_id Team() const;`

`Target()` returns the `BMessenger`'s handler (directly) and looper (by reference in `looper`). This function only works for local targets. If `Target()` returns `NULL`, it can mean one of four things:

  1. The target is remote; `looper` is set to `NULL`.

  2. The `BMessenger` hasn't been initialized; `looper` is set to `NULL`.

  3. The handler is the looper's preferred handler; `looper` will be valid.

  4. The handler has been deleted; `looper` will be valid given that it hasn't been deleted as well.

`IsTargetLocal()` returns `true` if the target is local. `Team()` returns a target's team.

* * *

### Operators

#### = (assignment)

`BMessenger& operator=(const BMessenger& from);`

Sets the left-side `BMessenger`'s target to that of the right-side object.

#### == (equality)

`bool operator==(const BMessenger& other) const;`

Two `BMessenger`s are equal if they have the same target.

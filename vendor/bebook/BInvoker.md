<!-- Classes And Methods - The Application Kit - BInvoker -->

## BInvoker

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `app/Invoker.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BInvoker_Overview.md "BInvoker")

  * [Constructor and Destructor](BInvoker.md#BInvoker_ConstructorDestructor)
  * [Member Functions](BInvoker.md#BInvoker_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BInvoker()

`BInvoker(BMessage* message,  
BMessenger messenger);` `BInvoker(BMessage* message,  
const BHandler* handler,  
const BLooper* looper = NULL);` `BInvoker();`

Initializes the `BInvoker` by setting its message and its messenger.

  * The object's [`BMessage`](BMessage.md "BMessage") is taken directly as `message`—the object is _not_ copied. The `BInvoker` takes over ownership of the [`BMessage`](BMessage.md "BMessage") that you pass in.

  * The object's [`BMessenger`](BMessenger.md "BMessenger") is copied from `messenger`, or initialized with `looper` and `handler`. See the [`BMessenger`](BMessenger.md "BMessenger") class for details on how a [`BMessenger`](BMessenger.md "BMessenger") identifies a target.

If you want a reply handler, you have to call [`SetHandlerForReply()`](BInvoker.md#BInvoker_SetHandlerForReply "SetHandlerForReply\(\), HandlerForReply\(\)") after the constructor returns. You can reset the message and messenger through [`SetMessage()`](BInvoker.md#BInvoker_SetMessage "SetMessage\(\), Message\(\), Command\(\)") and [`SetTarget()`](BInvoker.md#BInvoker_SetTarget "SetTarget\(\), Target\(\), IsTargetLocal\(\), Messenger\(\)").

#### ~BInvoker()

`virtual ~BInvoker();`

Deletes the object's [`BMessage`](BMessage.md "BMessage").

* * *

### Member Functions

####  BeginInvokeNotify(), EndInvokeNotify()

`void BeginInvokeNotify(uint32 kind = B_CONTROL_INVOKED);``void EndInvokeNotify();`

If for some reason you need to implement a method that emulates an [`InvokeNotify()`](BInvoker.md#BInvoker_Invoke "Invoke\(\), InvokeNotify\(\)") call inside an [`Invoke()`](BInvoker.md#BInvoker_Invoke "Invoke\(\), InvokeNotify\(\)") implementation, you should wrap the invocation code in these functions. They set up and tear down an [`InvokeNotify()`](BInvoker.md#BInvoker_Invoke "Invoke\(\), InvokeNotify\(\)") context.

####  Invoke(), InvokeNotify()

`virtual status_t Invoke(BMessage* message = NULL);``status_t InvokeNotify(BMessage* message,  
uint32 kind = B_CONTROL_INVOKED);`

`Invoke()` tells the `BInvoker`'s messenger to send a message. If `message` is non-`NULL`, that message is sent, otherwise the object sends its default message (i.e. the [`BMessage`](BMessage.md "BMessage") that was passed in the constructor or in [`SetMessage()`](BInvoker.md#BInvoker_SetMessage "SetMessage\(\), Message\(\), Command\(\)")). The message is sent asynchronously with no time limit on the reply.

Note

![Note](./images/admonitions/Info_32.png)

Regarding the use of the default message _vs_ the argument, a common practice is to reserve the default message as a template, and pass a fine-tuned copy to `Invoke()`
    
    
    /* Add the current system time to a copy of
       the default message. */
    BMessage copy(invoker.Message());
    copy.AddInt64("when", system_time());
    invoker.Invoke(copy);

The `InvokeNotify()` function sends the `message` to the target, using the notification change code specified by `kind`. If `message` is `NULL`, nothing gets sent to the target, but any watchers of the invoker's handler will receive their expected notifications. By default, the `kind` is `B_CONTROL_INVOKED`, the same kind sent by a straight `Invoke()`.

`Invoke()` doesn't call [`SendNotices()`](BHandler.md#BHandler_SendNotices "SendNotices\(\)") by default; you'll have to implement code to do it yourself. Here's how:
    
    
    status_t BControl::Invoke(BMessage* msg) {
       bool notify = false;
       uint32 kind = InvokeKind(notify);
    
       BMessage clone(kind);
       status_t err = B_BAD_VALUE;
    
       if (!msg && !notify) {
          // If no message is supplied, pull it from the BInvoker.
          // However, ONLY do so if this is not an InvokeNotify()
          // context -- otherwise, this is not the default invocation
          // message, so we don't want it to get in the way here.
          // For example, a control may call InvokeNotify() with their
          // "modification" message... if that message isn't set,
          // we still want to send notification to any watchers, but
          // -don't- want to send a message through the invoker.
          msg = Message();
       }
    
       if (!msg) {
          // If not being watched, there is nothing to do.
          if( !IsWatched() ) return err;
       } else {
          clone = *msg;
       }
    
       clone.AddInt64("when", system_time());
       clone.AddPointer("source", this);
       clone.AddInt32("be:value",fValue);
       clone.AddMessenger(B_NOTIFICATION_SENDER, BMessenger(this));
    
       if( msg ) err = BInvoker::Invoke(&clone);
    
       // Also send invocation to any observers of this handler.
       SendNotices(kind, clone);
    
       return err;
    }

Return Code| Description  
---|---  
`B_OK`.| The message was sent.  
`B_BAD_VALUE`.| No default message, and no message argument.  
Other errors.| Forwarded from [`BMessenger::SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)").  
  
#### InvokeKind()

`uint32 InvokeKind(bool* notify = NULL);`

Returns the kind passed to [`InvokeNotify()`](BInvoker.md#BInvoker_Invoke "Invoke\(\), InvokeNotify\(\)"). This should be called from within your implementation of [`Invoke()`](BInvoker.md#BInvoker_Invoke "Invoke\(\), InvokeNotify\(\)") if you need to determine what kind was specified when [`InvokeNotify()`](BInvoker.md#BInvoker_Invoke "Invoke\(\), InvokeNotify\(\)") was called. If you care whether [`Invoke()`](BInvoker.md#BInvoker_Invoke "Invoke\(\), InvokeNotify\(\)") or [`InvokeNotify()`](BInvoker.md#BInvoker_Invoke "Invoke\(\), InvokeNotify\(\)") was originally called, you can specify a pointer to a bool, `notify`, which is set to `true` if [`InvokeNotify()`](BInvoker.md#BInvoker_Invoke "Invoke\(\), InvokeNotify\(\)") was called, or `false` if [`Invoke()`](BInvoker.md#BInvoker_Invoke "Invoke\(\), InvokeNotify\(\)") was called. This lets you fetch the [`InvokeNotify()`](BInvoker.md#BInvoker_Invoke "Invoke\(\), InvokeNotify\(\)") arguments from your [`Invoke()`](BInvoker.md#BInvoker_Invoke "Invoke\(\), InvokeNotify\(\)") code without breaking compatibility with older applications by adding arguments to [`Invoke()`](BInvoker.md#BInvoker_Invoke "Invoke\(\), InvokeNotify\(\)").

####  SetHandlerForReply(), HandlerForReply()

`virtual status_t SetHandlerForReply(BHandler* replyHandler);``BHandler* HandlerForReply() const;`

`SetHandlerForReply()` sets the [`BHandler`](BHandler.md "BHandler") object that handles replies that are sent back by the target. By default (or if `replyHandler` is `NULL`), replies are sent to the [`BApplication`](BApplication.md "BApplication") object.

`HandlerForReply()` returns the object set through `SetHandlerForReply()`. If the reply handler isn't set, this function returns `NULL`, it doesn't return [`be_app`](BApplication.md#BApplication_be_app "be_app") (even though [`be_app`](BApplication.md#BApplication_be_app "be_app") will be handling the reply).

Return Code| Description  
---|---  
`B_OK`| `SetHandlerForReply()` always returns `B_OK` it doesn't check for validity.  
  
####  SetMessage(), Message(), Command()

`virtual status_t SetMessage(BMessage* message);``BMessage* Message() const;``uint32 Command() const;`

`SetMessage()` sets the `BInvoker`'s default message to point to `message` (the message is _not_ copied). The previous default message (if any) is deleted; a `NULL` `message` deletes the previous message without setting a new one. The `BInvoker` owns the [`BMessage`](BMessage.md "BMessage") that you pass in; you mustn't delete it yourself.

`Message()` returns a pointer to the default message, and `Command()` returns its `what` data member. Lacking a default message, the functions return `NULL`.

Return Code| Description  
---|---  
`B_OK`| `SetMessage()` always returns `B_OK`.  
  
####  SetTarget(), Target(), IsTargetLocal(), Messenger()

`virtual status_t SetTarget(BMessenger messenger);` `virtual status_t SetTarget(const BHandler* handler,  
const BLooper* looper = NULL);`

`BHandler* Target(BLooper** looper = NULL) const;``bool IsTargetLocal() const;``BMessenger Messenger() const;`

These functions set and query the `BInvoker`'s target. This is the [`BHandler`](BHandler.md "BHandler") to which the object sends a message when [`Invoke()`](BInvoker.md#BInvoker_Invoke "Invoke\(\), InvokeNotify\(\)") is called. The target is represented by a [`BMessenger`](BMessenger.md "BMessenger") object; you can set the [`BMessenger`](BMessenger.md "BMessenger") as a copy of `messenger`, or initialize it with `looper` and `handler`. See the [`BMessenger`](BMessenger.md "BMessenger") class for details on how a [`BMessenger`](BMessenger.md "BMessenger") identifies a target.

`Target()` returns the [`BHandler`](BHandler.md "BHandler") that's targeted by the object's messenger. If `looper` is non-`NULL`, the [`BLooper`](BLooper.md "BLooper") that owns the [`BHandler`](BHandler.md "BHandler") is returned by reference. If the target was set as a looper's preferred handler (i.e. SetTarget(`NULL`, looper)), or if the target hasn't been set yet, `Target()` returns `NULL`. The function returns `NULL` for both objects if the target is remote.

`IsTargetLocal()` returns `true` if the target lives within the `BInvoker`'s application, and `false` if it belongs to some other application.

`Messenger()` returns a copy of the [`BMessenger`](BMessenger.md "BMessenger") object the `BInvoker` uses to send messages. If a target hasn't been set yet, the return will be invalid.

Return Code| Description  
---|---  
`B_OK`.| The target was successfully set.  
`B_BAD_VALUE`.| The proposed `handler` doesn't belong to a [`BLooper`](BLooper.md "BLooper").  
`B_MISMATCHED_VALUES`.| `handler` doesn't belong to `looper`.  
  
Warning

![Warning](./images/admonitions/Stop_32.png)

`SetTarget()` doesn't detect invalid [`BLooper`](BLooper.md "BLooper")s and [`BMessenger`](BMessenger.md "BMessenger")s.

#### SetTimeout(), Timeout()

`status_t SetTimeout(bigtime_t timeout);``bigtime_t Timeout() const;`

`SetTimeout()` sets the timeout that will be used when sending the invocation message to the invoker's target. By default this is `B_INFINITE_TIMEOUT`.

`Timeout()` returns the current setting for this value.

Return Code| Description  
---|---  
`B_OK`.| No error.

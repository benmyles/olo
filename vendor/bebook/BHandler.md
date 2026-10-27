<!-- Classes And Methods - The Application Kit - BHandler -->

## BHandler

| Derived From:| –  
---|---  
Mix-in Classes:| [`BArchivable`](BArchivable.md "BArchivable")  
Declared In:| `app/Handler.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BHandler_Overview.md "BHandler")

  * [Constructor and Destructor](BHandler.md#BHandler_ConstructorDestructor)
  * [Hook Functions](BHandler.md#BHandler_HookFunctions)
  * [Member Functions](BHandler.md#BHandler_MemberFunctions)
  * [Static Functions](BHandler.md#BHandler_StaticFunctions)
  * [Archived Fields](BHandler.md#BHandler_ArchivedFields)
  * [Scripting Suites and Properties](BHandler.md#BHandler_ScriptingSuites)

  
  
* * *

### Constructor and Destructor

#### BHandler()

`BHandler(const char* name = NULL);` `BHandler(BMessage* archive);`

Initializes the `BHandler` by assigning it a `name` and registering it with the messaging system. `BHandler`s can also be reconstructed from a [`BMessage`](BMessage.md "BMessage") `archive`.

#### ~BHandler()

`virtual ~BHandler();`

Deletes any [`BMessageFilter`](BMessageFilter.md "BMessageFilter")s assigned to the `BHandler`.

* * *

### Hook Functions

#### MessageReceived()

`virtual void MessageReceived(BMessage* message);`

Implemented by derived classes to respond to messages that are received by the `BHandler`. The default implementation of this function responds only to scripting requests. It passes all other messages to the next handler by calling that object's version of `MessageReceived()`.

A typical `MessageReceived()` implementation distinguishes between messages by looking at its command constant (i.e. the `what` field). For example:
    
    
    void MyHandler::MessageReceived([BMessage](BMessage.md "BMessage")* message)
    {
       switch ( message->what ) {
       case COMMAND_ONE:
          HandleCommandOne();
          break;
    
       case COMMAND_TWO:
          HandleCommandTwo();
          break;
       ...
       default:
          _baseClass_ ::MessageReceived(message);![1](./images/callouts/1.png)
          break;
       ...
       }
    }

![1](./images/callouts/1.png) | It's essential that all unhandled messages are passed to the base class implementation of `MessageReceived()`, as shown here. The handler chain model depends on it.  
---|---  
  
If the message comes to the end of the line—if it's not recognized and there is no next handler—the `BHandler` version of this function sends a [`B_MESSAGE_NOT_UNDERSTOOD`](TheApplicationKit_MessageConstants.md#B_MESSAGE_NOT_UNDERSTOOD "B_MESSAGE_NOT_UNDERSTOOD") reply to notify the message source.

Important

![Important](./images/admonitions/Warning_32.png)

Do not delete the argument message when you're done with. It doesn't belong to you.

* * *

### Member Functions

#### Archive()

See [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)")

#### GetSupportedSuites()

`virtual status_t GetSupportedSuites(BMessage* message);`

Implemented by derived classes to report the suites of messages and specifiers they understand. This function is called in response to either a `B_GET_PROPERTIES` scripting message for the "Suites" property or a `B_GET_SUPPORTED_SUITES` message.

Each derived class should add the names of the suites it implements to the `suites` array of `message`. Each item in the array is a MIME string with the "suite" supertype. In addition, the class should add corresponding flattened [`BPropertyInfo`](BPropertyInfo.md "BPropertyInfo") objects in the `messages` array. A typical implementation of `GetSupportedSuites()` looks like:
    
    
    status_t MyHandler::GetSupportedSuites(BMessage* message)
    {
       message->AddString("suites", "suite/vnd.Me-my_handler"));
       BPropertyInfo prop_info(prop_list);
       message->AddFlat("messages", prop_info);
    
       return BHandler::GetSupportedSuites(message);
    }

The value returned by `GetSupportedSuites()` is added to `message` in the int32 `be:error` field.

`BHandler`'s version of this function adds the universal suite "suite/vnd.Be-handler" to `message` then returns `B_OK`.

####  LockLooper(), LockLooperWithTimeout(), UnlockLooper()

`bool LockLooper();``status_t LockLooperWithTimeout(bigtime_t timeout);``void UnlockLooper();`

These are "smart" versions of [`BLooper`](BLooper.md "BLooper")'s locking functions ([`BLooper::Lock()`](BLooper.md#BLooper_Lock "Lock\(\), LockWithTimeout\(\), Unlock\(\)") et. al.). The difference between the versions is that these functions retrieve the handler's looper and lock it (or unlock it) in a pseudo-atomic operation, thus avoiding a race condition. Anytime you're tempted to write code such as this:
    
    
    /* DON'T DO THIS */
    if (myHandler->Looper()->Lock()) {
       ...
       myHandler->Looper()->Unlock();
    }

Don't do it. Instead, do this:
    
    
    /* DO THIS INSTEAD */
    if (myHandler->LockLooper()) {
       ...
       myHandler->UnlockLooper();
    }

Except for an additional return value in `LockLooperWithTimeout()`, these functions are identical to their [`BLooper`](BLooper.md "BLooper") analogues. See [`BLooper::Lock()`](BLooper.md#BLooper_Lock "Lock\(\), LockWithTimeout\(\), Unlock\(\)") for details.

`LockLooper()` returns `true` if it was able to lock the looper, or if it's already locked by the calling thread, and `false` otherwise. If the handler changes loopers during the call, `false` is returned.

`LockLooperWithTimeout()` returns:

Return Code| Description  
---|---  
`B_OK`.| The looper was successfully locked.  
`B_TIMED_OUT`.| The call timed out without locking the looper.  
`B_BAD_VALUE`.| This handler's looper is invalid.  
`B_MISMATCHED_VALUES`.| The handler switched loopers during the call.  
  
#### Looper()

`BLooper* Looper() const;`

Returns the [`BLooper`](BLooper.md "BLooper") object that the `BHandler` has been added to. The function returns `NULL` if the object hasn't been added to a [`BLooper`](BLooper.md "BLooper"). A `BHandler` can be associated with only one [`BLooper`](BLooper.md "BLooper") at a time.

Note that a [`BLooper`](BLooper.md "BLooper") object automatically adds itself (as a handler) to itself (as a looper), and a [`BWindow`](BWindow.md "BWindow") automatically adds its child views. To explicitly add a handler to a looper, you call [`BLooper::AddHandler()`](BLooper.md#BLooper_AddHandler "AddHandler\(\), RemoveHandler\(\), HandlerAt\(\), CountHandlers\(\), IndexOf\(\)").

#### ResolveSpecifier()

`virtual BHandler* ResolveSpecifier(BMessage* message,  
int32 index,  
BMessage* specifier,  
int32 what,  
const char* property);`

Implemented by derived classes to determine the proper handler for a scripting message. The message is targeted to the `BHandler`, but the specifiers may indicate that it should be assigned to another object. It's the job of `ResolveSpecifier()` to examine the current specifier (or more, if necessary) and return the object that should either handle the message or look at the next specifier. This function is called before the message is dispatched and before any filtering functions are called.

The first argument, `message`, points to the scripting message under consideration. The current specifier is passed in `specifier`; it will be at index `index` in the specifier array of message. Finally, `what` contains the what data member of `specifier` while `property` contains the name of the targetted property.

`ResolveSpecifier()` returns a pointer to the next `BHandler` that should look at the message. To identify the `BHandler`, it tries these methods, in order:

##### Method 1:

If the `specifier` identifies a `BHandler` belonging to another [`BLooper`](BLooper.md "BLooper"), it should send the `message` to the [`BLooper`](BLooper.md "BLooper") and return `NULL`. The message will be handled in the message loop of the other [`BLooper`](BLooper.md "BLooper"); it won't be further processed in this one. For example, a `BHandler` that kept a list of proxies might use code like the following:
    
    
    if ( (strcmp(property, "Proxy") == 0)
          && (what == B_INDEX_SPECIFIER) ) {
       int32 i;
       if ( specifier->FindInt32("index", i) == B_OK ) {
          MyProxy* proxy = (MyProxy*)proxyList->ItemAt(i);
          if ( proxy ) {
                message->PopSpecifier();
                if ( proxy->Looper() != Looper() ) {
                   proxy->Looper()->PostMessage(message, proxy);
                   return NULL;
                }
          }
          . . .
       }
       . . .
    }

Since this function resolved the specifier at `index`, it calls [`PopSpecifier()`](BMessage.md#BMessage_PopSpecifier) to decrement the index before forwarding the message. Otherwise, the next handler would try to resolve the same specifier.

##### Method 2:

If the `specifier` picks out another `BHandler` object belonging to the same [`BLooper`](BLooper.md "BLooper"), `ResolveSpecifier()` can return that `BHandler`. For example:
    
    
    if ( proxy ) {
       message->PopSpecifier();
       if ( proxy->Looper() != Looper() ) {
          proxy->Looper()->PostMessage(message, proxy);
          return NULL;
       }
       else {
          return proxy;
       }
    }

This, in effect, puts the returned object in the `BHandler`'s place as the designated handler for the message. The [`BLooper`](BLooper.md "BLooper") will give the returned handler a chance to respond to the message or resolve the next specifier.

Again, [`PopSpecifier()`](BMessage.md#BMessage_PopSpecifier) should be called so that an attempt isn't made to resolve the same specifier twice.

##### Method 3:

If it can resolve all remaining specifiers and recognizes the message as one that the `BHandler` itself can handle, it should return the `BHandler` (`this`). For example:
    
    
    if ( (strcmp(property, "Value") == 0) &&
         (message->what == B_GET_PROPERTY) )
       return this;

This confirms the `BHandler` as the message target. `ResolveSpecifier()` won't be called again, so it's not necessary to call [`PopSpecifier()`](BMessage.md#BMessage_PopSpecifier) before returning.

##### Method 4:

If it doesn't recognize the property or can't resolve the specifier, it should call (and return the value returned by) the inherited version of `ResolveSpecifier()`.

##### Examples

The [`BApplication`](BApplication.md "BApplication") object takes the first path when it resolves a specifier for a "Window" property; it sends the message to the specified [`BWindow`](BWindow.md "BWindow") and returns `NULL`. A [`BWindow`](BWindow.md "BWindow") follows the second path when it resolves a specifier for a "View" property; it returns the specified [`BView`](BView.md "BView"). Thus, a message initially targeted to the [`BApplication`](BApplication.md "BApplication") object can find its way to a [`BView`](BView.md "BView").

`BHandler`'s version of `ResolveSpecifier()` recognizes a `B_GET_PROPERTY` `message` with a direct `specifier` requesting a "Suite" for the supported suites, "Messenger" for the `BHandler`, or the `BHandler`'s "InternalName" (the same name that its `Name()` function returns). In all three cases, it assigns the `BHandler`(`this`) as the object responsible for the message.

For all other specifiers and messages, it sends a `B_MESSAGE_NOT_UNDERSTOOD` reply and returns `NULL`. The reply message has an `error` field with `B_SCRIPT_SYNTAX` as the error and a `message` field with a longer textual explanation of the error.

####  SetFilterList(), FilterList(), AddFilter(), RemoveFilter()

`virtual void SetFilterList(BList* list);``BList* FilterList() const;``virtual void AddFilter(BMessageFilter* filter);``virtual bool RemoveFilter(BMessageFilter* filter);`

These functions manage a list of [`BMessageFilter`](BMessageFilter.md "BMessageFilter") objects associated with the `BHandler`.

`SetFilterList()` assigns the `BHandler` a new `list` of filters; the list must contain pointers to instances of the [`BMessageFilter`](BMessageFilter.md "BMessageFilter") class or to instances of classes that derive from [`BMessageFilter`](BMessageFilter.md "BMessageFilter"). The new list replaces any list of filters previously assigned. All objects in the previous list are deleted, as is the [`BList`](BList.md "BList") that contains them. If list is `NULL`, the current list is removed without a replacement. `FilterList()` returns the current list of filters.

`AddFilter()` adds a `filter` to the end of the `BHandler`'s list of filters. It creates the [`BList`](BList.md "BList") object if it doesn't already exist. By default, BHandlers don't maintain a [`BList`](BList.md "BList") of filters until one is assigned or the first [`BMessageFilter`](BMessageFilter.md "BMessageFilter") is added. `RemoveFilter()` removes a `filter` from the list without deleting it. It returns `true` if successful, and `false` if it can't find the specified filter in the list (or the list doesn't exist). It leaves the [`BList`](BList.md "BList") in place even after removing the last filter.

For `SetFilterList()`, `AddFilter()` and `RemoveFilter()` to work, the `BHandler` must be assigned to a [`BLooper`](BLooper.md "BLooper") object and the [`BLooper`](BLooper.md "BLooper") must be locked.

See also: [`BLooper::SetCommonFilterList()`](BLooper.md#BLooper_SetCommonFilterList), [`BLooper::Lock()`](BLooper.md#BLooper_Lock "Lock\(\), LockWithTimeout\(\), Unlock\(\)"), the [`BMessageFilter`](BMessageFilter.md "BMessageFilter") class

####  SetName(), Name()

`void SetName(const char* string);``const char* Name() const;`

These functions set and return the name that identifies the `BHandler`. The name is originally set by the constructor. `SetName()` assigns the `BHandler` a new name, and `Name()` returns the current name. The string returned by `Name()` belongs to the `BHandler` object; it shouldn't be altered or freed.

See also: The `BHandler` [constructor](BHandler.md#BHandler_Constructor "BHandler\(\)"), [`BView::FindView()`](BView.md#BView_FindView "FindView\(\)") in th [Interface Kit](TheInterfaceKit.md "The Interface Kit")

####  SetNextHandler(), NextHandler()

`void SetNextHandler(BHandler* handler);``BHandler* NextHandler() const;`

`SetNextHandler()` reorders the objects in the handler chain so that `handler` follows this `BHandler`. This `BHandler` and `handler` must already be part of the same chain, and the [`BLooper`](BLooper.md "BLooper") they belong to must be locked. The order of objects in the handler chain affects the way in-coming messages are handled (as explained in "[Inheritance and the Handler Chain](TheApplicationKit_Messaging.md#TheApplicationKit_Messaging_Inheritance "Inheritance and the Handler Chain")". By default handlers are placed in the order that they're added (through [`BLooper::AddHandler()`](BLooper.md#BLooper_AddHandler "AddHandler\(\), RemoveHandler\(\), HandlerAt\(\), CountHandlers\(\), IndexOf\(\)")).

`NextHandler()` returns this object's next handler. If this object is at the end of the chain, it returns `NULL`.

#### SendNotices()

`virtual void SendNotices(uint32 what,  
const [`BMessage`](BMessage.md "BMessage")* msg = 0);`

Sends a [`B_OBSERVER_NOTICE_CHANGE`](TheApplicationKit_MessageConstants.md#B_OBSERVER_NOTICE_CHANGE "B_OBSERVER_NOTICE_CHANGE") message to each `BHandler` object (or "observer") that's observing this handler (the "notifier"). To observe a notifier, the observer calls [`StartWatching()`](BHandler.md#BHandler_StartWatching "StartWatching\(\), StartWatchingAll\(\), StopWatching\(\), StopWatchingAll\(\)"). The `what` argument describes the type of change that's prompting this notification; only those observers that have registered to be notified about what (or that are watching all changes) are sent notifications.

The [`B_OBSERVER_NOTICE_CHANGE`](TheApplicationKit_MessageConstants.md#B_OBSERVER_NOTICE_CHANGE "B_OBSERVER_NOTICE_CHANGE") messages that are sent are copied from `msg` with the what argument added as the `be:old_what` field. Note that `msg`'s original `what` field is clobbered.

####  StartWatching(), StartWatchingAll(), StopWatching(), StopWatchingAll()

`status_t StartWatching(BMessenger watcher,  
uint32 what);` `status_t StartWatching(BHandler* watcher,  
uint32 what);`

`status_t StartWatchingAll(BMessenger watcher);` `status_t StartWatchingAll(BHandler* watcher);`

`status_t StopWatching(BMessenger watcher,  
uint32 what);` `status_t StopWatching(BHandler* watcher,  
uint32 what);`

`status_t StopWatchingAll(BMessenger watcher);` `status_t StopWatchingAll(BHandler* watcher);`

The `BHandler` class provides the concept of a **notifier**. Notifiers maintain one or more states that other entities might want to monitor changes to. These states are identified by a 32-bit `what` code. Another entity a `BHandler` or a [`BMessenger`](BMessenger.md "BMessenger") can watch for changes notifiers' states. These are called **observers**.

`StartWatching()` registers the [`BMessenger`](BMessenger.md "BMessenger") or `BHandler` specified by `watcher` to be notified whenever the state specified by `what` changes. `StartWatchingAll()` registers the specified [`BMessenger`](BMessenger.md "BMessenger") or `BHandler` to be notified when any of the notifer's states change.

`StartWatching()` works by sending a message to the `BHandler` you want to observe, with a [`BMessenger`](BMessenger.md "BMessenger") back to the observer, so both must be attached to a looper at the time `StartWatching()` is called.

Note

![Note](./images/admonitions/Info_32.png)

The forms of `StartWatching()` and `StartWatchingAll()` that accept a `BHandler` can be used to observe a handler that's not yet attached to a looper. However, these only work if the observer and notifier are both in the same looper.

`StopWatching()` ceases monitoring of the state `what`. `StopWatchingAll()`, by some odd coincidence, stops all monitoring by the `BHandler` or [`BMessenger`](BMessenger.md "BMessenger") specified by `watcher`.

Return Code| Description  
---|---  
`B_OK`.| No error.  
`B_BAD_HANDLER`.| The specified `BHandler` isn't valid.  
  
* * *

### Static Functions

#### Instantiate()

See [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)")

* * *

### Archived Fields

Field| Type code| Description  
---|---|---  
`_name`| `B_STRING_TYPE`| The object's name (see [`SetName()`](BHandler.md#BHandler_SetName "SetName\(\), Name\(\)")).  
  
`BHandler` records its own name.

* * *

### Scripting Suites and Properties

#### Suite: "suite/vnd.Be-handler"

##### "InternalName"

Message| Specifiers| Reply Type  
---|---|---  
`B_GET_PROPERTY`| `B_DIRECT_SPECIFIER`| `B_STRING_TYPE`  
  
Returns the handler's name.

##### "Messenger"

Message| Specifiers| Reply Type  
---|---|---  
`B_GET_PROPERTY`| `B_DIRECT_SPECIFIER`| `B_MESSENGER_TYPE`  
  
Returns a [`BMessenger`](BMessenger.md "BMessenger") for the handler.

##### "Suites"

Message| Specifiers| Reply Type  
---|---|---  
`B_GET_PROPERTY`| `B_DIRECT_SPECIFIER`| B_STRING_TYPE array  
  
Returns an array of suites that the target supports, identified by name (e.g. "suite/vnd.Be-handler").

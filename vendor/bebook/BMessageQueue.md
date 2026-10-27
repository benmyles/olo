<!-- Classes And Methods - The Application Kit - BMessageQueue -->

## BMessageQueue

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `app/MessageQueue.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BMessageQueue_Overview.md "BMessageQueue")

  * [Constructor and Destructor](BMessageQueue.md#BMessageQueue_ConstructorDestructor)
  * [Member Functions](BMessageQueue.md#BMessageQueue_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BMessageQueue

`BMessageQueue();`

Creates an empty `BMessageQueue` object.

#### ~BMessageQueue()

`virtual ~BMessageQueue();`

Deletes all the objects in the queue and all the data structures used to manage the queue.

* * *

### Member Functions

####  AddMessage(), RemoveMessage()

`void AddMessage(BMessage* message);``void RemoveMessage(BMessage* message);`

`AddMessage()` adds message to the far end of the queue. `RemoveMessage()` removes a particular message from the queue and deletes it.

####  CountMessages(), IsEmpty()

`int32 CountMessages() const;``bool IsEmpty() const;`

`CountMessages()` returns the number of messages currently in the queue.

`IsEmpty()` returns `true` if the object doesn't contain any messages, and `false` otherwise.

#### FindMessage()

`BMessage* FindMessage(int32 index) const;` `BMessage* FindMessage(uint32 what,  
int32 index = 0) const;`

`FindMessage()` returns a pointer to the `index`'th [`BMessage`](BMessage.md "BMessage") in the queue, where index 0 signifies the message that's been in the queue the longest. The second version lets you specify a `what` field value; in this case, only messages that match the `what` argument are counted. If no message matches the criteria, the functions return `NULL`.

The message is not removed from the message queue.

####  Lock(), Unlock()

`bool Lock();``void Unlock();`

These functions lock and unlock the `BMessageQueue`, so that another thread won't alter the contents of the queue while it's being read. `Lock()` doesn't return until it has the queue locked; it always returns `true`. `Unlock()` releases the lock so that someone else can lock it. Calls to these functions can be nested.

See also: [`BLooper::Lock()`](BLooper.md#BLooper_Lock "Lock\(\), LockWithTimeout\(\), Unlock\(\)")

#### NextMessage()

`[`BMessage`](BMessage.md "BMessage")* NextMessage();`

Removes and returns the oldest message from the queue. If the queue is empty, the function returns `NULL`.

See also: [`FindMessage()`](BMessageQueue.md#BMessageQueue_FindMessage "FindMessage\(\)")

<!-- Classes And Methods - The Application Kit - BMessageRunner -->

## BMessageRunner

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `app/MessageRunner.h`  
Library:| `libbe.so`  
Allocation:| Constructor only  
[Class Overview](BMessageRunner_Overview.md "BMessageRunner")

  * [Constructor and Destructor](BMessageRunner.md#BMessageRunner_ConstructorDestructor)
  * [Member Functions](BMessageRunner.md#BMessageRunner_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BMessageRunner()

`BMessageRunner([`BMessenger`](BMessenger.md "BMessenger") target,  
const [`BMessage`](BMessage.md "BMessage")* message,  
bigtime_t interval,  
int32 count = -1);` `BMessageRunner([`BMessenger`](BMessenger.md "BMessenger") target,  
const [`BMessage`](BMessage.md "BMessage")* message,  
bigtime_t interval,  
int32 count,  
BMessenger replyTo);`

Tells the roster to send the specified `message` to the `target` [`BMessenger`](BMessenger.md "BMessenger") every `interval` microseconds. The message will be sent `count` times (if `count` is -1, the message will be sent forever, or until the `BMessageRunner` is reconfigured or deleted).

The second form of the constructor lets the application specify, in `replyTo`, the [`BMessenger`](BMessenger.md "BMessenger") to which replies to the message should be sent.

The `BMessageRunner` can be reconfigured (to change the `interval` or `count`) by calling [`SetInterval()`](BMessageRunner.md#BMessageRunner_SetInterval "SetInterval\(\)") and [`SetCount()`](BMessageRunner.md#BMessageRunner_SetCount "SetCount\(\)").

After constructing a `BMessageRunner`, you should call [`InitCheck()`](BMessageRunner.md#BMessageRunner_InitCheck "InitCheck\(\)") to ensure that the object was created properly.

#### ~BMessageRunner()

`virtual ~BMessageRunner();`

Asks the roster to stop sending the message.

* * *

### Member Functions

#### GetInfo()

`status_t GetInfo(bigtime_t* interval,  
int32* count) const;`

`GetInfo()` returns in `interval` the time in microseconds that will pass between messages being sent, and in `count` the number of times the message will be sent.

Return Code| Description  
---|---  
`B_OK`.| Information returned successfully.  
`B_NAME_NOT_FOUND`.| The roster returned invalid information about the [`BMessenger`](BMessenger.md "BMessenger").  
`B_BAD_VALUE`.| The roster returned invalid information about the [`BMessenger`](BMessenger.md "BMessenger").  
Other errors.| In general, getting an error back from this function is a bad thing.  
  
#### InitCheck()

`status_t InitCheck() const;`

`InitCheck()` returns a result code indicating `B_OK` if the `BMessageRunner` constructor executed sucessfully, or some other value if an error occurred setting up the object. You should call this immediately after creating a `BMessageRunner`, and shouldn't use the object if this function returns anything but `B_OK`.

#### SetCount()

`status_t SetCount(int32 count);`

`SetCount()` sets the number of times the `BMessageRunner` will send the message. If you want the message to be sent forever (until the object is deleted or `SetCount()` is called again), specify -1.

#### SetInterval()

`status_t SetInterval(bigtime_t interval);`

`SetInterval()` sets the number of microseconds that will pass between messages being sent.

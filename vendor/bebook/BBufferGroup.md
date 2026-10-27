<!-- Classes And Methods - The Media Kit - BBufferGroup -->

## BBufferGroup

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `media/BufferGroup.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BBufferGroup_Overview.md "BBufferGroup")

  * [Constructor and Destructor](BBufferGroup.md#BBufferGroup_ConstructorDestructor)
  * [Member Functions](BBufferGroup.md#BBufferGroup_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BBufferGroup()

`BBufferGroup(size_t size,  
int32 numBuffers = 3,  
uint32 placement = B_ANY_ADDRESS,  
uint32 lock = B_FULL_LOCK);` `BBufferGroup();` `BBufferGroup(int32 numBuffers,  
const buffer_id * bufferList);`

The first form of the constructor creates a `BBufferGroup` with some number of [`BBuffer`](BBuffer.md "BBuffer")s already allocated by the group. These buffers will all live within a single Kernel Kit area, allocated by the group. The group tries to allocate an area with properties specified by the `placement` and `lock` arguments, large enough to hold `numBuffers` buffers of the specified `size` (there may be some padding added).

The second form of the constructor creates a `BBufferGroup` but doesn't create any buffers for it. You should add [`BBuffer`](BBuffer.md "BBuffer")s to the group before trying to use it.

The third form of the constructor creates a `BBufferGroup` that contains the specified list of buffers. `bufferList` points to an array of buffer IDs for the buffers to be controlled by the group, and `numBuffers` is the number of buffers in the list. This version of the constructor isn't one you'll use very often.

Before using any buffers from a new `BBufferGroup`, call [`InitCheck()`](BBufferGroup.md#BBufferGroup_InitCheck "InitCheck\(\)") to determine if any errors occurred while creating the group.

#### ~BBufferGroup()

`~BBufferGroup();`

Releases all memory used by the `BBufferGroup`, including the [`BBuffer`](BBuffer.md "BBuffer")s it controls.

Keep in mind that [`AddBuffer()`](BBufferGroup.md#BBufferGroup_AddBuffer "AddBuffer\(\)") clones the buffer area. This destructor releases the clones, but it's your application's job to release the original area you added.

Warning

![Warning](./images/admonitions/Stop_32.png)

Your node needs to delete the buffer group corresponding to any connection that is disconnected or whenever the node is deleted. Until your node does so, the other end of the connection will be blocked waiting for the buffer group to be deleted.

* * *

### Member Functions

#### AddBuffer()

`status_t AddBuffer(const buffer_clone_info& info);`

Given the buffer_clone_info, this function clones the buffer and adds the clone to the `BBufferGroup`. Normally you'll fill out the buffer_clone_info structure yourself.

Since the buffer is cloned, you'll need to delete the original memory area specified by the buffer_clone_info structure yourself when you're no longer using it; the `BBufferGroup` won't do it for you.

Note

![Note](./images/admonitions/Info_32.png)

You shouldn't pass the result of a [`BBuffer::CloneInfo()`](BBuffer.md#BBuffer_CloneInfo "CloneInfo\(\)") call to this function, as doing so would create an "alias" buffer for the same memory area. This is probably not the effect you want.

Return Code| Description  
---|---  
`B_OK`.| No error adding the buffer to the group.  
`B_NO_MEMORY`.| Couldn't clone the buffer into the `BBufferGroup`'s address space.  
  
#### AddBuffersTo()

`status_t AddBuffersTo(BMessage* message,  
const char* name,  
bool needLock = true);`

Adds the group's buffers to the specified `message`, storing them in an array with the specified `name`. If `needLock` is `true`, the `BBufferGroup` is locked before performing the operation, then unlocked when finished; otherwise, you guarantee that the buffers won't go anywhere during the `AddBuffersTo()` call.

The buffers are added by ID number, and are therefore in int32 format.

Return Code| Description  
---|---  
`B_OK`.| No error adding the buffers to the message.  
  
Errors from [`AddInt32()`](BMessage.md#BMessage_AddInt32).

#### CountBuffers()

`status_t CountBuffers(int32* outBufferCount);`

Returns, in `outBufferCount`, the number of buffers in the group.

Return Code| Description  
---|---  
`B_OK`.| The number of buffers was returned successfully.  
`B_BAD_VALUE`.| Invalid `outBufferCount` pointer  
  
#### GetBufferList()

`status_t GetBufferList(int32 listCount,  
BBuffer** outBuffers);`

Returns a list of all the buffers in the group. When calling `GetBufferList()`, pass in `outBuffers` a pointer to an array of [`BBuffer`](BBuffer.md "BBuffer") pointers that you want to be filled with pointers to the group's buffers, and specify the number of elements in the array in `listCount`.

Return Code| Description  
---|---  
`B_OK`.| No errors.  
`B_BAD_VALUE`.| `listCount` is less than 1, or `outBuffers` is `NULL`  
  
#### InitCheck()

`status_t InitCheck();`

Returns the error code resulting from the construction of the `BBufferGroup`.

Return Code| Description  
---|---  
`B_OK`.| The new group was created successfully.  
`B_ERROR`.| Couldn't allocate the buffers for the group.  
Area errors.| See [Areas](TheKernelKit_Areas_Overview.md "Areas") in The Kernel Kit.  
  
#### ReclaimAllBuffers()

`status_t ReclaimAllBuffers();`

If you pass a buffer group to some other [`BBufferProducer`](BBufferProducer.md "BBufferProducer") but pass `true` for `willReclaim` in the [`BBufferConsumer::SetOutputBuffersFor()`](BBufferConsumer.md#BBufferConsumer_SetOutputBuffersFor "SetOutputBuffersFor\(\)") call, you can later reclaim the buffers into the `BBufferGroup` by calling `ReclaimAllBuffers()`.

`ReclaimAllBuffers()` will return `B_OK` when all buffers are accounted for, or return an error if buffers can't be reclaimed.

If you have buffers that reference some object that might go away (such as a [`BBitmap`](BBitmap.md "BBitmap")), you should call `ReclaimAllBuffers()` on the group and delete the `BBufferGroup` before that object goes away.

Note

![Note](./images/admonitions/Info_32.png)

Before reclaiming your buffers, be sure to call `BBufferConsumer`::`SetOutputBuffersFor`(`output`, `NULL`) to let the Media Kit know your producer no longer has permission to use them. If you forget this step, the producer will hang onto the buffers until it's deleted, and your `ReclaimAllBuffers()` call will hang, possibly forever.

Return Code| Description  
---|---  
`B_OK`.| All buffers reclaimed successfully.  
`B_MEDIA_CANNOT_RECLAIM_BUFFERS`.| Some buffers couldn't be reclaimed.  
  
#### RequestBuffer()

`BBuffer* RequestBuffer(size_t size,  
bigtime_t timeout = B_INFINITE_TIMEOUT);` `status_t RequestBuffer(BBuffer* outBuffer,  
bigtime_t timeout = B_INFINITE_TIMEOUT);`

Returns a pointer to a [`BBuffer`](BBuffer.md "BBuffer") of at least the specified size that your [`BBufferProducer`](BBufferProducer.md "BBufferProducer") subclass can put data into, then pass on to a [media_destination](TheMediaKit_DefinedTypes.md#media_destination "media_destination"). If there isn't a suitable buffer available, the call will block until either a buffer becomes available (in which case the buffer is returned) or the specified `timeout` is reached.

If you pass a `timeout` value that's less than zero, `RequestBuffer()` will return `NULL` immediately if there's no buffer available, otherwise it will return a pointer to a buffer you can use.

Note

![Note](./images/admonitions/Info_32.png)

In BeOS Release 4.5, the timeout is ignored (unless you specify a negative value); `B_INFINITE_TIMEOUT` is always used, regardless of the value you specify.

`RequestBuffer()` doesn't use the buffer flags; instead, you can look at the buffers within a `BBufferGroup` to find a specific buffer to request yourself, based on the value returned by [`BBuffer::Flags()`](BBuffer.md#BBuffer_Flags "Flags\(\)") or [`BBuffer::SizeUsed()`](BBuffer.md#BBuffer_SizeUsed "SizeUsed\(\), SetSizeUsed\(\)"), for example. Use the second form of `RequestBuffer()` to obtain a specific buffer.

The first version of `RequestBuffer()` returns `NULL` if no buffer can be obtained, and sets `errno` to the appropriate error code. The second version returns an appropriate error code.

Warning

![Warning](./images/admonitions/Stop_32.png)

Don't call `RequestBuffer()` while an outstanding [`ReclaimAllBuffers()`](BBufferGroup.md#BBufferGroup_ReclaimAllBuffers "ReclaimAllBuffers\(\)") request is pending. To make sure that doesn't happen, a buffer producer should be designed to not know about the old group anymore once [`SetBufferGroup()`](BBufferProducer.md#BBufferProducer_SetBufferGroup "SetBufferGroup\(\)") is called to change its buffer group.

Return Code| Description  
---|---  
`B_OK`.| No errors.  
`B_MEDIA_BUFFERS_NOT_RECLAIMED`.| Buffers are in the process of being reclaimed.  
`B_ERROR`.| A miscellaneous error occurred.  
  
#### RequestError()

`status_t RequestError();`

Returns the last [`RequestBuffer()`](BBufferGroup.md#BBufferGroup_RequestBuffer "RequestBuffer\(\)") error. This is useful if [`RequestBuffer()`](BBufferGroup.md#BBufferGroup_RequestBuffer "RequestBuffer\(\)") returns `NULL`.

#### WaitForBuffers()

`status_t WaitForBuffers();`

Waits until the currently pending buffer reclamation is finished, then returns. If there isn't a buffer reclamation in progress, returns immediately.

Return Code| Description  
---|---  
`B_OK`.| No errors.  
Semaphore errors.| Unable to acquire the semaphore used to detect that buffer reclamation is done.

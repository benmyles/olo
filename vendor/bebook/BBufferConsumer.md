<!-- Classes And Methods - The Media Kit - BBufferConsumer -->

## BBufferConsumer

| Derived From:| [`BMediaNode`](BMediaNode.md "BMediaNode")  
---|---  
Mix-in Classes:| –  
Declared In:| `media/BBufferConsumer.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BBufferConsumer_Overview.md "BBufferConsumer")

  * [Constructor and Destructor](BBufferConsumer.md#BBufferConsumer_ConstructorDestructor)
  * [Hook Functions](BBufferConsumer.md#BBufferConsumer_HookFunctions)
  * [Member Functions](BBufferConsumer.md#BBufferConsumer_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BBufferConsumer()

##### protected

`BBufferConsumer(media_type consumerType);`

The `BBufferConsumer` constructor. Specify as `consumerType` the type of data the `BBufferConsumer` accepts.

Note

![Note](./images/admonitions/Info_32.png)

In BeOS Release 4.5.2 and earlier `consumerType` has a default value. It no longer does. You'll have to actually specify the media type from now on.

#### ~BBufferConsumer

##### protected

`~BBufferConsumer();`

The `BBufferConsumer` destructor. You can augment this to handle whatever closing-out your consumer node requires.

If your node has created and set [`BBufferGroup`](BBufferGroup.md "BBufferGroup")s for any producers, you should delete them in the destructor.

* * *

### Hook Functions

#### AcceptFormat()

`virtual status_t AcceptFormat(const media_destination & destination,  
media_format* format)  = 0;`

Implement this hook function to check that the specified format is reasonable for the specified destination, and to fill in any wildcard fields for which your `BBufferConsumer` has specific requirements.

If the format isn't reasonable (or is of a class that's unsuitable for `destination`), return `B_MEDIA_BAD_FORMAT`.

When `AcceptFormat()` returns `B_OK`, the Media Kit will expect a connection request on `destination` with the specified `format` not to fail due to a format incompatibility.

Warning

![Warning](./images/admonitions/Stop_32.png)

Don't try to ask the upstream producer about the format; it's waiting synchronously for your response, and doing so will cause deadlock.

#### BufferReceived()

##### protected

`virtual void BufferReceived(BBuffer* buffer)  = 0;`

When a [`BBufferProducer`](BBufferProducer.md "BBufferProducer") sends buffers to one of your `BBufferConsumer`'s inputs, it will eventually arrive here, at the `BufferReceived()` function (usually after first being dispatched by [`HandleMessage()`](BBufferConsumer.md#BBufferConsumer_HandleMessage "HandleMessage\(\)")).

Override this hook function to add the `buffer` to your internal playback queue, or to do whatever your node needs to do with buffers you consume. If you implement both [`BBufferProducer`](BBufferProducer.md "BBufferProducer") and `BBufferConsumer`, it's possible you might examine or alter the data in the `buffer` and then call [`BBufferProducer::SendBuffer()`](BBufferProducer.md#BBufferProducer_SendBuffer "SendBuffer\(\)") to send it along to someone else.

If your processing of the buffer is long enough to cause the buffer to become late, you should still process it as usual, but you should also call [`NotifyLateProducer()`](BBufferConsumer.md#BBufferConsumer_NotifyLateProducer "NotifyLateProducer\(\)") to let the producer know things are starting to lag.

Information about the contents and timing requirements of the buffer can be obtained by calling [`BBuffer::Header()`](BBuffer.md#BBuffer_Header "Header\(\)") on it.

Warning

![Warning](./images/admonitions/Stop_32.png)

If you're writing a node, and receive a buffer with the `B_SMALL_BUFFER` flag set, you must recycle the buffer before returning.

#### Connected()

##### protected

`virtual status_t Connected(const media_source & destination,  
const media_format* format,  
media_input* outInput)  = 0;`

This hook function is called when a connection is being established to your input destination from the specified source producer. The connection will be composed of media data with the specified format (which you've previously accepted via [`AcceptFormat()`](BBufferConsumer.md#BBufferConsumer_AcceptFormat "AcceptFormat\(\)")).

Your implementation of `Connected()` should do whatever preparation you need to do to handle data input on the connection, and fill out the `outInput` buffer with information about the connection from your node's point-of-view. You can set `outInput`'s destination field different from destination if destination is a global connection-establishing input that's used to negotiate a connection, then create a new input to actually handle the data stream.

Note

![Note](./images/admonitions/Info_32.png)

Since your `BBufferConsumer` has already had the opportunity to reject the specified format, it's poor form to return an error from this function. You should only return an error if the resources needed to establish the connection have become unavailable prior to the time `Connected()` was called.

On entry, `outInput`'s name field contains the name given the connection by the producer (this may be an empty string if the producer didn't assign a name). Your consumer should always make sure there's a valid name here, because it's a bad thing to have unnamed connections, and there's no guarantee that the producer will fill this in. If you don't have a good, descriptive name for a connection, the name should minimally contain the name of the node and a number that makes the connection's name unique (such as "MyNode Input 1" or "MyNode Output 3").

If you want the producer to use a specific [`BBufferGroup`](BBufferGroup.md "BBufferGroup") (for example, if you want a video producer to fill [`BDirectWindow`](BDirectWindow.md "BDirectWindow") buffers), you should create the [`BBufferGroup`](BBufferGroup.md "BBufferGroup") here, then call [`BBufferProducer::SetBufferGroup()`](BBufferProducer.md#BBufferProducer_SetBufferGroup "SetBufferGroup\(\)") to set the producer's buffer group:
    
    
    BBufferGroup *buffers = new BBufferGroup;
    BMediaRoster::Roster()->SetOutputBuffersFor(producer, buffers);

Return `B_OK` if the connection is started safely, otherwise, return an appropriate error code.

#### Disconnected()

##### protected

`virtual void Disconnected(const media_source& producer,  
const media_destination& whichInput)  = 0;`

This hook function is called when a connection is being terminated. You should do whatever needs to be done in order to ensure that future inquiries about the [media_source](TheMediaKit_DefinedTypes.md#media_source "media_source") connected to the [media_input](TheMediaKit_DefinedTypes.md#media_input "media_input") indicated by `whichInput` reference media_source::null (or, if another connection is later established on the input, that producer).

If your consumer node has created and set a [`BBufferGroup`](BBufferGroup.md "BBufferGroup") for the producer, you shouldn't delete or reclaim it here, because the producer has a clone of the [`BBufferGroup`](BBufferGroup.md "BBufferGroup") that references the same buffers; deleting the [`BBufferGroup`](BBufferGroup.md "BBufferGroup") would free those buffers, leaving the producer in deadlock. Instead, delete (or reclaim) the when [`Connected()`](BBufferConsumer.md#BBufferConsumer_Connected "Connected\(\)") is called again, and be sure to delete any remaining [`BBufferGroup`](BBufferGroup.md "BBufferGroup")s in your destructor.

#### DisposeInputCookie()

##### protected

`virtual void DisposeInputCookie(int32 cookie)  = 0;`

If the cookie value you return in [`GetNextInput()`](BBufferConsumer.md#BBufferConsumer_GetNextInput "GetNextInput\(\)") is a pointer to an object that needs to be deleted when the iteration process is completed, be sure to implement `DisposeInputCookie()` to do so.

#### GetLatencyFor()

##### protected

`virtual status_t GetLatencyFor(const media_destination& forWhom,  
bigtime_t* outLatency,  
media_node_id* outTimeSource)  = 0;`

Implement this hook function to calculate the total latency for the [media_destination](TheMediaKit_DefinedTypes.md#media_destination "media_destination") specified by `forWhom` and store the resulting value in `outLatency`. Also, return the time source your node is slaved to in `outTimeSource`.

If your node is a [`BBufferProducer`](BBufferProducer.md "BBufferProducer")

Return `B_OK` if you successfully compute the latency; otherwise, return an appropriate error.

#### GetNextInput()

`virtual status_t GetNextInput(int32 * cookie,  
media_input* outInput)  = 0;`

The first time a client calls this function, the value pointed to by cookie will be 0. You should fill the buffer pointed to by `outInput` with information about your first input, and set the value at `cookie` to something (other than zero) that will let you keep track of what to return the next time `GetNextInput()` is called.

Each successive call to `GetNextInput()` will pass back, in `cookie`, the value you returned in `cookie` the last time the function was called by that client, and you should fill `outInput` with information about the next input, and store a new value in `cookie` to continue to track your progress through the inputs.

Note

![Note](./images/admonitions/Info_32.png)

Whenever this function is called with a value of zero in cookie, you must start over with the first input.

When you reach the last input, return `B_BAD_INDEX` to indicate that there aren't any more inputs.

#### HandleMessage()

##### protected

`virtual status_t HandleMessage(int32 message,  
const void* data,  
size_t size);`

When your node derived from `BBufferConsumer` receives a message on its control port, you should try dispatching it by calling `HandleMessage()`. If `BBufferConsumer` doesn't understand the message, it'll return `B_ERROR` and you can try dispatching it to another class from which your node is derived, or handle it yourself.

If this function returns `B_OK`, the message has been handled.

See also: [`BMediaNode::HandleMessage()`](BMediaNode.md#BMediaNode_HandleMessage "HandleMessage\(\)"), "[About Multiple Virtual Inheritance](TheMediaKit_Overview_Introduction.md#TheMediaKit_AboutMultipleVirtualInheritance "About Multiple Virtual Inheritance")"

#### ProducerDataStatus()

##### protected

`virtual void ProducerDataStatus(const media_destination & destination,  
int32 status,  
bigtime_t atPerformanceTime)  = 0;`

This hook function is called to inform your consumer about changes in the availability of buffers from the producer that's connected to the input destination. The status argument specifies what change has occurred, and `atPerformanceTime` indicates when the change happened (or when it will happen).

This lets you keep track of which inputs you should await data from; for example, if your consumer is processing data arriving from four producers, and one of them stops sending buffers to the consumer, the producer that's stopping will cause a call to `ProducerDataStatus()` to let you know not to await buffers anymore. This way, you know that when buffers have arrived from the other three inputs, it's okay to begin processing the buffers.

Constant| Description  
---|---  
`B_DATA_NOT_AVAILABLE`| The producer doesn't have any data available.  
`B_DATA_AVAILABLE`| The producer has data available.  
`B_PRODUCER_STOPPED`| The producer has been stopped.  
  
#### SeekTagRequested()

##### protected

`virtual status_t SeekTagRequested(const media_destination& destination,  
bigtime_t inTargetTime,  
uint32 inFlags,  
media_seek_tag* outSeekTag,  
bigtime_t* outTaggedTime,  
uint32* outFlags);`

This function is provided to aid in supporting media formats in which the outer encapsulation layer doesn't supply timing information. Producers will tag the buffers they generate with seek tags; these tags can be used to locate key frames in the media data.

It's the consumer's job to match up seek tags with performance times. As the consumer processes each incoming buffer, it should cache the seek tag and the performance time at which it occurs (if there's a tag on the buffer). When the producer needs to know the seek tag and corresponding time that's closest to a given performance time, this function is resonsible for returning that information.

Return Code| Description  
---|---  
`B_OK`.| No error.  
Other errors.| Depend on the node's implementation.  
  
See also: "[Seek Tags](BBufferProducer_Overview.md#BBufferProducer_Overview_SeekTags "Seek Tags")"

* * *

### Member Functions

#### ConsumerType()

`media_type ConsumerType();`

Returns the type of media the `BBufferConsumer` consumes.

#### NotifyLateProducer()

##### static

`void NotifyLateProducer(const media_source & source,  
bigtime_t howLate,  
bigtime_t performanceTime);`

Notifies the [`BBufferProducer`](BBufferProducer.md "BBufferProducer") specified by source that it's running late by `howLate` microseconds; the notification conditions as of the specified `performanceTime`. Call this function when you detect that data is arriving too late and the run mode is `B_DECREASE_PRECISION`, `B_INCREASE_LATENCY`, or `B_DROP_DATA` (any of which permits adjustment of the media playback to maintain timeliness).

The producer should process this notification immediately and take the appropriate action.

#### RegionToClipData()

##### protected

`static status_t RegionToClipData(const BRegion* region,  
int32* format,  
int32* ioSize,  
void* data);`

Converts a [`BRegion`](BRegion.md "BRegion") into the clipping format used internally by the Media Kit. Prior to calling `RegionToClipData()`, `ioSize` is set to the size of the buffer pointed to by `data`. On return, `format` is the format of the clipping data, `ioSize` is changed to the actual number of bytes of data returned, and `data` contains the actual clipping data.

The clip data format is described in the section "[Video Clipping](BBufferProducer_Overview.md#BBufferProducer_Overview_VideoClipping "Video Clipping")".

Return Code| Description  
---|---  
`B_OK`.| Clip data returned without errors.  
`B_NO_MEMORY`.| The data buffer isn't big enough.  
  
#### RequestAdditionalBuffer()

##### protected

`status_t RequestAdditionalBuffer(const media_source& source,  
BBuffer* previousBuffer,  
void* _reserved_ = NULL);` `status_t RequestAdditionalBuffer(const media_source& source,  
bigtime_t startTime,  
void* _reserved_ = NULL);`

Asks the upstream producer specified by `source` to immediately send the next buffer, instead of waiting until the appropriate time. The most obvious use for this function is in cases where a codec requires multiple buffers in order to decode a frame of output (MPEG is a good example).

The requested buffer can be identified either by a `startTime` parameter, which indicates the time for which a buffer is requested, or by a `previousBuffer`, which specifies the buffer prior to the one being requested.

This function will cause the producer's [`AdditionalBufferRequested()`](BBufferProducer.md#BBufferProducer_AdditionalBufferRequested "AdditionalBufferRequested\(\)") function to be called.

Return Code| Description  
---|---  
`B_OK`.| The change was requested successfully.  
`B_MEDIA_BAD_SOURCE`.| The source isn't valid.  
`B_BAD_VALUE`.| The `previousBuffer` pointer is `NULL`.  
`B_TIMEOUT`.| The request to the Media Server timed out.  
`Port errors`.| An error occurred communicating with the Media Server.  
  
####  RequestFormatChange(), FormatChanged()

##### protected

`status_t RequestFormatChange(const media_source& source,  
const media_destination& destination,  
media_format& toFormat,  
void* userData,  
const media_destination& changeTag,  
void* _reserved_ = NULL);``virtual status_t FormatChanged(const media_source& source,  
const media_destination& destination,  
media_format& newFormat)  = 0;`

`RequestFormatChange()` requests that the producer `source` connected to the consumer `destination` change the format it produces to the format specified by `toFormat`. The Media Kit returns in `changeTag` the tag value that will be received by your [`RequestCompleted()`](BMediaNode.md#BMediaNode_RequestCompleted "RequestCompleted\(\)") function once the change takes effect; the change tag lets you match up the call to [`RequestCompleted()`](BMediaNode.md#BMediaNode_RequestCompleted "RequestCompleted\(\)") with this request. This function will receive a [media_request_info](TheMediaKit_DefinedTypes.md#media_request_info "media_request_info") structure with the indicated `userData` and `changeTag`.

`FormatChanged()` is called by the upstream producer when the media format your node will be receiving changes, and indicates the new format in `newFormat` and the change tag value at which the new format will take effect in `changeTag`. You should implement this function so your node will know that the data format is going to change. Note that this may be called in response to your [`AcceptFormat()`](BBufferConsumer.md#BBufferConsumer_AcceptFormat "AcceptFormat\(\)") call, if your [`AcceptFormat()`](BBufferConsumer.md#BBufferConsumer_AcceptFormat "AcceptFormat\(\)") call alters any wildcard fields in the specified format.

Note

![Note](./images/admonitions/Info_32.png)

Because `FormatChanged()` is called by the producer, you don't need to (and shouldn't) ask it if the new format is acceptable.

If the format change isn't possible, return an appropriate error from `FormatChanged()`; this error will be passed back to the producer that initiated the new format negotiation in the first place.

Return Code| Description  
---|---  
`B_OK`.| No error.  
`B_MEDIA_BAD_SOURCE`.| The specified `source` isn't valid.  
`B_MEDIA_BAD_DESTINATION`.| The specified `destination` is invalid.  
Port errors.| See Ports.  
  
#### SendLatencyChange()

##### protected

`status_t SendLatencyChange(const media_source& source,  
const media_destination& destination,  
bigtime_t newLatency,  
uint32 flags = 0);`

Lets the upstream producer know that the consumer node's latency has changed. `newLatency` indicates your new latency, in microseconds. The flags are currently unused and should always be 0.

You should call this whenever something happens to cause a change in your latency.

Return Code| Description  
---|---  
`B_OK`.| No error.  
`B_MEDIA_BAD_SOURCE`.| The `source` is invalid.  
`B_MEDIA_BAD_DESTINATION`.| The `destination` is invalid.  
`B_TIMED_OUT`.| The attempt to communicate with the Media Server timed out.  
Port errors.| An error occurred communicating with the Media Server.  
  
#### SetOutputBuffersFor()

##### protected

`status_t SetOutputBuffersFor(const media_source& source,  
const media_destination& destination,  
BBufferGroup* group,  
void* userData,  
int32* changeTag,  
bool willReclaim = false,  
void* _reserved_ = NULL);`

Specifies that the [`BBufferGroup`](BBufferGroup.md "BBufferGroup") group will provide the buffers for the connection between source and destination. If `willReclaim` is `false`, the Media Kit will dispose of the group for you; you can forget about it once this call returns. Otherwise, you're informing the Media Server that you want the group back, and that you'll delete it when you're done with it.

The Media Kit returns in `changeTag` the tag value that will be received by your [`RequestCompleted()`](BMediaNode.md#BMediaNode_RequestCompleted "RequestCompleted\(\)") function once the change takes effect; the change tag lets you match up the call to [`RequestCompleted()`](BMediaNode.md#BMediaNode_RequestCompleted "RequestCompleted\(\)") with this request. This function will receive a [media_request_info](TheMediaKit_DefinedTypes.md#media_request_info "media_request_info") structure with the indicated `userData` and `changeTag`.

The ability to request that certain buffers be used by a particular output can save you from having to perform unnecessary copies; you might be able to use [`BBuffer`](BBuffer.md "BBuffer")s that represent a graphics card frame buffer, for example, so that a video producer's output goes directly to video memory.

Note

![Note](./images/admonitions/Info_32.png)

Before reclaiming your buffers, be sure to call `SetOutputBuffersFor`(`output`, `NULL`) to let the Media Kit know your producer no longer has permission to use them. If you forget this step, the producer will hang onto the buffers until it's deleted, and your [`BBufferGroup::ReclaimAllBuffers()`](BBufferGroup.md#BBufferGroup_ReclaimAllBuffers "ReclaimAllBuffers\(\)") call will hang, possibly forever.

Return Code| Description  
---|---  
`B_OK`.| The change was requested successfully.  
Other errors.| The change could not be made.  
  
#### SetOutputEnabled()

##### protected

`static status_t SetOutputEnabled(const media_source& source,  
const media_destination& destination,  
bool enabled,  
void* userData,  
int32* changeTag,  
void* _reserved_ = NULL);`

Specifies whether or not the specified output should be transmitting buffers to the destination. If `enabled` is `true`, the producer should transmit buffers; otherwise it should not.

The Media Kit returns in `changeTag` the tag value that will be received by your [`RequestCompleted()`](BMediaNode.md#BMediaNode_RequestCompleted "RequestCompleted\(\)") function once the change takes effect; the change tag lets you match up the call to [`RequestCompleted()`](BMediaNode.md#BMediaNode_RequestCompleted "RequestCompleted\(\)") with this request. This function will receive a media_request_info structure with the indicated `userData` and `changeTag`.

Return Code| Description  
---|---  
`B_OK`.| The change was requested successfully.  
`B_MEDIA_BAD_SOURCE`| The source is invalid.  
  
#### SetVideoClippingFor()

##### protected

`status_t SetVideoClippingFor(const media_source& output,  
const media_destination& destination,  
const int16* shortsList,  
int32 shortCount,  
const media_video_display_info& display,  
void* userData,  
int32* changeTag,  
void* _reserved_ = NULL);`

This function requests that video buffers sent by the specified output to the specified destination clip all its writing in buffers it sends to the `BBufferConsumer` to the clipping region described by `shortsList` and `shortCount`. The clip data format is described in the section "[Video Clipping](BBufferProducer_Overview.md#BBufferProducer_Overview_VideoClipping "Video Clipping")".

The Media Kit returns in changeTag the tag value that will be received by your [`RequestCompleted()`](BMediaNode.md#BMediaNode_RequestCompleted "RequestCompleted\(\)") function once the change takes effect; the change tag lets you match up the call to [`RequestCompleted()`](BMediaNode.md#BMediaNode_RequestCompleted "RequestCompleted\(\)") with this request. This function will receive a [media_request_info](TheMediaKit_DefinedTypes.md#media_request_info "media_request_info") structure with the indicated `userData` and `changeTag`.

The [media_video_display_info](TheMediaKit_DefinedTypes.md#media_video_display_info "media_video_display_info") structure referenced by display describes the current configuration of the video in terms of color space, resolution, and so forth.

Return Code| Description  
---|---  
`B_OK`.| The clipping request has been sent without errors.  
`B_MEDIA_BAD_CLIP_FORMAT`| The clipping data isn't formatted correctly.  
Port errors| See [Ports](TheKernelKit_Ports_Overview.md "Ports").

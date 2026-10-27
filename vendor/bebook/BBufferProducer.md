<!-- Classes And Methods - The Media Kit - BBufferProducer -->

## BBufferProducer

| Derived From:| [`BMediaNode`](BMediaNode.md "BMediaNode")  
---|---  
Mix-in Classes:| –  
Declared In:| `media/BufferProducer.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BBufferProducer_Overview.md "BBufferProducer")

  * [Constructor and Destructor](BBufferProducer.md#BBufferProducer_ConstructorDestructor)
  * [Member Functions](BBufferProducer.md#BBufferProducer_MemberFunctions)
  * [Constants](BBufferProducer.md#BBufferProducer_Constants)

  
  
* * *

### Constructor and Destructor

#### BBufferProducer()

##### protected

`explicit BBufferProducer(media_type producerType);`

Constructs the `BBufferProducer` object. The `producerType` specifies the type of media data that will be output by the node.

If the node will produce more than one type of data, your `BBufferProducer` subclass should set the kind to the default (which is a wildcard value).

If your node has additional latency on startup, you should call [`SetInitialLatency()`](BBufferProducer.md#BBufferProducer_SetInitialLatency "SetInitialLatency\(\)") to record this information. This might be the case if the buffers your node produces are created from an input signal which refreshes infrequently, such as a television signal.

Note

![Note](./images/admonitions/Info_32.png)

In BeOS Release 4.5.2 and earlier, the `producerType` has a default value; it no longer does, and you'll have to specify the media type yourself.

* * *

### Member Functions

#### AdditionalBufferRequested()

`virtual status_t AdditionalBufferRequested(const media_source& source,  
media_buffer_id previousBufferID,  
bigtime_t previousTime,  
const media_seek_tag* previousTag);`

When a consumer calls [`BBufferConsumer::RequestAdditionalBuffer()`](BBufferConsumer.md#BBufferConsumer_RequestAdditionalBuffer "RequestAdditionalBuffer\(\)"), this function is called as a result. Its job is to call [`SendBuffer()`](BBufferProducer.md#BBufferProducer_SendBuffer "SendBuffer\(\)") to immediately send the next buffer to the consumer.

The `previousBufferID`, `previousTime`, and `previousTag` arguments identify the last buffer the consumer received. Your node should respond by sending the next buffer after the one described.

Note

![Note](./images/admonitions/Info_32.png)

The `previousTag` may be `NULL`.

Return `B_OK` if all is well; otherwise return an appropriate error code.

#### ChangeFormat()

`status_t ChangeFormat(const media_source& source,  
const media_destination& destination,  
media_format* newFormat);`

Informs the destination that the data flowing between source and destination is immediately changing to the format specified by `newFormat.`

Warning

![Warning](./images/admonitions/Stop_32.png)

You must never call [`SendBuffer()`](BBufferProducer.md#BBufferProducer_SendBuffer "SendBuffer\(\)") while this call is pending.

Return Code| Description  
---|---  
`B_OK`.| The format change request has been sent without error.  
`B_MEDIA_CHANGE_IN_PROGRESS`.| A mutual exclusion error has occurred with [`SendBuffer()`](BBufferProducer.md#BBufferProducer_SendBuffer "SendBuffer\(\)"); `ChangeFormat()` and [`SendBuffer()`](BBufferProducer.md#BBufferProducer_SendBuffer "SendBuffer\(\)") can't both be running at the same time.  
Other errors.| You may receive other errors if the consumer doesn't agree with the new format you're requesting.  
  
#### ClipDataToRegion()

`static status_t ClipDataToRegion(int32 format,  
int32 byteCount,  
const void* clipData,  
BRegion* region);`

Given `byteCount` bytes of clipping data `clipDatain` the specified `format`, makes the specified `region` match the clipping region.

The `region` you specify must already exist.

The only format currently supported is `B_CLIP_SHORT_RUNS`.

Return Code| Description  
---|---  
`B_OK`.| The clip data was converted without error.  
`B_MEDIA_BAD_CLIP_FORMAT`.| The specified clip format is invalid.  
  
See also: [`BBufferConsumer::RegionToClipData()`](BBufferConsumer.md#BBufferConsumer_RegionToClipData "RegionToClipData\(\)")

#### Connect()

`virtual void Connect(status_t status,  
const media_source& source,  
const media_destination& destination,  
const media_format& format,  
char* ioName);`

Implement this hook function to establish a connection between the source and the destination. The format negotiation is already complete by the time `Connect()` is called, so you have to accept the specified `format`.

The `status` argument indicates whether or not the connection actually took place; this is the result code returned by the [`BBufferConsumer::Connected()`](BBufferConsumer.md#BBufferConsumer_Connected "Connected\(\)") function or an error code indicating an error that has occurred during other preparation for the connection.

If `status` isn't `B_OK`, you should release the media_source that was reserved for this connection by [`PrepareToConnect()`](BBufferProducer.md#BBufferProducer_PrepareToConnect "PrepareToConnect\(\)"); this lets it be used by other connection attempts.

On entry, `ioName` contains the connection name specified by the consumer (this may be different from the name specified by the [`PrepareToConnect()`](BBufferProducer.md#BBufferProducer_PrepareToConnect "PrepareToConnect\(\)") function). On return, `ioName` should point to a name for the connection; if the name really matters to you, copy the name you want the connection to have back into `ioName`; otherwise, you can leave it alone.

#### Disconnect()

`virtual void Disconnect(const media_source& source,  
const media_destination& destination)  = 0;`

Your implementation of `Disconnect()` should terminate the connection between the specified `source` and `destination`. Once you return from this function, you shouldn't send any further buffers on the connection.

If a [`BBufferGroup`](BBufferGroup.md "BBufferGroup") has been specified for your producer (via the [`SetBufferGroup()`](BBufferProducer.md#BBufferProducer_SetBufferGroup "SetBufferGroup\(\)") function), you should delete it here.

#### DisposeOutputCookie()

`virtual status_t DisposeOutputCookie(int32 cookie)  = 0;`

Once a client has finished iterating through your outputs via [`GetNextOutput()`](BBufferProducer.md#BBufferProducer_GetNextOutput "GetNextOutput\(\)") calls, it will call this function with the last value you returned as a `cookie`. This gives you the opportunity to dispose of any memory you may have allocated for the iteration process.

Return `B_OK` if the cookie is successfully disposed of (or if nothing needs to be done); otherwise, return an appropriate error code.

#### EnableOutput()

`virtual void EnableOutput(const media_source& whichOutput,  
bool enabled,  
int32* _deprecated_)  = 0;`

This hook function is called when a consumer's [`SetOutputEnabled()`](BBufferConsumer.md#BBufferConsumer_SetOutputEnabled "SetOutputEnabled\(\)") function is called. This indicates whether or not the output specified by `whichOutput` needs to be sent buffers. You must implement this function so that you don't send buffers to outputs that don't need them. The `_deprecated_` argument is no longer used.

By default, output is enabled.

#### FindLatencyFor()

`status_t FindLatencyFor(const media_destination& forDestination,  
bigtime_t* outLatency,  
media_node_id* outTimeSource)  = 0;`

`FindLatencyFor()` returns the latency introduced by sending data to the destination `forDestination`. On return, the latency will be stored in `outLatency`, and the time source used by `forDestination` will be available in `outTimeSource` (unless an error is returned, in which case these values are undetermined).

The latency of sending a buffer from one time source to another should always be assumed to be zero, since there may be no relationship between the progress of time of two different time sources.

Return Code| Description  
---|---  
`B_OK`.| The latency was returned without error.  
`B_MEDIA_BAD_DESTINATION`.| The destination is invalid.  
Port errors.| The request couldn't be sent to the destination.  
  
#### FindSeekTag()

`status_t FindSeekTag(const media_destination& forDestination,  
bigtime_t inTargetTime,  
media_seek_tag* outTag,  
bigtime_t* outTaggedTime,  
uint32* outFlags = 0,  
uint32* inFlags = 0);`

In order to improve seek performance, the Media Kit provides the concept of seek tags. These are special tags that identify easily-located points in media data (such as key frames in Cinepak video). The `FindSeekTag()` function asks the consumer specified by `forDestination` for the nearest seek tag to the time specified by `inTargetTime`, and returns the tag in `outTag` and the time corresponding to that tag in `outTaggedTime`. On return, `outFlags` (if the pointer isn't `NULL`) contains flags giving further details about the tag.

There are currently no defined values for `inFlags` or `outFlags`.

Return Code| Description  
---|---  
`B_OK`.| No error.  
Port errors.| An error occurred communicating with the Media Server.  
  
#### FormatChangeRequested()

`virtual status_t FormatChangeRequested(const media_source& source,  
const media_destination& destination,  
media_format* ioFormat,  
int32* _deprecated_)  = 0;`

Implement `FormatChangeRequested()` to change the format of the media data flowing from the given `source` to the specified `destination` to the format specified by `ioFormat`. If there are wildcards specified in `ioFormat`, fill them in to match the format you prefer before returning from this call. You should ignore the `_deprecated_` argument; it's no longer used.

Warning

![Warning](./images/admonitions/Stop_32.png)

This call is issued synchronously by the destination, so you can't ask it if the format is acceptable. Fortunately, since the destination issued the request, you can safely assume that it's fine.

Return `B_OK` if the change request is processed successfully; otherwise, return an appropriate error code.

See also: [`FormatSuggestionRequested()`](BBufferProducer.md#BBufferProducer_FormatSuggestionRequested "FormatSuggestionRequested\(\)"), [`FormatProposal()`](BBufferProducer.md#BBufferProducer_FormatProposal "FormatProposal\(\)")

#### FormatProposal()

`virtual status_t FormatProposal(const media_source& output,  
media_format* format)  = 0;`

Your `BBufferProducer` should implement this function to verify that the proposed media_format is suitable for the specified `output`. If any fields in the `format` are wildcards, and you have a specific requirement, adjust those fields to match your requirements before returning.

Return `B_OK` if the proposed format is acceptable; once you've done so, the Media Kit will assume that any connection request made on output with the specified format (after any changes you may have made) will succeed.

Constant| Description  
---|---  
`B_BAD_SOURCE`| `output` isn't available.  
`B_BAD_MEDIA_FORMAT`| `format` isn't reasonable.  
  
#### FormatSuggestionRequested()

`virtual status_t FormatSuggestionRequested(media_type type,  
int32 quality,  
media_format* format)  = 0;`

You must implement `FormatSuggestionRequested()` to return fill the buffer pointed to by format that your producer is capable of emitting that meets the desired type and quality requirements.

If your producer can work with a range of possible formats, let the quality argument guide your selection. For example, you might choose to use 10 fps for previews, and 60 fps interlaced 640x480 for full-quality video.

If `type` is a media class that your producer doesn't want to work with, return `B_BAD_MEDIA_FORMAT`. If you're preapared to accept a wide range of values for some specific field, set that field to the wildcard value (see media_audio_format::wildcard() and media_video_format::wildcard() for more information.

Return `B_OK` if the `format` is successfully returned.

#### GetLatency()

`virtual status_t GetLatency(bigtime_t* outLatency)  = 0;`

Implement this hook function to store, in `outLatency`, the total amount of latency your `BBufferProducer` incurs from receiving a buffer of data until it reaches its ultimate destination.

Call [`FindLatencyFor()`](BBufferProducer.md#BBufferProducer_FindLatencyFor "FindLatencyFor\(\)") on whatever outputs the data is being forwarded to, add your own latency to the largest of those values, and return that value.

The default implementation of `GetLatency()` finds the maximum latency of your currently-available outputs by iterating over them, and returns that value in `outLatency`; therefore, your implementation of this function may simply need to call the inherited version of this function, then add your own processing latency to the returned value.

Return Code| Description  
---|---  
`B_OK`.| The latency has been returned successfully.  
Other errors.| Unable to calculate the latency.  
  
#### GetNextOutput()

`virtual status_t GetNextOutput(int32* cookie,  
media_output* outOutput)  = 0;`

Implement this function to return information about your available outputs. The first time it's called for a new iteration loop, the value pointed to by `cookie` will be 0. Each time `GetNextOutput()` is called, you should set it to some value that makes sense to you so you can keep track of where in the iteration process the client is, but never set it to 0.

For each call to `GetNextOutput()`, including the first, you should return one of your outputs that the client hasn't seen during the iteration loop in `outOutput`.

Once all outputs have been reported, you should return `B_ERROR`.

#### HandleMessage()

`virtual status_t HandleMessage(int32 message,  
const void* data,  
size_t size);`

When your node derived from `BBufferProducer` receives a message on its control port, you should handle it yourself if you know how, or dispatch to each ancestor class in turn (starting with `BBufferProducer`'s `HandleMessage()`) until one of the `HandleMessage()` implementations returns `B_OK`. If none of the inherited implementations of this function returns `B_OK`, you should pass the message to [`BMediaNode::HandleBadMessage()`](BMediaNode.md#BMediaNode_HandleBadMessage "HandleBadMessage\(\)") to be dealt with.

Your port-listening thread should call `HandleMessage()` to dispatch the received data.

See also: "[About Multiple Virtual Inheritance](TheMediaKit_Overview_Introduction.md#TheMediaKit_AboutMultipleVirtualInheritance "About Multiple Virtual Inheritance")"

#### LatencyChanged()

`virtual void LatencyChanged(const media_source& source,  
const media_destination& destination,  
bigtime_t newLatency,  
uint32 flags);`

This hook function is called when a [`BBufferConsumer`](BBufferConsumer.md "BBufferConsumer") that's receiving data from you determines that its latency has changed. It will call its [`BBufferConsumer::SendLatencyChange()`](BBufferConsumer.md#BBufferConsumer_SendLatencyChange "SendLatencyChange\(\)") function, and in response, the Media Server will call your `LatencyChanged()` function.

The `source` argument indicates your output that's involved in the connection, and `destination` specifies the input on the consumer to which the connection is linked. `newLatency` is the consumer's new latency. The `flags` are currently unused.

Override this function to implement whatever functionality you need to adjust your own latency calculations to keep the data flowing smoothly.

#### LateNoticeReceived()

`virtual void LateNoticeReceived(const media_source& whichSource,  
bigtime_t howLate,  
bigtime_t performanceTime)  = 0;`

This hook function is called when a [`BBufferConsumer`](BBufferConsumer.md "BBufferConsumer") that's receiving data from you determines that data is arriving late (when the [`BBufferConsumer::NotifyLateProducer()`](BBufferConsumer.md#BBufferConsumer_NotifyLateProducer "NotifyLateProducer\(\)") function is called); the exact degree to which your buffers are late is specified by the `howLate` argument. Your implementation of this function should take whatever steps are necessary to correct the problem, either by asking nodes upstream from you to deliver buffers earlier, dropping buffers, or other appropriate actions, depending on the current run mode.

The `performanceTime` argument specifies the performance time at which the notification was sent.

See also: [`BMediaNode::RunMode()`](BMediaNode.md#BMediaNode_RunMode "RunMode\(\), SetRunMode\(\)")

#### PrepareToConnect()

`virtual status_t PrepareToConnect(const media_source& whichSource,  
const media_destination& whichDestination,  
media_format* format,  
media_source* outSource,  
char* outName)  = 0;`

The `PrepareToConnect()` hook is called before a new connection between the source `whichSource` and the destination `whichDestination` is established, in order to give your producer one last chance to specialize any wildcards that remain in the format (although by this point there shouldn't be any, you should check anyway).

Your implementation should, additionally, return in `outSource` the source to be used for the connection, and should fill the `outName` buffer with the name the connection will be given; the consumer will see this in the `outInput`->`name` argument specified to [`BBufferConsumer::Connected()`](BBufferConsumer.md#BBufferConsumer_Connected "Connected\(\)"). If your node doesn't care what the name is, you can leave the `outName` untouched.

Note

![Note](./images/admonitions/Info_32.png)

Your [`Connect()`](BBufferProducer.md#BBufferProducer_Connect "Connect\(\)") function may return a different media_source value in `outOutput`'s source field than the one specified as the `source` argument to this function. One reason you might do this is if you implement one media_source to accept connection requests, then create a new media_source to actually handle each connection.

Return `B_OK` if the connection process should proceed, or an appropriate error code if something's wrong.

If you return `B_OK`, the consumer's [`Connected()`](BBufferConsumer.md#BBufferConsumer_Connected "Connected\(\)") function will be called, to let it know that a new connection is being established. Finally, the producer's `Connect()` function is called to complete the exchange.

#### ProducerType()

`media_type ProducerType();`

Returns the media_type of the media data produced by the node.

#### ProposeFormatChange()

`status_t ProposeFormatChange(media_format* format,  
const media_destination& forDestination);`

Call this function to determine whether or not the destination `forDestination` is prepared to accept buffers in the specified `format`. This function can be especially useful if you want to test various formats to select the best compatible format during a hookup request in which the requested format contains wildcards.

Return Code| Description  
---|---  
`B_OK`.| The proposed format is acceptable to the destination.  
Other errors.| The proposed format is unacceptable, or an error occurred in querying the destination node.  
  
#### SendBuffer()

`status_t SendBuffer(BBuffer* buffer,  
media_destination& destination);`

Call this function to send a buffer of media data to the specified `destination`, which must already be connected to one of your outputs. This is how your `BBufferProducer` object will send data downstream to [`BBufferConsumer`](BBufferConsumer.md "BBufferConsumer")s to which it's connected.

It's your responsibility to ensure that the buffer's header and the data contained in the buffer itself are valid, although `SendBuffer()` will automatically fill out the following header fields for you:

  * `buffer`

  * `for_id`

  * `change_tag`

In particular, be sure that if you're outputting video buffers you set the media_video_buffer to describe the video properly. If you don't, things will go badly for you.

You can obtain a buffer to fill and send by calling [`BBufferConsumer::RequestAdditionalBuffer()`](BBufferConsumer.md#BBufferConsumer_RequestAdditionalBuffer "RequestAdditionalBuffer\(\)") on a [`BBufferConsumer`](BBufferConsumer.md "BBufferConsumer") that you own (and that's okay to use for buffers going to the specified destination).

Return Code| Description  
---|---  
`B_OK`.| The buffer was sent without error.  
Port errors.| An error occurred sending the buffer.  
  
#### SendDataStatus()

`status_t SendDataStatus(int32 status,  
media_destination& destination,  
bigtime_t atTime);`

Call this function to inform the specified `destination` whether or not there's data available from your producer node. Specify the appropriate status flag as the `status` argument, and the time at which the status takes effect as the atTime argument.

Possible values for the status argument are:

Constant| Description  
---|---  
`B_DATA_NOT_AVAILABLE`| There aren't any buffers ready for the destination.  
`B_DATA_AVAILABLE`| There are buffers ready for the destination.  
`B_PRODUCER_STOPPED`| The producer has stopped.  
Return Code| Description  
---|---  
`B_OK`.| The status update was sent without error.  
Port errors.| The status update couldn't be delivered.  
  
#### SetBufferGroup()

`virtual status_t SetBufferGroup(const media_source& forSource,  
BBufferGroup* group)  = 0;`

When a client wants a specific [`BBufferGroup`](BBufferGroup.md "BBufferGroup") to be used for a given output `forSource`, it will call this function. You should remember the `group` and use it for all requests for buffers to send on the output forSource (and for no other outputs, unless the client explicitly requests you do so by calling `SetBufferGroup()` for another output source).

If your `BBufferProducer` goes away, or the connection is broken, delete the [`BBufferGroup`](BBufferGroup.md "BBufferGroup") object.

If `group` is `NULL`, you should use whatever [`BBufferGroup`](BBufferGroup.md "BBufferGroup") you wish after disposing of the previous group.

It's okay to pass `group` on to another node upstream from your `BBufferProducer` if your `BBufferProducer` only passes along buffers it receives in its processing loop; in that case, you're not really the owner of the [`BBufferGroup`](BBufferGroup.md "BBufferGroup"), unless you pass `true` for `willReclaim` in the call to [`BBufferConsumer::SetOutputBuffersFor()`](BBufferConsumer.md#BBufferConsumer_SetOutputBuffersFor "SetOutputBuffersFor\(\)").

Return `B_OK` if the buffer group is set without incident; otherwise, return an appropriate error code.

#### SetInitialLatency()

`void SetInitialLatency(bigtime_t initialLatency,  
uint32 flags)  = 0;`

If your node has additional startup latency imposed by the signal from which its buffers are constructed, you should call `SetInitialLatency()` to specify the maximum possible latency that can be added by this delay. `initialLatency` should be the maximum latency, in microseconds, that might occur.

One situation in which this occurs is for TV capture card nodes. An NTSC television signal broadcasts a new field about every sixtieth of a second, which means that if your node is started partway through one field being received, you might have to wait as long as a sixtieth of a second for the first complete frame to arrive. So the maximum latency in this situation is a sixtieth of a second.

Setting the initial latency correctly can prevent consumers from having problems synchronizing with your node, and can improve performance.

`flags` should be 0 for now; there are no values defined yet.

#### SetPlayRate()

`virtual status_t SetPlayRate(int32 numerator,  
int32 denominator);`

This function is called to tell the producer to resample the data rate by the specified factor. Specifying a value of 1 (ie, `numerator`/`denominator` = 1) indicates that the data should be output at the same playback rate that it comes into the node at. The format of the data should be unchanged.

For example, if you're playing a sound at 48 kHz, and you receive a call to `SetPlayRate()` with a `numerator` of 2 and a `demoninator` of 1 (double speed), you should resample so that you move twice as fast through the source data while keeping the output rate constant. You might do this by doing a brute-force resample to 24 kHz (which would result in twice the data rate) or do time-compression (which would retain the pitch).

As another example, if you're playing video at 30 frames per second, and your `SetPlayRate()` function is called with a ratio of 1:2 specified (half speed), you should continue sending 30 frames per second, but you need to arrange for the playback to look like half-speed. A reasonable way to do this would be to send each frame twice (re-time-stamped and buffered internally, if necessary), which would result in the desired half-speed appearance.

Return `B_OK` if the sampling rate is changed; otherwise, return an error. It's okay to return an error if you don't support varying sampling rates—the Media Kit won't hold that against you.

#### VideoClippingChanged()

`virtual status_t VideoClippingChanged(const media_source& forSource,  
int16 numShorts,  
int16* clipData,  
const media_video_display_info& display,  
int32* outFromChangeTag)  = 0;`

This hook function is called when a client wants your `BBufferProducer` to output video data clipped to a particular region. Your producer must remember this clipping region and apply it to all video data you produce, without altering any bytes outside the region in any buffers sent through the source `forSource`.

Before your implementation of `VideoClippingChanged()` returns, you should set the value pointed to by `outFromChangeTag` to the change tag value at which the clipping will take effect, so the client will know what buffers it can expect to have the requested clipping. This can be done easily by adding the following line to your implementation:
    
    
    *outFromChangeTag = UpdateChangeTag();

You can use the [`ClipDataToRegion()`](BBufferProducer.md#BBufferProducer_ClipDataToRegion "ClipDataToRegion\(\)") function to convert the data in `clipData` into an actual [`BRegion`](BRegion.md "BRegion") if that's a better format for you to work with. If you do, keep in mind that `numShorts` is the actual number of int16 values in the array specified by `clipData`, while [`ClipDataToRegion()`](BBufferProducer.md#BBufferProducer_ClipDataToRegion "ClipDataToRegion\(\)") requires the number of bytes of data in the array; be sure to multiply `numShorts` by `sizeof`(int16).

The media_video_display_info structure referred to by display indicates the format of the video display onto which the video is being displayed; this lets you know what color space, screen size, and so forth is in use on the video display, so your producer can render properly. `VideoClippingChanged()` is called not only when clipping changes, but when the configuration of the display changes as well. Your producer must abide by this starting at the specified change count.

See "[Video Clipping](BBufferProducer_Overview.md#BBufferProducer_Overview_VideoClipping "Video Clipping")" for information on the format of the clip data.

* * *

### Constants

####  Clipping Data Formats

Declared in: `media/BufferProducer.h`

Constant| Description  
---|---  
`B_CLIP_SHORT_RUNS`.| Clipping is encoded using runs of shorts.  
  
This value defines the only clipping format currently supported by `BBufferProducer`. Note that because this constant is a member of the `BBufferProducer` class, if you need to access it from other classes, you must code it as `BBufferProducer`::`B_CLIP_SHORT_RUNS`. See "[Video Clipping](BBufferProducer_Overview.md#BBufferProducer_Overview_VideoClipping "Video Clipping")" for a description of this format.

####  suggestion_quality

Declared in: `media/BufferProducer.h`

Constant| Description  
---|---  
`B_ANY_QUALITY`| Any quality.  
`B_LOW_QUALITY`| A low quality level (10).  
`B_MEDIUM_QUALITY`| Medium quality level (50).  
`B_HIGH_QUALITY`| High quality level (100).  
  
Quality values you can use when you don't want to have to come up with one on your own.

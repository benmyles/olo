<!-- Classes And Methods - The Media Kit - BMediaRoster -->

## BMediaRoster

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `media/MediaRoster.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BMediaRoster_Overview.md "BMediaRoster")

  * [Constructor and Destructor](BMediaRoster.md#BMediaRoster_ConstructorDestructor)
  * [Member Functions](BMediaRoster.md#BMediaRoster_MemberFunctions)
  * [Constants](BMediaRoster.md#BMediaRoster_Constants)

  
  
* * *

### Constructor and Destructor

#### BMediaRoster()

`BMediaRoster();`

You never construct a `BMediaRoster` yourself. Instead, use the static [`Roster()`](BMediaRoster.md#BMediaRoster_Roster "Roster\(\), CurrentRoster\(\)") function to obtain an instance of the `BMediaRoster` class that you can use.

#### ~BMediaRoster()

`virtual ~BMediaRoster();`

You never delete a `BMediaRoster` yourself. Just let it go away automatically when your application shuts down.

* * *

### Member Functions

#### AudioBufferSizeFor()

`ssize_t AudioBufferSizeFor(int32 channelCount,  
uint32 sampleFormat,  
float frameRate,  
bus_type busKind);`

`AudioBufferSizeFor()` returns the size, in bytes, that the Media Kit recommends for audio data with `channelCount` channels, with the specified `sampleFormat` and `frameRate`.

The `busKind` argument is a bus_type value (see `drivers/config_manager.h`) indicating the type of bus the data is moving across. Specify `B_UNKNOWN_BUS` if you don't know.

####  Connect(), Disconnect()

`status_t Connect(const media_source& source,  
const media_destination& destination,  
media_format* ioFormat,  
media_output* outOutput,  
media_input* outInput);` `status_t Connect(const media_source& source,  
const media_destination& destination,  
media_format* ioFormat,  
media_output* outOutput,  
media_input* outInput,  
uint32 inFlags,  
void* _reserved = NULL);`

`status_t Disconnect(media_node_id sourceNode,  
const media_source& source,  
media_node_id destinationNode,  
const media_destination& destination);`

`Connect()` negotiates a connection from the `source` to the `destination`, using the media format specified in `ioFormat` as a basis for the negotiation; `ioFormat` is changed to the negotiated format before this call returns. This describes the format of media data that will flow across the connection.

The actual connection is returned as an output and an input in `outOutput` and `outInput`. These two structures contain the data format as interpreted by the source and destination. There may be differences among these formats if wildcard fields were used in the original format.

The second form of `Connect()` lets you specify connect flags. Currently the only possible flag is `B_CONNECT_MUTED`, which indicates that the connection should be muted on creation.

The actual [media_source](TheMediaKit_DefinedTypes.md#media_source "media_source") and [media_destination](TheMediaKit_DefinedTypes.md#media_destination "media_destination") used for the connection may vary from those passed into `Connect()` if the source or the destination creates new sources or destinations for each connection request; the `outOutput` and `outInput` structures contain the actual [media_source](TheMediaKit_DefinedTypes.md#media_source "media_source") and [media_destination](TheMediaKit_DefinedTypes.md#media_destination "media_destination") values resulting from the call.

For more detailed information on the use of wildcards in format negotiation, see media_audio_format::wildcard and media_video_format::wildcard. A [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") with a type of `B_MEDIA_UNKNOWN_TYPE` matches any media class and format, although without specific knowledge of the source and destination, this will rarely result in a useful connection.

`Disconnect()` breaks the connection established between `source` and `destination`, which must belong to the nodes `sourceNode` and `destinationNode`, repectively.

The result of breaking a connection that's currently running is undefined, but is not permitted to crash. Your application should stop both nodes involved in a connection prior to disconnecting them.

Note

![Note](./images/admonitions/Info_32.png)

These functions will deadlock if called from a node's control thread or while the control thread is blocked.

Return Code| Description  
---|---  
`B_OK`| No error terminating the connection.  
`B_NAME_NOT_FOUND`| The connection couldn't be made.  
Other errors.| The nodes that are being connected may return other error codes as they see fit.  
  
####  GetAllInputsFor(), GetAllOutputsFor()

`status_t GetAllInputsFor(const media_node& node,  
media_input* outInputs,  
int32 bufNumInputs,  
int32* outTotalCount);``status_t GetAllOutputsFor(const media_node& node,  
media_output* outOutputs,  
int32 bufNumOutputs,  
int32* outTotalCount);`

`GetAllInputsFor()` fills the array of [media_input](TheMediaKit_DefinedTypes.md#media_input "media_input") structures specified by `outInputs` with information about all inputs belonging to node; the number of elements that `outInputs` can hold is passed in `bufNumInputs`.

Similarly, `GetAllOutputsFor()` fills the array `outOutputs` with information about all outputs from the specified node.

Both functions return the number of elements actually returned in the buffer in `outTotalCount`. If this number is less than the number you requested, your buffer was too small to receive all the results of the query. In this case, you might want to resize your array and try again.

Note

![Note](./images/admonitions/Info_32.png)

These functions will deadlock if called from a node's control thread or while the control thread is blocked.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_MEDIA_BAD_NODE`| The node isn't of the correct type for the call you issued.  
Other errors.| An error occurred communicating with the producer or with the Media Server.  
  
####  GetAudioInput(), GetVideoInput()

`status_t GetAudioInput(media_node* outNode);``status_t GetVideoInput(media_node* outNode);`

These functions return the nodes designated by the user as the preferred nodes for audio and video input. You can then query the returned node, hook into it, and manipulate it, using the reference returned in `outNode`.

Once your application has finished using these nodes (and they've been stopped and disconnected), you should release them by calling `ReleaseNode()`.

Return Code| Description  
---|---  
`B_OK`| No error locating the default input node.  
`B_NAME_NOT_FOUND`| The default node couldn't be identified.  
  
####  GetAudioOutput(), GetVideoOutput(), GetAudioMixer()

`status_t GetAudioOutput(media_node* outNode);` `status_t GetAudioOutput(media_node* outNode,  
int32* outInputID,  
BString* outInputName);`

`status_t GetVideoOutput(media_node* outNode);``status_t GetAudioMixer(media_node* outNode);`

These functions return the nodes designated by the user as the preferred nodes for audio and video output. You can then query the returned node, hook into it, and manipulate it, using the reference returned in `outNode`.

The second form of `GetAudioOutput()` returns additional information, including the input ID of the input used for audio output, and the input's name.

You should usually use `GetAudioMixer()` when getting a node for playing audio instead of using the `GetAudioOutput()` function. `GetAudioOutput()` returns the lower-level node for audio output, which you would typically only need access to if you wanted to do some form of processing on all audio data being played in the system (such as a level meter).

The `GetAudioMixer()` function returns a reference to the audio mixer, which will perform audio mixing, format conversion, and sample rate conversion for you, then pass along the audio to the output node.

Once your application has finished using these nodes (and they've been stopped and disconnected), you should release them by calling [`ReleaseNode()`](BMediaRoster.md#BMediaRoster_ReleaseNode "ReleaseNode\(\)").

Return Code| Description  
---|---  
`B_OK`| No error locating the default input node.  
`B_NAME_NOT_FOUND`| The default node couldn't be identified.  
  
####  GetConnectedInputsFor(), GetFreeInputsFor()

`status_t GetConnectedInputsFor(const media_node& node,  
media_input* outActiveInputsList,  
int32 numListInputs,  
int32* outNumInputs);``status_t GetFreeInputsFor(const media_node& node,  
media_input* outFreeInputsList,  
int32 numListInputs,  
int32* outNumInputs,  
media_type filterType = B_MEDIA_UNKNOWN_TYPE);`

`GetConnectedInputsFor()` fills the array of [media_input](TheMediaKit_DefinedTypes.md#media_input "media_input") structures specified by `outActiveInputsList` with information about all inputs belonging to node that are currently connected to some output; the number of elements that `outActiveInputsList` can hold is passed in `numListInputs`.

Similarly, `GetFreeInputsFor()` fills the array `outFreeInputsList` with information about all inputs that are still available in the specified node. Specifying a `filterType` other than `B_MEDIA_NO_TYPE` lets you obtain a list of inputs for a specific media type (or for inputs that can handle any media type). This is especially useful if you're only interested in a list of accepted media types your application supports.

Note

![Note](./images/admonitions/Info_32.png)

Even though a node may report that a specific number of free inputs are available, it is possible that a node might create more inputs on demand. There is no way to know if this might happen, so `GetFreeInputsFor()` may not tell you whether or not a node can accept all the connections you'd like to make.

Both functions return the number of elements actually returned in the buffer in `outNumInputs`. If this number is less than `numListInputs`, your buffer was too small to receive all the results of the query. In this case, you might want to resize your array and try again.

These functions will deadlock if called from a node's control thread or while the control thread is blocked.

Return Code| Description  
---|---  
`B_OK`| No error cloning the node.  
`B_NAME_NOT_FOUND`| The requested node couldn't be cloned.  
  
####  GetConnectedOutputsFor(), GetFreeOutputsFor()

`status_t GetConnectedOutputsFor(const media_node& node,  
media_output* outActiveOutputsList,  
int32 numListOutputs,  
int32* outNumOutputs);``status_t GetFreeOutputsFor(const media_node& node,  
media_output* outFreeOutputsList,  
int32 numListOutputs,  
int32* outNumOutputs,  
media_type filterType = B_MEDIA_UNKNOWN_TYPE);`

`GetConnectedOutputsFor()` fills the array of [media_output](TheMediaKit_DefinedTypes.md#media_output "media_output") structures specified by `outActiveOutputsList` with information about all outputs belonging to node that are currently connected to some input; the number of elements that `outActiveOutputsList` can hold is passed in `numListOutputs`.

Similarly, `GetFreeOutputsFor()` fills the array `outFreeOutputsList` with information about all outputs that are still available in the specified node. Specifying a `filterType` other than `B_MEDIA_UNKNOWN_TYPE` lets you obtain a list of outputs for a specific media type (or for outputs that can handle any media type). This is especially useful if you're only interested in a list of accepted media types your application supports.

Note

![Note](./images/admonitions/Info_32.png)

Even though a node may report that a specific number of free outputs are available, it is possible that a node might create more outputs on demand. There is no way to know if this might happen, so `GetFreeOutputsFor()` may not tell you whether or not a node can accept all the connections you'd like to make.

Both functions return the number of elements actually returned in the buffer in `outNumOutputs`. If this number is less than `numListOutputs`, your buffer was too small to receive all the results of the query. In this case, you might want to resize your array and try again.

Note

![Note](./images/admonitions/Info_32.png)

These functions will deadlock if called from a node's control thread or while the control thread is blocked.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_MEDIA_BAD_NODE`| The node isn't a buffer producer.  
Other errors.| An error occurred communicating with the producer or with the Media Server.  
  
#### GetDormantFlavorInfoFor()

`status_t GetDormantFlavorInfoFor(const dormant_node_info& inDormantNode,  
dormant_flavor_info* outFlavor);`

This function returns, in `outFlavor`, information describing the dormant flavors supported by the dormant node `inDormantNode`.

Return Code| Description  
---|---  
`B_OK`| No error.  
Messaging errors.| An error occurred communicating with the Media Server.  
  
#### GetDormantNodes()

`status_t GetDormantNodes(dormant_node_info* outDormantNodeList,  
int32* inOutNumNodes,  
const media_format* hasInputFormat = NULL,  
const media_format* hasOutputFormat = NULL,  
char* name = NULL,  
uint64 requireKinds = 0,  
uint64 denyKinds = 0);`

Queries dormant nodes (those nodes that live in add-ons, rather than in the application) and returns those who match the specified inputs. If `hasInputFormat` isn't `NULL`, the node has to be a [`BBufferConsumer`](BBufferConsumer.md "BBufferConsumer") and have an input format compatible with the format described in `hasInputFormat`. Likewise, if `hasOutputFormat` isn't `NULL`, the node has to be a [`BBufferProducer`](BBufferProducer.md "BBufferProducer") that's compatible with the format described in `hasOutputFormat.`

If name isn't `NULL`, the node has to have a name that equals `name`, or, if the last character of name is an asterisk ("*"), a name whose initial characters match name up to, but not including, the asterisk.

The `requireKinds` and `denyKinds` arguments specifiy, respectively, the kinds that must be supported, and the kinds that must not be supported by the returned nodes.

Matching nodes are returned in `outDormantNodeList`. You should pass the size of the `outDormantNodeList` array (the number of elements that the array can hold) in `inOutNumNodes`; when this function returns, the value in `inOutNumNodes` will be changed to the actual number of matching nodes found, unless an error occurs.

Nodes you obtain using `GetDormantNodes()` must be released when you're done using them. To do this, be sure they're stopped and disconnected, then call [`ReleaseNode()`](BMediaRoster.md#BMediaRoster_ReleaseNode "ReleaseNode\(\)").

Return Code| Description  
---|---  
`B_OK`| No error cloning the node.  
`B_NAME_NOT_FOUND`| The requested node couldn't be cloned.  
  
#### GetFileFormatsFor()

`status_t GetFileFormatsFor(const media_node& fileInterface,  
media_file_format* outFormatList,  
int32* inOutFormatCount = 0);`

Given a [`BFileInterface`](BFileInterface.md "BFileInterface") node in `fileInterface`, returns information about the file formats the file interface can deal with in the array `outFormatList`. On entry, `inOutFormatCount` points to the number of [media_file_format](TheMediaKit_DefinedTypes.md#media_file_format "media_file_format") structures that can fit in the array specified by `outFormatList`. Upon return, it will contain the actual number of formats returned, unless `GetFileFormatsFor()` returns an error.

Note

![Note](./images/admonitions/Info_32.png)

This function will deadlock if called from a node's control thread or while the control thread is blocked.

Return Code| Description  
---|---  
`B_OK`| No error sending the set mode request.  
`B_NAME_NOT_FOUND`| The requested node couldn't be cloned.  
  
#### GetFormatFor()

`status_t GetFormatFor(const media_output& output,  
media_format* ioFormat,  
uint32 flags = 0);` `status_t GetFormatFor(const media_input& input,  
media_format* ioFormat,  
uint32 flags = 0);` `status_t GetFormatFor(const media_node& input,  
media_format* ioFormat,  
float quality = B_MEDIA_ANY_QUALITY);`

`GetFormatFor()` returns the [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") being used by the given object, which may be a [media_output](TheMediaKit_DefinedTypes.md#media_output "media_output"), a [media_input](TheMediaKit_DefinedTypes.md#media_input "media_input"), or a [media_node](TheMediaKit_DefinedTypes.md#media_node "media_node"). Pass in `ioFormat` a pointer to a [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") object to be filled out with the object's format.

The `flags` currently must be zero.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_BAD_VALUE`| You can't pass `NULL` for `ioFormat`.  
`B_MEDIA_BAD_NODE`| The node isn't of the correct type for the given request.  
`B_MEDIA_BAD_SOURCE`| The node's [media_source](TheMediaKit_DefinedTypes.md#media_source "media_source") is invalid.  
Messaging errors.| An error occurred communicating with the Media Server.  
  
#### GetInitialLatencyFor()

`status_t GetInitialLatencyFor(media_node& producer,  
bigtime_t* outLatency,  
uint32* outFlags = NULL);`

Returns, in `outLatency`, the additional amount of time in microseconds the specified producer node requires in order to synchronize to a signal. For example, a TV capture card that's started while the capture is in the middle of a field will have to wait until the next field begins before actually starting to produce buffers.

`outFlags` is set to the flags returned by the producer. Currently there aren't any flags defined, so this will be returned as zero for now.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_BAD_VALUE`| `outLatency` was specified as `NULL`.  
`B_MEDIA_BAD_NODE`| `producer` isn't a valid node.  
Port errors.| An error occurred communicating with the producer.  
  
#### GetInstancesFor()

`status_t GetInstancesFor(media_addon_id addon,  
int32 flavor,  
media_node_id* outID,  
int32* ioCount = 0);`

Given the specified `addon` ID and `flavor`, this function fills the `outID` list with up to `ioCount` node IDs that were derived from the specified add-on. If you specify zero for `ioCount`, one node ID will be returned. On return, `ioCount` is changed to indicate how many nodes have been returned in the list.

Return Code| Description  
---|---  
`B_OK`| No error.  
Port errors.| Communication with the Media Server failed.  
  
#### GetLatencyFor()

`status_t GetLatencyFor(const media_node& producer,  
bigtime_t* outLatency);`

Reports in `outLatency` the maximum latency found downstream from the specified [`BBufferProducer`](BBufferProducer.md "BBufferProducer"), `producer`, given the current connections.

If an error occurs, the value in `outLatency` is unreliable.

Note

![Note](./images/admonitions/Info_32.png)

This function will deadlock if called from a node's control thread or while the control thread is blocked.

Return Code| Description  
---|---  
`B_OK`| No error.  
Other errors.| Unable to get the latency.  
  
#### GetLiveNodes()

`status_t GetLiveNodes(live_node_info* outLiveNodeList,  
int32* ioTotalCount,  
const media_format* hasInput = NULL,  
const media_format* hasOutput = NULL,  
const char* name = NULL,  
uint64 nodeKinds = 0);`

Queries the Media Server for a list of all currently active nodes (whether they're running or not), and fills the array specified by `outLiveNodeList` with information about the nodes. The size of the array should be specified—in terms of how many elements it can contain—by the `ioTotalCount` argument; the actual number of entries in the returned list will be stored in `ioTotalCount` before the call returns.

An active node is a node that is preloaded by the system and is always available for use, as opposed to a dormant node, which resides in an add-on and is only loaded when instantiated using [`InstantiateDormantNode()`](BMediaRoster.md#BMediaRoster_InstantiateDormantNode "InstantiateDormantNode\(\)").

You can obtain a more specific result list by specifying one or more of the `hasInput`, `hasOutput`, `name`, and `nodeKinds` arguments. `hasInput` and `hasOutput` let you restrict the resulting list to containing nodes that accept as input (or output) the specified format.

You should always specify 0 for `nodeKinds`; this parameter is currently not used.

Return Code| Description  
---|---  
`B_OK`| No error.  
Other errors.| Unable to get the list of live nodes.  
  
#### GetNodeAttributesFor()

`ssize_t GetNodeAttributesFor(const media_node& node,  
media_node_attributes* outArray,  
size_t inMaxCount);`

Fills the array `outArray` with up to `inMaxCount` attributes of the given node. Returns the number of attributes returned. If the result is less than zero, an error occurred.

#### GetNodeFor()

`status_t GetNodeFor(media_node_id nodeID,  
media_node* clonedNode);`

Given a node specified by `node_id`, `GetNodeFor()` returns in `clonedNode` a [media_node](TheMediaKit_DefinedTypes.md#media_node "media_node") reference to a clone of the node. You can then use the `clonedNode` to query the node for available inputs, outputs, and so forth.

Once your application has finished using the returned node (and it's been stopped and disconnected), you should release it by calling [`ReleaseNode()`](BMediaRoster.md#BMediaRoster_ReleaseNode "ReleaseNode\(\)").

Return Code| Description  
---|---  
`B_OK`| No error cloning the node.  
[`BMessage`](BMessage.md "BMessage") errors.| Unable to get the list of live nodes.  
  
#### GetParameterWebFor()

`status_t GetParameterWebFor(const media_node& node,  
[`BParameterWeb`](BParameterWeb.md "BParameterWeb")** outWeb);`

Instantiates a [`BParameterWeb`](BParameterWeb.md "BParameterWeb") that describes the internal layout of a specific controllable node and stores a pointer to the [`BParameterWeb`](BParameterWeb.md "BParameterWeb") in `outWeb`. You can then walk the various [`BParameter`](BParameter.md "BParameter")s within the web to figure out what there is to control, and to present a user interface to the node's parameters. Delete the web pointed to by `outWeb` when you're done with it.

Note that the [`StartControlPanel()`](BMediaRoster.md#BMediaRoster_StartControlPanel "StartControlPanel\(\)") function provides an easy, painless way to automatically present user interface for configuring nodes.

Note

![Note](./images/admonitions/Info_32.png)

This function will deadlock if called from a node's control thread or while the control thread is blocked.

Return Code| Description  
---|---  
`B_OK`| No error obtaining the [`BParameterWeb`](BParameterWeb.md "BParameterWeb").  
`B_MEDIA_BAD_NODE`| The requested node is invalid.  
  
####  GetReadFileFormatListFor(), GetWriteFileFormatListFor()

`status_t GetReadFileFormatList(const dormant_node_info& inNode,  
media_file_format* outReadFormats,  
int32 inReadCount,  
int32* outReadCount);``status_t GetWriteFileFormatList(const dormant_node_info& inNode,  
media_file_format* outWriteFormats,  
int32 inWriteCount,  
int32* outWriteCount);`

These two functions return lists of file formats that the dormant node described by `inNode` can read or write.

`GetReadFileFormatList()` returns in the array specified by `outReadFormats` a list of file formats the node can read. Specify in `inReadCount` the number of formats that can be held by the `outReadFormats` array. On exit, `outReadCount` indicates how many formats are being returned in the array.

`GetWriteFileFormatList()` returns in the array specified by `outWriteFormats` a list of file formats the node can write. Specify in `inWriteCount` the number of formats that can be held by the `outWriteFormats` array. On exit, `outWriteCount` indicates how many formats are being returned in the array.

Return Code| Description  
---|---  
`B_OK`| The list was returned without error.  
`B_BAD_VALUE`| `outReadFormats` or `outWriteFormats` is `NULL`.  
[`BMessage`](BMessage.md "BMessage") errors.| An error occurred communicating with the Media Server.  
  
####  GetRealtimeFlags(), SetRealtimeFlags()

`status_t GetRealtimeFlags(uint32* outEnabled);``status_t SetRealtimeFlags(uint32 inEnabled);`

`GetRealtimeFlags()` returns flags that the Media Server uses to determine whether or not memory needs to be locked down. `SetRealtimeFlags()` sets these flags, and is generally only called by the Media preference application.

Any or all of these flags can be set, in combination.

Constant| Description  
---|---  
`B_MEDIA_REALTIME_ALLOCATOR`|  When set, [`rtm_alloc()`](TheMediaKit_RealTimeFunctions.md#rtm_alloc "rtm_alloc\(\), rtm_realloc\(\)") will return locked memory.  
`B_MEDIA_REALTIME_AUDIO`| Audio add-ons in the Media Server are locked in memory, and should lock their thread stacks using [`media_realtime_init_thread()`](TheMediaKit_Functions.md#media_realtime_init_thread "media_realtime_init_thread\(\)").  
`B_MEDIA_REALTIME_VIDEO`| Video add-ons are locked in memory, and should lock their thread stacks using [`media_realtime_init_thread()`](TheMediaKit_Functions.md#media_realtime_init_thread "media_realtime_init_thread\(\)").  
`B_MEDIA_REALTIME_ANYKIND`| All Media add-ons are locked in memory, and should lock their thread stacks using [`media_realtime_init_thread()`](TheMediaKit_Functions.md#media_realtime_init_thread "media_realtime_init_thread\(\)").  
  
See the [`BMediaNode` overview](BMediaNode_Overview.md "BMediaNode") for a discussion of realtime allocation and thread stack locking.

Return Code| Description  
---|---  
`B_OK`| No error.  
Other errors.| Unable to set or retrieve the realtime flags.  
  
#### GetStartLatencyFor()

`status_t GetStartLatencyFor(const media_node& timeSource,  
bigtime_t* outLatency);`

Reports in `outLatency` the maximum latency found downstream from the time source specified by `timeSource`, given the current connections.

If an error occurs, the value in `outLatency` is unreliable.

Note

![Note](./images/admonitions/Info_32.png)

This function will deadlock if called from a node's control thread or while the control thread is blocked.

Return Code| Description  
---|---  
`B_OK`| No errors.  
Other errors.| Unable to get the latency.  
  
#### GetSystemTimeSource()

`status_t GetSystemTimeSource(media_node* clonedTimeSource);`

This function returns, in `clonedTimeSource`, a reference to a clone of the system time source. The system time source is the fallback time source used when no other source is available; its time is derived from the [`system_time()`](TheKernelKit_Time.md#system_time "system_time\(\)") real-time clock. As such, it's quite accurate, but has no relevant relationship to the timing of the hardware devices being used for media input and output. Thus it's not a good choice for a master clock—but it's there if nothing else is available.

By default, new nodes are slaved to the system time source.

Return Code| Description  
---|---  
`B_OK`| No error cloning the node.  
`B_NAME_NOT_FOUND`| The time source couldn't be cloned.  
  
####  GetTimeSource(), MakeTimeSourceFor()

`status_t GetTimeSource(media_node* outNode);``[`BTimeSource`](BTimeSource.md "BTimeSource")* MakeTimeSourceFor(media_node& node);`

`GetTimeSource()` returns, in `outNode`, the preferred master clock to which other nodes can be slaved. By slaving all nodes to a single master clock, good synchronization can be ensured.

Typically, the preferred master clock will be the same node as the default audio output (assuming that the audio output node is also a [`BTimeSource`](BTimeSource.md "BTimeSource"), which should be the case). The sound circuitry's DAC is then used as a timing reference. Although this may be less accurate than the system clock (as defined by the global [`system_time()`](TheKernelKit_Time.md#system_time "system_time\(\)") function), glitch-free audio performance is best ensured by using the audio output to synchronize media operations.

By default, nodes are slaved to the system time source (see [`GetSystemTimeSource()`](BMediaRoster.md#BMediaRoster_GetSystemTimeSource "GetSystemTimeSource\(\)") above). Usually you'll want to use this function to obtain a more accurate time source, then slave your nodes to it:
    
    
    media_node timeSource;
    roster->GetTimeSource(&media_node);
    roster->SetTimeSourceFor(myNode, timeSource.node);

This will slave the previously-created node myNode to the preferred time source.

`MakeTimeSourceFor()` returns a [`BTimeSource`](BTimeSource.md "BTimeSource") object corresponding to the specified node's time source. This object can then be used to issue [`BTimeSource`](BTimeSource.md "BTimeSource") calls to determine and adjust timing issues (for instance, to determine the current performance time). When you're done with the [`BTimeSource`](BTimeSource.md "BTimeSource"), you should call [`ReleaseNode()`](BMediaRoster.md#BMediaRoster_ReleaseNode "ReleaseNode\(\)") on it.

Return Code| Description  
---|---  
`B_OK`| No error locating the default input node.  
`B_NAME_NOT_FOUND`| The default node couldn't be identified.  
  
#### InstantiateDormantNode()

`status_t InstantiateDormantNode(const dormant_node_info& inInfo,  
media_node* outNode);` `status_t InstantiateDormantNode(const dormant_node_info& inInfo,  
media_node* outNode,  
uint32 flags);`

These functions instantiate a node from an add-on, given the information specified in the dormant_node_info structure.

The `addon` field should be filled out to contain the add-on ID of the add-on from which the node should be instantiated, and the `flavor_id` should be the flavor ID number the node should be instantiated to process. Typically you'll use a function such as [`GetDormantNodes()`](BMediaRoster.md#BMediaRoster_GetDormantNodes "GetDormantNodes\(\)") to find a dormant_node_info structure that describes a suitable node.

When you're done using the node, and have stopped and disconnected it, you should always call [`ReleaseNode()`](BMediaRoster.md#BMediaRoster_ReleaseNode "ReleaseNode\(\)") to let the Media Server know you're finished with it. This lets the Media Server track whether or not the node's add-on can be unloaded, based on the number of applications still using it.

The difference between these two functions is that the second form lets you specify flags controlling how the node is instantiated. The `B_FLAVOR_IS_GLOBAL` flag instantiates the node in the Media Add-on Server's memory space, while the `B_FLAVOR_IS_LOCAL` flag instantiates the node in your application's memory. Using `B_FLAVOR_IS_LOCAL` protects other applications—not to mention the Media Server—from being derailed if the node crashes. Whenever possible, you should instantiate nodes locally. You should only use `B_FLAVOR_IS_GLOBAL` if you need the node to stay around after your application exits.

Return Code| Description  
---|---  
`B_OK`| No error instantiating the node.  
`B_NAME_NOT_FOUND`| The requested node couldn't be instantiated.  
  
#### MediaFlags()

`static ssize_t MediaFlags(media_flags flag,  
void* buffer,  
size_t bufferSize);`

Asks the Media Server about its support for specific features and capabilities.

The specified buffer will be filled with the data indicating the value of the specified `flag`. If the buffer is too small (as indicated by `bufferSize`), only the first `bufferSize` bytes of the result data will be stored in the buffer, but no error will occur.

Constant| Description  
---|---  
`B_MEDIA_FLAGS_VERSION`| Returns the Media Kit version as an int32 value.  
  
If the result is negative, an error occurred, or the Media Server isn't running.

#### NodeIDFor()

`media_node_id NodeIDFor(port_id sourceOrDestinationPort);`

Given a source or destination port, this function returns the corresponding node's ID number.

#### PrerollNode()

`status_t PrerollNode(const media_node& node);`

Calling `PrerollNode()` sends a preroll message to the specified `node`; the node's [`Preroll()`](BMediaNode.md#BMediaNode_Preroll "Preroll\(\)") hook function will be called. When that hook returns, `PrerollNode()` will also return. A node that's been prerolled should respond very quickly to a [`StartNode()`](BMediaRoster.md#BMediaRoster_StartNode "StartNode\(\), StopNode\(\)") call, because the time-consuming setup operations should have been done already by the [`Preroll()`](BMediaNode.md#BMediaNode_Preroll "Preroll\(\)") hook.

While it's not mandatory for an application to call `PrerollNode()` before calling [`StartNode()`](BMediaRoster.md#BMediaRoster_StartNode "StartNode\(\), StopNode\(\)"), it's recommended, because doing so may improve real-time performance once the node is started.

Note

![Note](./images/admonitions/Info_32.png)

This function will deadlock if called from a node's control thread or while the control thread is blocked.

Return Code| Description  
---|---  
`B_OK`| No error sending the set mode request.  
`B_NAME_NOT_FOUND`| The requested node couldn't be cloned.  
  
####  RegisterNode(), UnregisterNode()

`status_t RegisterNode([`BMediaNode`](BMediaNode.md "BMediaNode")* node);``status_t UnregisterNode([`BMediaNode`](BMediaNode.md "BMediaNode")* node);`

`RegisterNode()` registers an object of a class derived from [`BMediaNode`](BMediaNode.md "BMediaNode") with the media roster and assigns it a node_id. This function should be called once a `BMediaNode`-derived object is fully-constructed and before any attempt is made to connect the node to some other participant in the Media Server.

`RegisterNode()` is called automatically for nodes instantiated from add-ons, but your application will have to call it for any nodes it creates itself.

If you create your own subclass of [`BMediaNode`](BMediaNode.md "BMediaNode"), its constructor can call `RegisterNode()` itself just before returning (it must be the last thing the constructor does).

`UnregisterNode()` unregisters a node from the Media Server. It's called automatically by the [`BMediaNode`](BMediaNode.md "BMediaNode") destructor, but it might be convenient to call it sometime before you delete your node instance, depending on your implementation and circumstances.

These functions are generally only used if you're creating your own node class.

Return Code| Description  
---|---  
`B_OK`| No error sending the seek request.  
`B_BAD_VALUE`| Invalid [`BMediaNode`](BMediaNode.md "BMediaNode") specified.  
  
#### ReleaseNode()

`status_t ReleaseNode(const media_node& node);`

Releases the specified node, which has previously been obtained by using the [`InstantiateDormantNode()`](BMediaRoster.md#BMediaRoster_InstantiateDormantNode "InstantiateDormantNode\(\)"), [`GetNodeFor()`](BMediaRoster.md#BMediaRoster_GetNodeFor "GetNodeFor\(\)"), or default node functions (such as [`GetVideoOutput()`](BMediaRoster.md#BMediaRoster_GetVideoOutput) or [`GetAudioMixer()`](BMediaRoster.md#BMediaRoster_GetAudioMixer)).

Return Code| Description  
---|---  
`B_OK`| No error releasing the node.  
[`BMessage`](BMessage.md "BMessage") errors.| The node couldn't be released.  
  
#### RollNode()

`status_t RollNode(const media_node& node,  
bigtime_t startPerformanceTime,  
bigtime_t stopPerformanceTime,  
bigtime_t atMediaTime = -B_INFINITE_TIMEOUT);`

Atomically queues a start and stop for the given node. The node will start playing at the performance time indicated by `startPerformanceTime`, and will stop playing at the performance time indicated by `stopPerformanceTime`.

If the `atMediaTime` argument is given, a seek to that media time is also queued.

This function is especially useful for the offline rendering case (the `B_OFFLINE` run mode). It lets you render a certain time range without accidentally going too far; if you queue up a start and stop using [`Start()`](BMediaNode.md#BMediaNode_Start "Start\(\)") and [`Stop()`](BMediaNode.md#BMediaNode_Stop "Stop\(\)"), the node may have already rendered past your desired stop time before your [`Stop()`](BMediaNode.md#BMediaNode_Stop "Stop\(\)") call occurs. [`RollNode()`](BMediaRoster.md#BMediaRoster_RollNode "RollNode\(\)") avoids that problem.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_MEDIA_BAD_NODE`| The node isn't valid.  
  
####  Roster(), CurrentRoster()

`static BMediaRoster* Roster(status_t* outError = NULL);``static BMediaRoster* CurrentRoster();`

`Roster()` returns a pointer to the default `BMediaRoster` instance, or creates the `BMediaRoster` instance if it doesn't exist yet, then returns a pointer to it. If you don't want to create the roster if it doesn't already exist, use the `CurrentRoster()` function (it returns `NULL` if there's no roster).

Since `CurrentRoster()` doesn't create a media roster, you obviously must use `Roster()` at least once in your application to create one.

These static member functions should be called by explicit scope, and never by dereference; this is how you get the `BMediaRoster` through which all other media roster functions are called. For example:
    
    
    BMediaRoster* r = BMediaRoster::Roster();
    status_t err = r->GetFreeOutputsFor(some_node, some_array, 3, &n);

On return, `outError` is set to `B_OK` if the default `BMediaRoster` was successfully returned, or a negative error code if something went wrong (for example, if the Media Server isn't running). If `outError` is `NULL`, no error code is returned.

In any case, `Roster()` returns `NULL` if an error occurs.

#### SeekNode()

`status_t SeekNode(const media_node& node,  
bigtime_t newMediaTime,  
bigtime_t atPerformanceTime = 0);`

Sends the specified `node` a request that it change its playing location to the media time `newMediaTime` once the performance time `atPerformanceTime` is reached.

If the node isn't running, the seek request is processed immediately, and the `atPerformanceTime` argument is ignored.

The error returned by this function only indicates whether or not the request was sent successfully; the node may later run into problems trying to perform the seek operation.

Note

![Note](./images/admonitions/Info_32.png)

If the node is a time source, and you want to operate on the time source aspect of the node (to seek all slaved nodes), you should call [`SeekTimeSource()`](BMediaRoster.md#BMediaRoster_SeekTimeSource "SeekTimeSource\(\)") instead.

Return Code| Description  
---|---  
`B_OK`| No error sending the seek request.  
Other errors| Are node specific  
  
See also: [`StartNode()`](BMediaRoster.md#BMediaRoster_StartNode "StartNode\(\), StopNode\(\)"), [`StopNode()`](BMediaRoster.md#BMediaRoster_StopNode)

#### SeekTimeSource()

`status_t SeekTimeSource(const media_node& timeSource,  
bigtime_t newPerformanceTime,  
bigtime_t atRealTime);`

Sends the specified timeSource a request that it change the performance time it outputs to its slaved nodes to the time `newPerformanceTime` once the performance time `atRealTime` is reached.

If the `timeSource` isn't running, the seek request is processed immediately, and the atRealTime argument is ignored.

The error returned by this function only indicates whether or not the request was sent successfully; the node may later run into problems trying to perform the seek operation.

Return Code| Description  
---|---  
`B_OK`| No error sending the seek request.  
Other errors| Are node specific  
  
See also: [`StartTimeSource()`](BMediaRoster.md#BMediaRoster_StartTimeSource "StartTimeSource\(\), StopTimeSource\(\)"), [`StopTimeSource()`](BMediaRoster.md#BMediaRoster_StopTimeSource)

####  SetAudioInput(), SetVideoInput()

`status_t SetAudioInput(const media_node& defaultNode);` `status_t SetAudioInput(const dormant_node_info& defaultNodeInfo);`

`status_t SetVideoInput(const media_node& defaultNode);` `status_t SetVideoInput(const dormant_node_info& defaultNodeInfo);`

These functions set the preferred nodes for audio and video input. If the specified node isn't capable of being the system default, an error will be returned (for example, nodes defined by an application can't be the system default—only nodes defined by Media Kit add-ons can be system defaults).

Note

![Note](./images/admonitions/Info_32.png)

In general, you shouldn't call these functions unless you're writing software that reimplements the functionality of the BeOS Audio or Video preference panels.

Return Code| Description  
---|---  
`B_OK`| No error setting the default input node.  
`B_NAME_NOT_FOUND`| The default node couldn't be changed.  
  
####  SetAudioOutput(), SetVideoOutput()

`status_t SetAudioOutput(const media_node& defaultNode);` `status_t SetAudioOutput(const dormant_node_info& defaultNodeInfo);` `status_t SetAudioOutput(const media_input& inputToOutput);`

`status_t SetVideoOutput(const media_node& defaultNode);` `status_t SetVideoOutput(const dormant_node_info& defaultNodeInfo);`

These functions set the preferred nodes for audio and video output. If the specified node isn't capable of being the system default, an error will be returned (for example, nodes defined by an application can't be the system default—only nodes defined by Media Kit add-ons can be system defaults).

Note

![Note](./images/admonitions/Info_32.png)

In general, you shouldn't call these functions unless you're writing software that reimplements the functionality of the BeOS Audio or Video preference panels.

Return Code| Description  
---|---  
`B_OK`| No error setting the default output node.  
`B_NAME_NOT_FOUND`| The default node couldn't be changed.  
  
#### SetProducerRate()

`status_t SetProducerRate(const media_node& node,  
int32 numerator,  
int32 demominator);`

This function is called to tell the producer to resample the data rate by the specified factor. Specifying a value of 1 (ie, `numerator`/`denominator` = 1) indicates that the data should be output at the same playback rate that it comes into the node at. The format of the data should be unchanged.

Note

![Note](./images/admonitions/Info_32.png)

Nodes are not required to support this mechanism for controlling their data rate, so this call may have no effect.

Return Code| Description  
---|---  
`B_OK`| No error.  
Other errors.| Returned by [`BBufferProducer::SetPlayRate()`](BBufferProducer.md#BBufferProducer_SetPlayRate "SetPlayRate\(\)").  
  
#### SetProducerRunModeDelay()

`status_t SetProducerRunModeDelay(const media_node& node,  
bigtime_t delay,  
[BMediaNode::run_mode](BMediaNode.md#Enums_run_mode "run_mode") mode = B_RECORDING);`

Sets the run mode for the given producer node to `mode`. Also sets the specified `delay` to be added to each buffer sent by the producer node. This function should only be called for `B_RECORDING` mode; it's provided to compensate when you connect a node that's in recording mode to a node that isn't.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_MEDIA_BAD_NODE`| The node is invalid.  
Port errors.| An error occurred communicating with the Media Server.  
  
####  SetRefFor(), GetRefFor()

`status_t SetRefFor(const media_node& fileInterface,  
entry_ref& file,  
bool createAndTruncate,  
bigtime_t* outDuration);``status_t GetRefFor(const media_node& fileInterface,  
entry_ref* outFile,  
[`BMimeType`](BMimeType.md "BMimeType")* outMimeType = NULL);`

`SetRefFor()` tells the [`BFileInterface`](BFileInterface.md "BFileInterface") `fileInterface` to work on the file whose entry_ref is specified by `file`. If `createAndTruncate` is `true`, any previous file with that reference is deleted and the file will be prepared for new output. If `createAndTruncate` is `false`, `outDuration` will, on return, contain the duration of the performance data found in the file.

`GetRefFor()` fills out the specified entry_ref, `outFile`, to reference the file with which the specified `fileInterface` node is working. If `outMimeType` isn't `NULL`, it'll contain a [`BMimeType`](BMimeType.md "BMimeType") object describing the file's type, unless an error occurs.

Note

![Note](./images/admonitions/Info_32.png)

These functions will deadlock if called from a node's control thread or while the control thread is blocked.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_MEDIA_BAD_NODE`| The specified node doesn't exist, or isn't a file interface.  
  
#### SetRunModeNode()

`status_t SetRunModeNode(const media_node& node,  
bigtime_t delay,  
[BMediaNode::run_mode](BMediaNode.md#Enums_run_mode "run_mode") newMode);`

Sends the specified `node` a request that it change its policy for handling situations where it falls behind during real-time processing.

The error returned by this function only indicates whether or not the request was sent successfully.

Return Code| Description  
---|---  
`B_OK`| No error sending the set mode request.  
`B_BAD_NODE`| The specified node is invalid.  
  
#### SetTimeSourceFor()

`status_t SetTimeSourceFor(media_node_id node,  
media_node_id timeSource);`

Tells the specified `node` to slave its timing to `timeSource`. Once this is done, the node will receive its notion of the passage of time from `timeSource`. As such, it will pause whenever timeSource is stopped, and so forth.

Note

![Note](./images/admonitions/Info_32.png)

By default, nodes are slaved to the system time source, so you only need to call this function if you need to slave a node to a different time source.

The node will take whatever precautions are necessary to remain faithful to the notion of time presented by `timeSource` without causing glitches in the presentation of its media. For example, if a sound card node has a DAC that drifts from `timeSource`, it might try to fix the problem by varying the sampling rate slightly, or by dropping or doubling buffers occasionally. This is why you should usually use the preferred time source—rather than the system time source—as your master time source. The preferred time source will usually be derived directly from the DAC being used to produce the media output.

Return Code| Description  
---|---  
`B_OK`| No error.  
[`BMessage`](BMessage.md "BMessage") errors.| Unable to set the time source.  
  
####  SniffRef(), SniffRefFor()

`status_t SniffRef(const entry_ref& node,  
uint64 requireNodeKinds,  
dormant_node_info* outNode,  
[`BMimeType`](BMimeType.md "BMimeType")* outMimeType = NULL);``status_t SniffRefFor(const media_node& fileInterface,  
const entry_ref& node,  
uint64 requireNodeKinds,  
dormant_node_info* outNode,  
[`BMimeType`](BMimeType.md "BMimeType")* outMimeType = NULL);`

`SniffRef()` asks all [`BMediaAddOn`](BMediaAddOn.md "BMediaAddOn") instances that satisfy the `requireNodeKinds` restraint to identify the file. The `requireNodeKinds` argument should contain flags composited from the [node_kind](TheMediaKit_Constants.md#Enums_node_kind "node_kind") constants.

The node that returns the greatest `outCapability` value will be chosen, and a reference to it put in `outNode`. The MIME type of the file will be put into `outMimeType`.

In simpler terms: `SniffRef()` returns the node that can best handle the media data in the specified file.

`SniffRefFor()`, on the other hand, asks the specified `fileInterface` node to examine the file. If the node recognizes the file, the MIME type of the file is stored in the buffer `outMimeType`, and the node's capability to handle the file is returned in `outCapability`.

If the node doesn't recognize the file, an error is returned. If the node recognizes the file format but finds no recognizable data within the file, `outCapability` is set to 0.0 and no error is returned.

In either case, the higher the `outCapability` value returned, the more appropriate the node is for handling the media data in the file.

Note

![Note](./images/admonitions/Info_32.png)

These functions will deadlock if called from a node's control thread or while the control thread is blocked.

Return Code| Description  
---|---  
`B_OK`| No errors.  
`B_MEDIA_BAD_NODE`| The specified node is invalid, or isn't a file interface.  
  
#### StartControlPanel()

`status_t StartControlPanel(const media_node node,  
[`Messenger`](BMessenger.md "BMessenger")* outMessenger = NULL);`

Tells the specified node to start its custom control panel, which is started outside your application. There's no way to tell when the user has closed the control panel, other than by indirectly detecting possible changes to the node, such as a renegotiation of the format of data being output by the node.

If a [`BMessenger`](BMessenger.md "BMessenger") is provided as input to `StartControlPanel()`, the function returns in `outMessenger` a [`Messenger`](BMessenger.md "BMessenger") that can be used to communicate with the control panel.

Note

![Note](./images/admonitions/Info_32.png)

These functions will deadlock if called from a node's control thread or while the control thread is blocked.

Return Code| Description  
---|---  
`B_OK`| No error starting the control panel.  
`B_MEDIA_BAD_NODE`| The specified node is invalid.  
  
####  StartNode(), StopNode()

`status_t StartNode(const media_node node,  
bigtime_t atPerformanceTime);``status_t StartNode(const media_node node,  
bigtime_t atPerformanceTime,  
bool immediate = false);`

`StartNode()` sends the specified node a request to start streaming data at the performance time specified by the `atPerformanceTime` argument, according to that node's time source.

By default, nodes are in a stopped state upon creation, so you have to call `StartNode()` once you have a reference to it before anything will happen. Starting a node that's already running has no effect.

`StopNode()` sends node a request to stop streaming data once the specified performance time `atPerformanceTime` is reached, according to that node's time source. Stopping a node that's already stopped has no effect. If `immediate` is `true`, the node is instructed to stop immediately and the `atPerformanceTime` argument is ignored; if `immediate` is `false`, the node is stopped at the specified performance time.

In either case, the requested change will occur at the time specified by `atPerformanceTime`.

Note

![Note](./images/admonitions/Info_32.png)

If the node is a time source, and you want to operate on the time source aspect of the node (to start or stop all slaved nodes), you should call [`SeekTimeSource()`](BMediaRoster.md#BMediaRoster_SeekTimeSource "SeekTimeSource\(\)") instead.

The error returned by these functions only indicates whether or not the request was sent successfully; the node may later run into problems trying to start or stop its media and you won't know it based on the result of these functions.

Note

![Note](./images/admonitions/Info_32.png)

`StopNode()` will deadlock if called from a node's control thread or while the control thread is blocked.

Return Code| Description  
---|---  
`B_OK`| No error sending the start or stop request.  
Other errors.| Are node specific.  
  
See also: [`SeekNode()`](BMediaRoster.md#BMediaRoster_SeekNode "SeekNode\(\)")

####  StartTimeSource(), StopTimeSource()

`status_t StartTimeSource(const media_node timeSource,  
bigtime_t atRealTime);``status_t StopTimeSource(const media_node timeSource,  
bigtime_t atRealTime,  
bool immediate = false);`

`StartTimeSource()` sends the specified `timeSource` a request to start running at the real time specified by the `atRealTime` argument.

`StopTimeSource()` sends node a request to stop the specified `timeSource` once the specified real time `atRealTime` is reached. Stopping a time source that's already stopped has no effect. If `immediate` is `true`, the time source is instructed to stop immediately and the `atRealTime` argument is ignored; if `immediate` is `false`, the time source is stopped at the specified real time.

In either case, the requested change will occur at the time specified by `atRealTime`.

The error returned by these functions only indicates whether or not the request was sent successfully; the node may later run into problems trying to start or stop and you won't know it based on the result of these functions.

Return Code| Description  
---|---  
`B_OK`| No error sending the start or stop request.  
Other errors.| Are node specific.  
  
See also: [`SeekTimeSource()`](BMediaRoster.md#BMediaRoster_SeekTimeSource "SeekTimeSource\(\)")

####  StartWatching(), StopWatching()

`status_t StartWatching(const [`BMessenger`](BMessenger.md "BMessenger")& notifyHandler);` `status_t StartWatching(const [`BMessenger`](BMessenger.md "BMessenger")& notifyHandler,  
int32 notificationType);` `status_t StartWatching(const [`BMessenger`](BMessenger.md "BMessenger")& notifyHandler,  
const media_node& node,  
int32 notificationType);`

`status_t StopWatching(const [`BMessenger`](BMessenger.md "BMessenger")& notifyHandler);` `status_t StopWatching(const [`BMessenger`](BMessenger.md "BMessenger")& notifyHandler,  
int32 notificationType);` `status_t StopWatching(const [`BMessenger`](BMessenger.md "BMessenger")& notifyHandler,  
const media_node& node,  
int32 notificationType);`

`StartWatching()` registers the specified [`BHandler`](BHandler.md "BHandler") or [`BLooper`](BLooper.md "BLooper") as a recipient of notification messages from the Media Server. `StopWatching()` cancels this registration so that no further notifications will be sent.

If you're only interested in a particular notification type, you can specify that code in the `notificationType` argument. If you don't specify a notification type, `B_MEDIA_WILDCARD` is assumed; this matches all notification types. You can also specify that you want to watch a specific node; if you don't specify a node, you'll receive notifications for all nodes.

Events are sent to registered [`BHandler`](BHandler.md "BHandler")s and [`BLooper`](BLooper.md "BLooper")s when certain events happen, such as nodes being created or deleted, or connections being made or broken.

See "[Notification Messages](TheMediaKit_MessageConstants.md "Message Constants")" for a list of the notifications you can receive, and the formats of the corresponding messages.

Although the Media Server will automatically cancel notifications to [`BHandler`](BHandler.md "BHandler")s and [`BLooper`](BLooper.md "BLooper")s that go away without explicitly calling `StopWatching()`, this detection is expensive and may briefly interrupt the media system, so you should always call `StopWatching()` before allowing a [`BHandler`](BHandler.md "BHandler") or [`BLooper`](BLooper.md "BLooper") to go away.

Return Code| Description  
---|---  
`B_OK`| No error cloning the node.  
`B_NAME_NOT_FOUND`| The requested node couldn't be cloned.  
  
#### SyncToNode()

`status_t SyncToNode(const media_node node,  
bigtime_t atPerformanceTime,  
bigtime_t timeout = B_INFINITE_TIMEOUT);`

If you want to detect the arrival of a specific performance time on a given node, you can do that by calling `SyncToNode()`. Specify the node you want to monitor in `node`, and the time you want to be notified of in `atPerformanceTime`. You can, optionally, specify a `timeout`; if the sync hasn't occurred in `timeout` microseconds, the request will time out.

This function doesn't return until either the specified performance time arrives, or the sync operation times out.

Return Code| Description  
---|---  
`B_OK`| All's well.  
`B_MEDIA_BAD_NODE`| The specified node is invalid.  
`B_TIMED_OUT`| The request timed out.  
Port errors.| An error occurred communicating with the Media Server.  
  
* * *

### Constants

#### connect_flags

Declared in: `media/MediaRoster.h`

Constant| Description  
---|---  
`B_CONNECT_MUTED`| The connection should be muted on creation.

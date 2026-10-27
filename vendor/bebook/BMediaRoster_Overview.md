<!-- System Overview - The Media Kit - BMediaRoster -->

## BMediaRoster  
  
The [`BMediaRoster`](BMediaRoster.md "BMediaRoster") class comprises the functionality that applications that use the Media Kit can access.

An application can only have a single instance of the [`BMediaRoster`](BMediaRoster.md "BMediaRoster") class, which is accessed by calling the static member function [`BMediaRoster::Roster()`](BMediaRoster.md#BMediaRoster_Roster "Roster\(\), CurrentRoster\(\)"), which creates the media roster, establishes the connection to the Media Server, then returns a pointer to the roster.

The creation of the roster object is thread protected, so you can safely call [`BMediaRoster::Roster()`](BMediaRoster.md#BMediaRoster_Roster "Roster\(\), CurrentRoster\(\)") from multiple threads without synchronization, and both threads will safely get the same instance. The cost of this synchronization is low enough that there's no need to cache the returned pointer, but it's perfectly safe to do so if you wish:
    
    
    BMediaRoster *gMediaRoster;
    
    int main(void) {
       status_t err;
    
       BApplication app("application/x-vnd.me-myself");
       gMediaRoster = BMediaRoster::Roster(&err);
    
       if (!gMediaRoster || (err != B_OK)) {
          /* the Media Server appears to be dead -- handle that here */
       }
    
       /* The Media Server connection is in place -- enjoy! */
    
       return 0;
    }

Because [`BMediaRoster`](BMediaRoster.md "BMediaRoster") is derived from [`BLooper`](BLooper.md "BLooper"), you should create your [`BApplication`](BApplication.md "BApplication") before calling [`BMediaRoster::Roster()`](BMediaRoster.md#BMediaRoster_Roster "Roster\(\), CurrentRoster\(\)"), although the [`BApplication`](BApplication.md "BApplication") doesn't have to be running yet.

You should never delete the [`BMediaRoster`](BMediaRoster.md "BMediaRoster") returned to you by [`BMediaRoster`](BMediaRoster.md "BMediaRoster"). Also, you can't derive a class from [`BMediaRoster`](BMediaRoster.md "BMediaRoster").

If you want to receive notifications from the Media Server when specific changes occur, such as nodes coming online or going offline, for example, you can register to receive such notifications by calling [`StartWatching()`](BMediaRoster.md#BMediaRoster_StartWatching "StartWatching\(\), StopWatching\(\)").

* * *

### Playing Media from Disk

To play a media file from disk, you would follow the following steps:

  * Create an [entry_ref](BEntry.md#entry_ref "entry_ref") that refers to the file to be played.

  * Call [`SniffRef()`](BMediaRoster.md#BMediaRoster_SniffRef "SniffRef\(\), SniffRefFor\(\)") to obtain a [dormant_node_info](BMediaAddOn.md#dormant_node_info "dormant_node_info") reference to the node that is best capable of playing back the media file.

  * Call [`InstantiateDormantNode()`](BMediaRoster.md#BMediaRoster_InstantiateDormantNode "InstantiateDormantNode\(\)") to instantiate the node; this returns a [media_node](TheMediaKit_DefinedTypes.md#media_node "media_node") that you can use for other [`BMediaRoster`](BMediaRoster.md "BMediaRoster") calls.

  * Use [`SetRefFor()`](BMediaRoster.md#BMediaRoster_SetRefFor "SetRefFor\(\), GetRefFor\(\)") to pass the [entry_ref](BEntry.md#entry_ref "entry_ref") of the file to be played to the node.

  * Call [`GetFreeOutputsFor()`](BMediaRoster.md#BMediaRoster_GetFreeOutputsFor) to obtain a [media_output](TheMediaKit_DefinedTypes.md#media_output "media_output") structure describing an available output on the producer node. This structure contains a [media_source](TheMediaKit_DefinedTypes.md#media_source "media_source") you can pass to [`Connect()`](BMediaRoster.md#BMediaRoster_Connect "Connect\(\), Disconnect\(\)") as the source of the media data.

  * The [media_output](TheMediaKit_DefinedTypes.md#media_output "media_output") structure returned by [`GetFreeOutputsFor()`](BMediaRoster.md#BMediaRoster_GetFreeOutputsFor) contains a [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") field that indicates the general type of media data contained by the media file; if you don't already know what type of data you're playing back (audio or video or whatever), this will let you determine what you'll be playing.

  * You can use this value to determine what type of destination is needed to output the data (for example, `B_MEDIA_RAW_AUDIO` would go to an audio output, and `B_MEDIA_RAW_VIDEO` would go to a video output).

  * Call [`GetAudioMixer()`](BMediaRoster.md#BMediaRoster_StartWatching "StartWatching\(\), StopWatching\(\)") to get a [media_node](TheMediaKit_DefinedTypes.md#media_node "media_node") for the default audio mixer (if the media data is audio), or [`GetVideoOutput()`](BMediaRoster.md#BMediaRoster_GetVideoOutput) to get a [media_node](TheMediaKit_DefinedTypes.md#media_node "media_node") for the default video output (if the media data is video).

  * Use [`GetFreeInputsFor()`](BMediaRoster.md#BMediaRoster_GetFreeInputsFor) to obtain a [media_input](TheMediaKit_DefinedTypes.md#media_input "media_input") structure describing an available input on the consumer node (the audio mixer or video output); this structure contains a [media_destination](TheMediaKit_DefinedTypes.md#media_destination "media_destination") you can pass to [`Connect()`](BMediaRoster.md#BMediaRoster_Connect "Connect\(\), Disconnect\(\)").

  * Call [`Connect()`](BMediaRoster.md#BMediaRoster_Connect "Connect\(\), Disconnect\(\)") to connect the source to the destination.

  * Next, you need to set the time source for the media file's node. Normally you'll set this to the preferred time source. The audio mixer or video output is already slaved to an appropriate time source.

  * Finally, you can use [`StartNode()`](BMediaRoster.md#BMediaRoster_StartNode "StartNode\(\), StopNode\(\)") to start the producer, consumer, and time source. The media file should begin playing back.

Let's look at actual sample code that does this. This example is particular to playing movie files; in particular, it assumes that the video is encoded (`B_MEDIA_ENCODED_VIDEO`) and that the audio is in a raw audio format (`B_MEDIA_RAW_AUDIO`). However, it demonstrates the principles of playing both encoded and raw media formats, and you can easily extrapolate from it any type of playback you need. First it's necessary to identify an appropriate node to handle the file, and to instantiate the node and configure it for the file we want to play:
    
    
    bigtime_t     duration;
    media_node      timeSourceNode;
    
    media_node      mediaFileNode;
    media_output    fileNodeOutput;
    int32           fileOutputCount;
    
    media_output    fileAudioOutput;
    int32           fileAudioCount;
    
    media_node      codecNode;
    media_output    codecOutput;
    media_input     codecInput;
    
    media_node      videoNode;
    media_input     videoInput;
    int32           videoInputCount;
    
    media_node      audioNode;
    media_input     audioInput;
    int32           audioInputCount;
    
    dormant_node_info nodeInfo;
    status_t err;
    
    playingFlag = false;
    roster = BMediaRoster::Roster();
    
    initStatus = roster->SniffRef(*ref, 0, &nodeInfo);
    if (initStatus) {
       return;
    }
    
    initStatus = roster->InstantiateDormantNode(nodeInfo, &mediaFileNode);
    if (initStatus) {
       return;
    }
    
    roster->SetRefFor(mediaFileNode, *ref, false, &duration);
    if ((err = Setup()) != B_OK) {
       printf("Error %08lX in Setup()\n", err);
    }
    else {
       Start();
    }

This code begins by obtaining a pointer to the media roster. It then calls [`SniffRef()`](BMediaRoster.md#BMediaRoster_SniffRef "SniffRef\(\), SniffRefFor\(\)") to get a [dormant_node_info](BMediaAddOn.md#dormant_node_info "dormant_node_info") structure describing the best-suited node for reading the media data from the file specified by `ref`.

Once a dormant_node_info structure has been filled out, the [`InstantiateDormantNode()`](BMediaRoster.md#BMediaRoster_InstantiateDormantNode "InstantiateDormantNode\(\)") function is called to instantiate a node to handle the file. A dormant node is a node whose code resides in an add-on, instead of within the application itself. The `nodeInfo` structure is passed into the function, and on return, the `mediaFileNode` has been set up for the appropriate node.

Since the [media_node](TheMediaKit_DefinedTypes.md#media_node "media_node") `mediaFileNode` is a file handling node, the [`SetRefFor()`](BMediaRoster.md#BMediaRoster_SetRefFor "SetRefFor\(\), GetRefFor\(\)") function is then called to tell the newly-instantiated file handler node what file it should handle. The inputs here are:

  * `mediaFileNode` is the node whose file reference is to be set.

  * `ref` is the file that the node should reference.

  * `false` indicates that the file must already exist. If this flag were `true`, and the file indicated by `ref` were nonexistent, the node would create a new file. Since we're playing a file, we don't want to do that.

  * `duration` is a bigtime_t variable that will receive the duration of the media file, in microseconds.

Once this has been accomplished, it's time to instantiate the other nodes needed to perform the media playback. Note that your code should check the error results from each of these calls and only proceed if `B_OK` is returned.

The `Setup()` and `Start()` functions used in the example above are given below. `Setup()` actually sets up the connections and instantiates the various other nodes (such as codecs and output nodes) required to play back the media data. Let's take a look at `Setup()` next:
    
    
    status_t MediaPlayer::Setup(void) {
       status_t err;
       media_format tryFormat;
       dormant_node_info nodeInfo;
       int32 nodeCount;
    
       err = roster->GetAudioMixer(&audioNode);
       err = roster->GetVideoOutput(&videoNode);

First, [`GetAudioMixer()`](BMediaRoster.md#BMediaRoster_GetAudioMixer) and [`GetVideoOutput()`](BMediaRoster.md#BMediaRoster_GetVideoOutput) are called to obtain an audio mixer node and a video output node. The nodes returned by this function are based on the user's preferences in the Audio and Video preference applications. By default, video is output to a simple video output consumer that creates a window to contain the video display.

Note

![Note](./images/admonitions/Info_32.png)

The VideoConsumer node will be available with R4.5; it's not provided in R4. In addition, there are no video producer nodes in R4; media add-ons for a variety of movie file formats will also be available beginning with R4.5.
    
    
       err = roster->GetTimeSource(&timeSourceNode);
       b_timesource = roster->MakeTimeSourceFor(timeSourceNode);

This code obtains a [media_node](TheMediaKit_DefinedTypes.md#media_node "media_node") for the preferred time source, and then creates a [`BTimeSource`](BTimeSource.md "BTimeSource") object that refers to the same node; we'll need to be able to make some [`BTimeSource`](BTimeSource.md "BTimeSource") calls to obtain some specific timing information later.

A time source is a node that can be used to synchronize other nodes. By default, nodes are slaved to the system time source, which is the computer's internal clock. However, this time source, while very precise, isn't good for synchronizing media data, since its concept of time has nothing to do with actual media being performed. For this reason, you typically will want to change nodes' time sources to the preferred time source.

You can think of a media node (represented by the [media_node](TheMediaKit_DefinedTypes.md#media_node "media_node") structure) as a component in a home theater system you might have at home. It has inputs for audio and video (possibly multiple inputs for each), and outputs to pass that audio and video along to other components in the system. To use the component, you have to connect wires from the outputs of some other components into the component's inputs, and the outputs into the inputs of other components.

The Media Kit works the same way. We need to locate audio outputs from the `mediaFileNode` and find corresponding audio inputs on the `audioNode`. This is analogous to choosing an audio output from your new DVD player and matching it to an audio input jack on your stereo receiver. Since you can't use ports that are already in use, we call [`GetFreeOutputsFor()`](BMediaRoster.md#BMediaRoster_GetFreeOutputsFor) to find free output ports on the `mediaFileNode`, and [`GetFreeInputsFor()`](BMediaRoster.md#BMediaRoster_GetFreeInputsFor) to locate free input ports on the `audioNode`.
    
    
       err = roster->GetFreeOutputsFor(mediaFileNode, &fileAudioOutput, 1,
                   &fileAudioCount, B_MEDIA_RAW_AUDIO);
       err = roster->GetFreeInputsFor(audioNode, &audioInput, fileAudioCount,
                   &audioInputCount, B_MEDIA_RAW_AUDIO);

We only want a single audio connection between the two nodes (a single connection can carry stereo sound), and the connection is of type `B_MEDIA_RAW_AUDIO`. On return, `fileAudioOutput` and `audioInput` describe the output from the `mediaFlieNode` and the input into the `audioNode` that will eventually be connected to play the movie's sound.

We likewise have to find a video output from the `mediaFileNode` and an input into the `videoNode`. In this case, though, we expect the video output from the `mediaFileNode` to be encoded, and the `videoNode` will want to receive raw, uncompressed video. We'll work that out in a minute; for now, let's just find the two ports:
    
    
       err = roster->GetFreeOutputsFor(mediaFileNode, &fileNodeOutput, 1,
                   &fileOutputCount, B_MEDIA_ENCODED_VIDEO)
       err = roster->GetFreeInputsFor(videoNode, &videoInput, fileOutputCount,
                   &videoInputCount, B_MEDIA_RAW_VIDEO);

The problem we have now is that the `mediaFileNode` is outputting video that's encoded somehow (like in Cinepak format, for instance). The `videoNode`, on the other hand, wants to display raw video. Another node must be placed between these to decode the video (much like having an adapter to convert PAL video into NTSC, for example). This node will be the codec that handles decompressing the video into raw form.

We need to locate a codec node that can handle the video format being output by the `mediaFileNode`. This is accomplished like this:
    
    
       nodeCount = 1;
       err = roster->GetDormantNodes(&nodeInfo, &nodeCount,
                   &fileNodeOutput.format);
       if (!nodeCount) {
          return -1;
       }

This call to [`GetDormantNodes()`](BMediaRoster.md#BMediaRoster_GetDormantNodes "GetDormantNodes\(\)") looks for a dormant node that can handle the media format specified by the `mediaFileNode`'s output [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") structure. Information about the node is returned in `nodeInfo`. `nodeCount` indicates the number of matching nodes that were found. If it's zero, an error is returned.

Note that in real life you should ask for several nodes, and search through them, looking at the formats until you find one that best meets your needs.

Then we use [`InstantiateDormantNode()`](BMediaRoster.md#BMediaRoster_InstantiateDormantNode "InstantiateDormantNode\(\)") to instantiate the codec node, and locate inputs into the node (that accept encoded video) and outputs from the node (that output raw video):
    
    
       err = roster->InstantiateDormantNode(nodeInfo, &codecNode);
       err = roster->GetFreeInputsFor(codecNode, &codecInput, 1, &nodeCount,
                   B_MEDIA_ENCODED_VIDEO);
       err = roster->GetFreeOutputsFor(codecNode, &codecOutput, 1,
                   &nodeCount, B_MEDIA_RAW_VIDEO);

Now we're ready to start connecting these nodes together. If we were setting up a home theater system, right about now we'd be getting rug burns on our knees and skinned knuckles on our hands, trying to reach behind the entertainment center to run wires. The Media Kit is way easier than that, and doesn't involve salespeople telling you to get expensive gold-plated cables.

We begin by connecting the file node's video output to the codec's input:
    
    
       tryFormat = fileNodeOutput.format;
       err = roster->Connect(fileNodeOutput.source, codecInput.destination,
                   &tryFormat, &fileNodeOutput, &codecInput);

`tryFormat` indicates the format of the encoded video that will be output by the `mediaFileNode`. [`Connect()`](BMediaRoster.md#BMediaRoster_Connect "Connect\(\), Disconnect\(\)"), in essense, runs a wire between the output from the media node's video output (`fileNodeOutput`) to the codec node's input.

You may wonder what's up with the `fileNodeOutput`.`source` and `codecInput`.`destination` structures. These [media_source](TheMediaKit_DefinedTypes.md#media_source "media_source") and [media_destination](TheMediaKit_DefinedTypes.md#media_destination "media_destination") structures are simplified descriptors of the two ends of the connection. They contain only the data absolutely needed for the Media Kit to establish the connection. This saves some time when issuing the [`Connect()`](BMediaRoster.md#BMediaRoster_Connect "Connect\(\), Disconnect\(\)") call (and time is money, especially in the media business).

Next it's necessary to connect the codec to the video output node. This begins by setting up `tryFormat` to describe raw video of the same width and height as the encoded video being fed into the codec, then calling [`Connect()`](BMediaRoster.md#BMediaRoster_Connect "Connect\(\), Disconnect\(\)") to establish the connection:
    
    
       tryFormat.type = B_MEDIA_RAW_VIDEO;
       tryFormat.u.raw_video = media_raw_video_format::wildcard;
       tryFormat.u.raw_video.display.line_width =
                codecInput.format.u.encoded_video.output.display.line_width;
       tryFormat.u.raw_video.display.line_count =
                codecInput.format.u.encoded_video.output.display.line_count;
       err = roster->Connect(codecOutput.source, videoInput.destination,
                   &tryFormat, &codecOutput, &videoInput);

Now we connect the audio from the media file to the audio mixer node. We just copy the [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") from the file's audio output, since both ends of the connection should exactly match.
    
    
       tryFormat = fileAudioOutput.format;
       err = roster->Connect(fileAudioOutput.source, audioInput.destination,
                   &tryFormat, &fileAudioOutput, &audioInput);

The last step of configuring the connections is to ensure that all the nodes are slaved to the preferred time source. This will keep them synchronized with the preferred time source (and by association, with each other):
    
    
       err = roster->SetTimeSourceFor(mediaFileNode.node,
    timeSourceNode.node);
       err = roster->SetTimeSourceFor(videoNode.node, timeSourceNode.node);
       err = roster->SetTimeSourceFor(codecOutput.node.node,
                timeSourceNode.node);
       err = roster->SetTimeSourceFor(audioNode.node, timeSourceNode.node);
       return B_OK;
    }

Finally, we return `B_OK` to the caller. Note that this code should be enhanced to check the results of each [`BMediaRoster`](BMediaRoster.md "BMediaRoster") call, and to return the result code if it's not `B_OK`. This has been left out of this example for brevity.

The `Start()` function actually starts the movie playback. Starting playback involves starting, one at a time, all the nodes involved in playing back the audio. This includes the audio mixer (`audioNode`), the media file's node (`mediaFileNode`), the codec, and the video node.
    
    
    status_t MediaPlayer::Start(void) {
       status_t err;
       err = roster->GetStartLatencyFor(timeSourceNode, &startTime);
       startTime += b_timesource->PerformanceTimeFor(BTimeSource::RealTime()
                   + 1000000 / 50);
    
       err = roster->StartNode(mediaFileNode, startTime);
       err = roster->StartNode(codecNode, startTime);
       err = roster->StartNode(videoNode, startTime);
    
       return B_OK;
    }

Because there's lag time between starting each of these nodes, we pick a time a few moments in the future for playback to begin, and schedule each node to start playing at that time. So we begin by computing that time in the future.

The [`BTimeSource::RealTime()`](BTimeSource.md#BTimeSource_RealTime "RealTime\(\)") static member function is called to obtain the current real system time. We add a fiftieth of a second to that time, and convert it into performance time units. This is the time at which the performance of the movie will begin (basically a fiftieth of a second from "now"). This value is saved in `startTime`. These are added to the value returned by [`GetStartLatencyFor()`](BMediaRoster.md#BMediaRoster_GetStartLatencyFor "GetStartLatencyFor\(\)"), which returns the time required to actually start the time source and all the nodes slaved to it.

Then we simply call [`BMediaRoster::StartNode()`](BMediaRoster.md#BMediaRoster_StartNode "StartNode\(\), StopNode\(\)") for each node, specifying `startTime` as the performance time at which playback should begin.

Again, error handling should be added to actually return the error code from these functions.

Stopping playback of the movie is even simpler:
    
    
       err = roster->StopNode(mediaFileNode, 0, true);
       err = roster->StopNode(codecNode, 0, true);
       err = roster->StopNode(videoNode, 0, true);

This tells the media file, video codec, and video output nodes to stop immediately. If we wanted them to stop together at some time in the future, we could compute an appropriate performance time and pass that instead of 0. In this case, we would need to specify `false` for the last argument; when this value is `true`, [`StopNode()`](BMediaRoster.md#BMediaRoster_StopNode) stops the node immediately. We could use this ability to schedule all three nodes to stop at the same time, so that video and audio playback would halt simultaneously.

Note that we don't stop the audio mixer node. You should never stop the mixer node, because other applications are probably using it.

Once you're done playing the movie, and have stopped playback, you should disconnect the nodes from each other:
    
    
       err = roster->Disconnect(mediaFileNode.node, fileNodeOutput.source,
                   codecNode.node, codecInput.destination);
       err = roster->Disconnect(codecNode.node, codecOutput.source,
                   videoNode.node, videoInput.destination);
       err = roster->Disconnect(mediaFileNode.node, fileAudioOutput.source,
                   audioNode.node, audioInput.destination);

This will close out the connections between the media file node and the video codec, the codec and the video output, and between the file node and the audio mixer. You should always stop playback before disconnecting; although nodes aren't allowed to crash if you disconnect them while running, their behavior isn't specified, and may not be what you expect.

Once the connections are severed, you should release any dormant nodes you instantiated. This includes not only nodes instantiated using [`InstantiateDormantNode()`](BMediaRoster.md#BMediaRoster_InstantiateDormantNode "InstantiateDormantNode\(\)"), but also default nodes (those obtained using functions like [`GetAudioInput()`](BMediaRoster.md#BMediaRoster_GetAudioInput "GetAudioInput\(\), GetVideoInput\(\)") and [`GetVideoOutput)`](BMediaRoster.md#BMediaRoster_GetVideoOutput), for example):
    
    
       roster->ReleaseNode(codecNode);
       roster->ReleaseNode(mediaFileNode);
       roster->ReleaseNode(videoNode);
       roster->ReleaseNode(audioNode);

If you want to play audio, you may find it much easier to use the `BSound` and [`BSoundPlayer`](BSoundPlayer.md "BSoundPlayer") classes to do so. As of R4.5, there are no Be-provided nodes for producing audio from a disk file.

#### Detecting When Playback Is Complete

There isn't a Media Kit function that can directly tell you whether or not the media has reached the end of the data during playback. However, the following easy-to-implement code can do the job for you:
    
    
    bigtime_t currentTime;
    bool isPlaying = true;
    
    currentTime = b_timesource->PerformanceTimeFor(BTimeSource::RealTime());
    if (currentTime >= startTime+duration) {
       isPlaying = false;
    }

This works by obtaining the time source's performance time and comparing it to the time at which playback of the movie was begun plus the movie's duration (both of which were saved when we initially set up and began playback of the movie, as seen in the code in the previous section above).

If the current performance time is equal to or greater than the sum of the starting time and the movie's duration, then playback is finished, and we set `isPlaying` to `false`; otherwise, this value remains `true`.

#### Using BMediaRoster Functions from Nodes

You can issue [`BMediaRoster`](BMediaRoster.md "BMediaRoster") function calls from within your own node, however, as a general rule, you shouldn't call [`BMediaRoster`](BMediaRoster.md "BMediaRoster") functions from within your control thread, or while the control thread is blocked. Many [`BMediaRoster`](BMediaRoster.md "BMediaRoster") functions use synchronous turnarounds, and will deadlock in this situation. You should assume, for safety's sake, that all [`BMediaRoster`](BMediaRoster.md "BMediaRoster") functions will deadlock if used in these cases.

For example, if you have an application that's playing video into a window, and you call [`StopNode()`](BMediaRoster.md#BMediaRoster_StopNode) from the window's [`MessageReceived()`](BWindow.md#BWindow_MessageReceived "MessageReceived\(\)") function, a deadlock would occur if the video player node blocks waiting on the window to be unlocked, and the [`StopNode()`](BMediaRoster.md#BMediaRoster_StopNode) function is keeping the window locked while it waits for the video producer node, which is blocked waiting on the consumer node, and so forth. Deadlock results, and that's a bad thing.

Instead, you should consider creating a seperate [`BLooper`](BLooper.md "BLooper") that manages your nodes. Future versions of the Media Kit will provide convenience classes to do some of this for you.

<!-- Classes And Methods - The Media Kit - Constants -->

## Constants

This section describes the constants that make the Media Kit tick.

* * *

### Codec Type Info

Declared in: `media/MediaDefs.h`

Constant| Description  
---|---  
`B_CODEC_TYPE_INFO`| Data is in codec type info format.  
  
The `B_CODEC_TYPE_INFO` constant is used to indicate that data is a codec info structure; it's used in the `user_data_type` field in the [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") structure, for example.

* * *

###  Data Endianness

Declared in: `media/MediaDefs.h`

Constant| Description  
---|---  
`B_MEDIA_BIG_ENDIAN`| The media data is big-endian  
`B_MEDIA_LITTLE_ENDIAN`| The media data is little-endian.  
  
These values indicate whether media data is little-endian or big-endian.

In addition, `B_MEDIA_HOST_ENDIAN` is defined to be one of these values, indicating the endianness of the system on which your application is running.

* * *

###  Media Message Size

Declared in: `media/MediaDefs.h`
    
    
    const size_t B_MEDIA_MESSAGE_SIZE = 16384;

The `B_MEDIA_MESSAGE_SIZE` constant specifies the maximum possible size a message a node's service thread can receive. 16k is an upper limit and won't be exceeded. Your thread can simply create a buffer this large, [`read_port()`](TheKernelKit_Ports.md#read_port "read_port\(\), read_port_etc\(\)") into it, and call [`HandleMessage()`](BMediaNode.md#BMediaNode_HandleMessage "HandleMessage\(\)") to process it.

* * *

###  media_display_flags

Declared in: `media/MediaDefs.h`

Constant| Description  
---|---  
`B_F1_DOMINANT`| The first buffer sent (temporally) will be an F1 field.  
`B_F2_DOMINANT`| The first buffer sent (temporally) will be an F2 field.  
`B_TOP_SCANLINE_F1`| The topmost scanline of the output buffer belongs to F1.  
`B_TOP_SCANLINE_F2`| The topmost scanline of the output buffer belongs to F2.  
  
These values are used to define the order in which the F1 and F2 fields of video data are used.

* * *

###  media_flags

Declared in: `media/MediaDefs.h`

Constant| Description  
---|---  
`B_MEDIA_FLAGS_VERSION`| Returns the Media Kit version as an int32 value.  
  
The media flags indicate values you can pass to [`BMediaRoster::MediaFlags()`](BMediaRoster.md#BMediaRoster_MediaFlags "MediaFlags\(\)") to get information about the version of the Media Kit that your application is running under.

* * *

###  media_format_family

Declared in: `media/MediaDefs.h`

Constant| Description  
---|---  
`B_ANY_FORMAT_FAMILY`| Any family.  
`B_BEOS_FORMAT_FAMILY`| BeOS format.  
`B_QUICKTIME_FORMAT_FAMILY`| QuickTime format.  
`B_AVI_FORMAT_FAMILY`| AVI format.  
`B_ASF_FORMAT_FAMILY`| ASF format.  
`B_MPEG_FORMAT_FAMILY`| MPEG format.  
`B_WAV_FORMAT_FAMILY`| WAVE format audio.  
`B_AIFF_FORMAT_FAMILY`| AIFF format audio.  
`B_AVR_FORMAT_FAMILY`| AVR format audio.  
`B_MISC_FORMAT_FAMILY`| Miscellaneous.  
  
These constants indicate the media format families the [`BMediaFormats`](BMediaFormats.md "BMediaFormats") class can map among.

* * *

###  media_format_flags

Declared in: `media/MediaDefs.h`

Constant| Description  
---|---  
`B_MEDIA_RETAINED_DATA`| Data in the buffer should be retained between calls to the decoder. A decoder that requires this behavior will set this bit in `require_flags`, and an application that doesn't support this behavior will set this bit in `deny_flags`.  
`B_MEDIA_MULTIPLE_BUFFERS`| An application will set this flag in `require_flags` if it passes multiple buffers to the decoder; a decoder that doesn't support this will set this flag in `deny_flags`.  
`B_MEDIA_CONTIGUOUS_BUFFER`| A decoder will set this flag in `require_flags` if it requires that buffers be in contiguous physical memory (for example, if it uses a hardware decoder). An application at doesn't support this should set this bit in `deny_flags`.  
`B_MEDIA_LINEAR_UPDATES`| An application sets this flag in `require_flags` if it requires the decoder to write linearly (instead of randomly) into the buffer (for example, if using overlays). A decoder that really wants to write randomly should set this flag in `deny_flags`.  
`B_MEDIA_MAUI_UNDEFINED_FLAGS`| Both applications and decoders should set this in both `require_flags` and `deny_flags` to avoid allowing or denying support for features they know nothing about.  
  
These flags are used when specifying [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format")'s `allow_flags` or `deny_flags` fields, to indicate how the decoder should behave.

* * *

###  media_multi_matrix

Declared in: `media/MediaDefs.h`

Constant| Description  
---|---  
`B_MATRIX_PROLOGIC_LR`| Dolby ProLogic matrix.  
`B_MATRIX_AMBISONIC_WXYZ`| Ambisonic WXYZ matrix.  
  
These values identify a particular multi-channel audio matrix.

* * *

###  media_producer_status

Declared in: `media/MediaDefs.h`

Constant| Description  
---|---  
`B_DATA_NOT_AVAILABLE`| There aren't any buffers ready for the destination.  
`B_DATA_AVAILABLE`| There are buffers ready for the destination.  
`B_PRODUCER_STOPPED`| The producer isn't running.  
  
These values can be specified when your producer node calls [`BBufferProducer::SendDataStatus()`](BBufferProducer.md#BBufferProducer_SendDataStatus "SendDataStatus\(\)") to tell the consumers it's feeding whether or not data is available for it to process. The `B_PRODUCER_STOPPED` code is sent to indicate to downstream consumers that it's no longer running.

Consumers receive these values in the [`BBufferConsumer::ProducerDataStatus()`](BBufferConsumer.md#BBufferConsumer_ProducerDataStatus "ProducerDataStatus\(\)") hook function.

* * *

###  media_realtime_flags

Declared in: `media/MediaDefs.h`

Constant| Description  
---|---  
`B_MEDIA_REALTIME_ALLOCATOR`| When set, [`rtm_alloc()`](TheMediaKit_RealTimeFunctions.md#rtm_alloc "rtm_alloc\(\), rtm_realloc\(\)") will return locked memory.  
`B_MEDIA_REALTIME_AUDIO`| Audio add-ons in the Media Server are locked in memory, and should lock their thread stacks using [`media_realtime_init_thread()`](TheMediaKit_Functions.md#media_realtime_init_thread "media_realtime_init_thread\(\)").  
`B_MEDIA_REALTIME_VIDEO`| Video add-ons are locked in memory, and should lock their thread stacks using [`media_realtime_init_thread()`](TheMediaKit_Functions.md#media_realtime_init_thread "media_realtime_init_thread\(\)").  
`B_MEDIA_REALTIME_ANYKIND`| All Media add-ons are locked in memory, and should lock their thread stacks using [`media_realtime_init_thread()`](TheMediaKit_Functions.md#media_realtime_init_thread "media_realtime_init_thread\(\)").  
  
These values are passed to realtime allocator functions to indicate what types of nodes should use locked memory.

* * *

###  media_type

Declared in: `media/MediaNode.h`

Constant| Description  
---|---  
`B_MEDIA_NO_TYPE`| Typeless media data.  
`B_MEDIA_UNKNOWN_TYPE`| Unknown media type.  
`B_MEDIA_RAW_AUDIO`| Uncompressed, linear audio.  
`B_MEDIA_RAW_VIDEO`| Uncompressed, linear video.  
`B_MEDIA_VBL`| Raw data from VBL area.  
`B_MEDIA_TIMECODE`| Timecode data. Undetermined format.  
`B_MEDIA_MIDI`| MIDI  
`B_MEDIA_TEXT`| Text data; typically closed-captioning information.  
`B_MEDIA_HTML`| HTML data.  
`B_MEDIA_MULTISTREAM`| Multi-stream media data, such as AVI or QuickTime.  
`B_MEDIA_PARAMETERS`| [`BControllable`](BControllable.md "BControllable") change data.  
`B_MEDIA_ENCODED_AUDIO`| Encoded audio (such as AC3 or DTS).  
`B_MEDIA_ENCODED_VIDEO`| Encoded video (such as Indeo or MPEG).  
`B_MEDIA_PRIVATE`| Base value for private Be media types.  
`B_MEDIA_FIRST_USER_TYPE`| Base value for user-defined media types.  
  
Types of media data.

* * *

###  multi_media_channels

Declared in: `media/MediaDefs.h`

Constant| Description  
---|---  
`B_CHANNEL_LEFT`| The left channel.  
`B_CHANNEL_RIGHT`| The right channel.  
`B_CHANNEL_CENTER`| The center channel (5.1+ or fake surround)  
`B_CHANNEL_SUB`| Subwoofer (5.1+).  
`B_CHANNEL_REARLEFT`| Quad surround or 5.1+  
`B_CHANNEL_REARRIGHT`| Quad surround or 5.1+.  
`B_CHANNEL_FRONT_LEFT_CENTER`| Front-left center channel.  
`B_CHANNEL_FRONT_RIGHT_CENTER`| Front-right center channel.  
`B_CHANNEL_BACK_CENTER`| The rear center channel (6.1 or fake surround).  
`B_CHANNEL_SIDE_LEFT`| The left side channel.  
`B_CHANNEL_SIDE_RIGHT`| The right side channel.  
`B_CHANNEL_TOP_CENTER`| The top center channel.  
`B_CHANNEL_TOP_FRONT_LEFT`| The top front-left channel.  
`B_CHANNEL_TOP_FRONT_CENTER`| The top front-center channel.  
`B_CHANNEL_TOP_FRONT_RIGHT`| The top front-right channel.  
`B_CHANNEL_TOP_BACK_LEFT`| The top back-left channel.  
`B_CHANNEL_TOP_BACK_CENTER`| The top back-center channel.  
`B_CHANNEL_TOP_BACK_RIGHT`| The top back-right channel.  
  
Speaker positions for multi-channel audio.

* * *

###  node_kind

Declared in: `media/MediaDefs.h`

Constant| Description  
---|---  
`B_BUFFER_PRODUCER`| The flavor implements [`BBufferProducer`](BBufferProducer.md "BBufferProducer").  
`B_BUFFER_CONSUMER`| The flavor implements [`BBufferConsumer`](BBufferConsumer.md "BBufferConsumer").  
`B_TIME_SOURCE`| The flavor implements [`BTimeSource`](BTimeSource.md "BTimeSource").  
`B_CONTROLLABLE`| The flavor implements [`BControllable`](BControllable.md "BControllable").  
`B_FILE_INTERFACE`| The flavor implements [`BFileInterface`](BFileInterface.md "BFileInterface").  
`B_ENTITY_INTERFACE`| The flavor implements `BEntityInterface` (not available for R4).  
`B_PHYSICAL_INPUT`| The flavor represents a physical input point (such as an input jack).  
`B_PHYSICAL_OUTPUT`| The flavor represents a physical output point (such as line output).  
`B_SYSTEM_MIXER`| The flavor represents the system mixer.  
  
Defines the type or types of node that a particular flavor supports. Note that a flavor can implement more than one type of node, so you can combine these values using a bitwise or.

* * *

###  Quality

Declared in: `media/MediaDefs.h`

Constant| Description  
---|---  
`B_MEDIA_ANY_QUALITY`| Any quality level (0.0).  
`B_MEDIA_LOW_QUALITY`| Low quality (0.1).  
`B_MEDIA_MEDIUM_QUALITY`| Medium quality (0.5).  
`B_MEDIA_HIGH_QUALITY`| High quality (1.0).  
  
These are default values you can use for various quality levels.

* * *

###  video_orientation

Declared in: `media/MediaNode.h`

Constant| Description  
---|---  
`B_VIDEO_TOP_LEFT_RIGHT`| Typical progressive scan: top to bottom, left to right.  
`B_VIDEO_BOTTOM_LEFT_RIGHT`| Bottom to top, left to right (BMP and TGA graphics are oriented this way).  
  
Indicates the orientation of video data (whether the data is oriented top to bottom or bottom to top).

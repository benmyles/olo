<!-- Classes And Methods - The Media Kit - Defined Types -->

## Defined Types

This section describes the defined types that make the Media Kit tick.

* * *

### encode_parameters

Declared in: `media/MediaDefs.h`
    
    
    struct encode_parameters {
        float  quality;
        int32  avg_field_size;
        int32  max_field_size;
        int32  _pad[27];
        void*  user_data;
        size_t user_data_size;
    };

Parameters for encoding a buffer.

Field| Description  
---|---  
`quality`| Indicates the compression quality to use. 0.0 is lowest quality, 1.0 is high quality. You can use the [`B_MEDIA_ANY_QUALITY`](TheMediaKit_Constants.md#B_MEDIA_ANY_QUALITY), [`B_MEDIA_LOW_QUALITY`](TheMediaKit_Constants.md#B_MEDIA_LOW_QUALITY), and so forth constants if you wish.  
`avg_field_size`| Indicates the average size of a field that should be allowed, in bytes. The encoder will try to maintain this average as it compresses.  
`max_field_size`| Indicates the maximum field size that should be allowed, in bytes. The encoder will try to encode each field to be smaller than this size.  
`user_data` and `user_data_size`| Indicate user-defined data.  
  
* * *

### live_node_info

Declared in: `media/MediaDefs.h`
    
    
    struct live_node_info {
        live_node_info();
        ~live_node_info();
        media_node node;
        BPoint hint_point;
        char name[B_MEDIA_NAME_LENGTH];
      private:
        reserved[160];
    };

The live_node_info structure describes an active node, and is returned by [`BMediaRoster::GetLiveNodes()`](BMediaRoster.md#BMediaRoster_GetLiveNodes "GetLiveNodes\(\)").

Field| Description  
---|---  
`node`| Specifies which node the live_node_info structure describes.  
`hint_point`| Is reserved for future use. Pass a [`BPoint`](BPoint.md "BPoint") with x and y both set to 0.  
  
* * *

### media_addon_id

Declared in: `media/MediaDefs.h`
    
    
    typedef int32 media_addon_id;

A media add-on ID number.

* * *

### media_audio_header

Declared in: `media/MediaDefs.h`
    
    
    struct media_audio_header {
        int32 _reserved_[16];
    };

Defines the header of an audio buffer. There are no Be-defined fields in this structure as of the release of BeOS R4; however, you may append fields to the end of this structure if you wish.

* * *

### media_buffer_id

Declared in: `media/MediaDefs.h`
    
    
    typedef int32 media_buffer_id;

A media buffer ID number.

* * *

### media_decode_info

Declared in: `media/MediaDefs.h`
    
    
    struct media_decode_info {
        bigtime_t time_to_decode;
        int32  _pad[26];
        void*  file_format_data;
        size_t file_format_data_size;
        void*  codec_data;
        size_t codec_data_size;
    
        media_decode_info();
    };

Provides options to decoders.

Field| Description  
---|---  
`time_to_decode`| Indicates how long, in microseconds, the decoder is allowed to take to decode the buffer. Specify 0 to do it as fast as possible, or `B_INFINITE_TIMEOUT` if you don't care.  
`file_format_data` and `file_format_data_size`| These are a pointer to file-format specific data, and the size of the buffer respectively.  
`codec_data` and `codec_data_size`| These are a pointer to a codec-specific data buffer, and the size of the buffer respectively.  
  
* * *

### media_destination

Declared in: `media/MediaDefs.h`
    
    
    struct media_destination {
        media_destination(port_id, int32);
        media_destination(const media_destination& clone);
        media_destination& operator= (const media_destination& clone);
        media_destination();
        ~media_destination();
        port_id port;
        int32 id;
        static media_destination null;
    private:
        uint32 _reserved_media_destination_[2];
    };

Describes the destination end of a media connection between two nodes. The `port` indicates the port ID on which the destination communicates. This can be different from the `media_node`.`port` value.

* * *

### media_encode_info

Declared in: `media/MediaDefs.h`
    
    
    struct media_encode_info {
        uint32 flags;
        int32 used_data_size;
        bigtime_t start_time;
        bigtime_t time_to_encode;
        int32 _pad[22];
        void* file_format_data;
        size_t file_format_data_size;
        void* codec_data;
        size_t codec_data_size;
    
        media_encode_info();
    };

Provides options to encoders and file writers about.

Field| Description  
---|---  
`flags`| Contains flags that alter the behavior of the encoder or writer. The only defined value right now is `B_MEDIA_KEY_FRAME`.  
`used_data_size`| Is currently reserved for use by Be.  
`start_time`| Indicates the time at which the buffer being written is located, in microseconds, from the start of the file.  
`time_to_decode`| Indicates how long, in microseconds, the encoder is allowed to take to encode the buffer. Specify 0 to do it as fast as possible, or `B_INFINITE_TIMEOUT` if you don't care.  
`file_format_data` and `file_format_data_size`| These are a pointer to file-format specific data, and the size of the buffer respectively.  
`codec_data` and `codec_data_size`| These are a pointer to a codec-specific data buffer, and the size of the buffer respectively.  
  
* * *

### media_encoded_audio_format

Declared in: `media/MediaDefs.h`
    
    
    struct media_encoded_audio_format {
        enum audio_encoding {
           B_ANY
        };
        media_raw_audio_format output;
        audio_encoding encoding;
        float bit_rate;
        size_t frame_size;
        media_multi_audio_info multi_info;
        uint32 _reserved_[3];
        static media_encoded_audio_format wildcard;
    };

Describes the format of encoded audio data.

Field| Description  
---|---  
`output`| Indicates the raw audio format that results from decoding the encoded audio.  
`encoding`| Specifies the encoding format. This should just be `B_ANY`; this field, and the audio_encoding enum, are depreciated. Instead, the [`BMediaFormats`](BMediaFormats.md "BMediaFormats") database should be used to identify the encoding format.  
`bit_rate`| Specifies the bit rate (not byte rate) of the data stream.  
`frame_size`| Indicates the size of each frame of audio, in bytes.  
`multi_info`| Provides multi-channel information.  
  
* * *

### media_encoded_audio_header

Declared in: `media/MediaDefs.h`
    
    
    struct media_encoded_audio_header {
        int32 _reserved_0[14];
        uint32 buffer_flags;
        uchar unused_mask;
        uchar _reserved_2[3];
    };

Defines the header of a an encoded audio buffer.

Field| Description  
---|---  
`buffer_flags`| Contains flags that provide additional information about the buffer. The only value currently defined (other than 0) is `B_MEDIA_KEY_FRAME`, which indicates that the buffer is a key frame.  
`unused_mask`| Indicates which bits of the last byte in the buffer are unused (since encoded audio may result in a number of bits that doesn't fill up the last byte in the buffer).  
  
If you wish, you can add more fields to the end of the header.

* * *

### media_encoded_video_format

Declared in: `media/MediaDefs.h`
    
    
    struct media_encoded_video_format {
        enum video_encoding {
           B_ANY
        };
        media_raw_video_format output;
        float avg_bit_rate;
        float max_bit_rate;
        video_encoding encoding;
        size_t frame_size;
        int16 forward_history;
        int16 backward_history;
        uint32 _reserved_[3];
        static media_encoded_video_format wildcard;
    };

Describes the format of encoded video data.

Field| Description  
---|---  
`output`| Specifies the raw video format that results from decoding the video. Set this to media_raw_video_format::`wildcard`.  
`avg_bit_rate`| Indicates the average bit rate of the video.  
`max_bit_rate`| Specifies the maximum bit rate that occurs at any point in the video.  
`encoding`| Has been depreciated, and should always be B_ANY; use the [`BMediaFormats`](BMediaFormats.md "BMediaFormats") lookup database to identify the encoding format.  
`frame_size`| Indicates the size in bytes of a frame of data.  
`forward_history`| Indicates the maximum number of future frames the encoding algorithm requires in order to decode the current frame.  
`backward_history`| Indicates the maximum number of past frames the encoding algorithm requires in order to decode the current frame.  
  
* * *

### media_encoded_video_header

Declared in: `media/MediaDefs.h`
    
    
    struct media_encoded_video_header {
        int32 _reserved_1[9];
        uint32 field_flags;
        int16 forward_history;
        int16 backward_history;
        uchar unsued_mask;
        uchar _rserved_2[3];
        float field_gamma;
        uint32 field_sequence;
        uint16 field_number;
        uint16 pulldown_number;
        uint16 first_active_line;
        uint16 line_count;
    };

Defines the header of a video buffer. The fields in this buffer are the same as those in the [media_video_header](TheMediaKit_DefinedTypes.md#media_video_header "media_video_header") structure, except for the reserved fields, the `forward_history` and `backward_history` fields, the `unused_mask`, which indicates which bits are unused in the last byte of the video buffer, and `field_flags`, which can be either 0 or `B_MEDIA_KEY_FRAME` (which indicates that the field is a key frame).

The `forward_history` field indicates how many buffers ahead of the current one are required in order to decode the current buffer. The `backward_history` field indicates the number of previous buffers are required to decode the current buffer.

Field| Description  
---|---  
`field_gamma`| Specifies the gamma at which the video was captured.  
`field_sequence`| Is an identifying number assigned to the field when it was captured. They progress in sequence; you can tell if a field was dropped by looking for skips in the sequence. Note that this value doesn't necessarily start at 0, and it can wrap back around to 0.  
`field_number`| Indicates which field of a video frame the buffer represents. It's 0 for field 1, 1 for field 2, and so forth.  
`pulldown_number`| Indicates that the field represented by the buffer is a duplicate of a field `pulldown_number` fields back. If this value is 0, it's not a duplicate field. This is used in cases when the video has been converted from one field rate to another, resulting in duplication of fields; these duplicated fields can be marked using pulldown numbers so video hardware and software can optimize their performance by knowing that duplicate fields exist.  
`first_active_line`| Indicates the topmost line that actually contains video data (instead of VBL data); this is an NTSC or PAL line number, which is 1-based.  
`line_count`| Indicates the number of active video lines in the field.  
  
You may add fields to the end of this buffer if your video format requires them.

* * *

### media_file_format

Declared in: `media/MediaDefs.h`
    
    
    struct media_file_format {
        enum {
           B_READABLE = 0x1,
           B_WRITABLE = 0x2,
           B_PERFECTLY_SEEKABLE = 0x4,
           B_IMPERFECTLY_SEEKABLE = 0x8,
           B_KNOWS_RAW_VIDEO = 0x10,
           B_KNOWS_RAW_AUDIO = 0x20,
           B_KNOWS_MIDI = 0x40,
           B_KNOWS_ENCODED_VIDEO = 0x80,
           B_KNOWS_ENCODED_AUDIO = 0x100,
           B_KNOWS_OTHER = 0x1000000,
           B_KNOWS_ANYTHING = 0x2000000
        };
        uint32 capabilities;
        media_file_format_id id;
        media_format_family family;
        int32 version;
    
        uint32 _reserved_[25];
        char mime_type[64];
        char pretty_name[64];
        char short_name[32];
        char file_extension[8];
        char reserved[88];
    };

Describes a media file format.

Field| Description  
---|---  
`capabilities`| Is a bitfield providing informational flags describing the file format's capabilities.

`B_READABLE`
    Indicates that the file format is readable.
`B_WRITABLE`
    Indicates that the format can be written.
`B_PERFECTLY_SEEKABLE`
    Indicates that seek operations are precise.
`B_IMPERFECTLY_SEEKABLE`
    Indicates that seek operations may not be totally accurate.
`B_KNOWS_RAW_VIDEO`
    Indicates that the file format can handle video.
`B_KNOWS_RAW_AUDIO`
    Indicates that the format can handle audio.
`B_KNOWS_MIDI`
    Indicates that the format can handle MIDI data.
`B_KNOWS_ENCODED_VIDEO`
    Indicates that the file format can handle encoded video.
`B_KNOWS_ENCODED_AUDIO`
    Indicates that the file format can handle encoded audio.
`B_KNOWS_OTHER`
    Indicates that the format can handle other media information, such as text, clipping, control changes, and so forth.
`B_KNOWS_ANYTHING`
    Indicates that the format can handle any kind of media data.  
`id`| Is an opaque ID used by the Media Kit to construct a [`BMediaFile`](BMediaFile.md "BMediaFile") object.  
`family`| Is one of the [media_format_family](TheMediaKit_Constants.md#Constants_media_format_family "media_format_family") constants.  
`version`| Should be 100.  
`mime_type`| Specifies the MIME type string that identifies the file format.  
`pretty_name`| Is a nice user-viewable name, such as "QuickTime File Format".  
`short_name`| Is a short-form name, like "QuickTime".  
`file_extension`| Specifies the file extension used on files containing this format, such as ".mov".  
  
* * *

### media_file_format_id

Declared in: `media/MediaDefs.h`
    
    
    struct media_file_format_id {
        ino_t node;
        dev_t device;
        uint32 internal_id;
    };

A simple means for identifying file formats; used by the [media_file_format](TheMediaKit_DefinedTypes.md#media_file_format "media_file_format") structure.

* * *

### media_format

Declared in: `media/MediaDefs.h`
    
    
    struct media_format {
        media_type type;
        type_code user_data_type;
        uchar user_data[48];
        uint32 _reserved_[3];
        uint16 require_flags;
        uint16 deny_flags;
    
    private:
        void* meta_data;
        int32 meta_data_size;
        area_id meta_data_area;
        area_id use_area;
        team_id team;
        void* thisPtr;
    
    public:
        union {
           media_multi_audio_format raw_audio;
           media_raw_video_format raw_video;
           media_multistream_format multistream;
           media_encoded_audio_format encoded_audio;
           media_encoded_video_format encoded_video;
           char _reserved_[96];
        } u;
        bool IsVideo() const ;
        uint32 Width() const;
        uint32 Height() const;
        color_space ColorSpace() const;
        uint32& Width();
        uint32& Height();
        color_space& ColorSpace();
    
        bool IsAudio() const;
        uint32& AudioFormat();
        uint32 AudioFormat() const;
        uint32 Encoding() const;
        bool Matches(const media_format* otherFormat) const;
        void SpecializeTo(const media_format* otherFormat);
        void SetMetaData(const void* data, int32 size);
        const void* MetaData() const;
        int32 MetaDataSize() const;
    
        media_format();
        media_format(const media_format& other);
        ~media_format();
        media_format& operator=(const media_format& clone);
    };

The media_format structure can describe any media data format the BeOS can support.

Field| Description  
---|---  
`type`| Indicates the type of media the format describes.  
`user_data`| Allows the application to store custom data in the media_format structure  
`user_data_type`| specifies the type of data stored in `user_data` (`B_INT32_TYPE`, `B_STRING_TYPE`, etc).  
`require_flags` and `deny_flags`| Are [media_format_flags](TheMediaKit_Constants.md#Constants_media_format_flags "media_format_flags") indicating what options are required, and what options are rejected.  
`meta_data`, `meta_data_size`, `meta_data_area`, `use_area`, `team`, `thisPtr`| Are currently only used by decoders when communicating with the file reader; they're not currently for public use.  
  
The union, `u`, permits the selection of a specific media format, such as raw audio or multistream data, and contains the descriptive information specific to that media format.

If any field is 0, it's treated as a wildcard.

There are several convenience functions for fetching information from the format that take care of dealing with different types automatically:

Function| Description  
---|---  
`IsVideo()`| Returns `true` if the format represents video (either raw or encoded), otherwise it returns `false`.  
`Width()`| Returns the width of the video data; its value is undetermined if the format doesn't represent video.  
`Height()`| Returns the height of the video data; its value is undetermined if the format doesn't represent video.  
`ColorSpace()`| Returns the video's color space. Its value is undetermined if the format doesn't represent video.  
`IsAudio()`| Returns `true` if the format represents audio (either raw or encoded), otherwise it returns `false`.  
`AudioFormat()`| Returns the audio's sample format.  
`Matches()`| Returns `true` if the format matches the specified otherFormat, otherwise it returns `false`.  
`SpecializeTo()`| Adjusts the specified otherFormat as needed to match the format.  
  
* * *

### media_header

Declared in: `media/MediaDefs.h`
    
    
    struct media_header {
        media_type type;
        media_buffer_id buffer;
        int32 destination;
        media_node_id time_source;
        uint32 _depreciated_;
        uint32 size_used;
        bigtime_t start_time;
        area_id owner;
        enum {
           B_SEEK_TAG = 'TRST'
        };
        type_code user_data_type;
        uchar user_data[64];
        uint32 _reserved_[2];
        off_t file_pos;
        size_t orig_size;
        uint32 data_offset;
        union {
           media_audio_header raw_audio;
           media_video_header raw_video;
           media_multistream_header multistream;
           media_encoded_audio_header encoded_audio;
           media_encoded_video_header encoded_video;
           char _reserved_[64];
        } u;
    };

Defines the header of a media data buffer.

Field| Description  
---|---  
`type`| Indicates the type of media the format describes.  
`buffer`| Indicates what buffer the header goes with. This field is filled out by the [`BBufferProducer::SendBuffer()`](BBufferProducer.md#BBufferProducer_SendBuffer "SendBuffer\(\)") function.  
`destination`| Specifies the ID number of the [media_destination](TheMediaKit_DefinedTypes.md#media_destination "media_destination") to which the buffer is being sent. This field is filled out by the [`BBufferProducer::SendBuffer()`](BBufferProducer.md#BBufferProducer_SendBuffer "SendBuffer\(\)") function.  
`time_source`| Indicates the ID number of the node that encoded the `start_time`.  
`size_used`| Specifies how many bytes of the buffer actually contain data.  
`start_time`| Indicates the performance time at which the buffer should be performed.  
`owner`| Specifies the area ID of the buffer's owner.  
`user_data_type` and `user_data`| Allow the application to store custom data in the media_format structure; `user_data_type` specifies the type of data stored in `user_data` (`B_INT32_TYPE`, `B_STRING_TYPE`, etc). The `B_SEEK_TAG` constant indicates that the `user_data` contains a seek tag.   
`file_pos` and `orig_size`| `file_pos` indicates where in the corresponding file the data came from, and `orig_size` indicates how big it was; if the data didn't come from a file, these should be zero.  
`data_offset`| Indicates the offset within the buffer to the data. This is reflected by the [`BBuffer::Data()`](BBuffer.md#BBuffer_Data "Data\(\)") function.  
  
The union, `u`, permits the selection of a specific media format, such as raw audio or multistream data, and contains the descriptive information specific to that media format.

* * *

### media_input

Declared in: `media/MediaDefs.h`
    
    
    struct media_input {
        media_input();
        ~media_input();
        media_node node;
        media_source source;
        media_destination destination;
        media_format format;
        char name[B_MEDIA_NAME_LENGTH];
    private:
        char _reserved_media_input_[4];
    };

The media_input structure describes a complete connection between a [media_source](TheMediaKit_DefinedTypes.md#media_source "media_source") and [media_destination](TheMediaKit_DefinedTypes.md#media_destination "media_destination"), from the point-of-view of the consumer (the destination).

Field| Description  
---|---  
`node`| Is the [media_node](TheMediaKit_DefinedTypes.md#media_node "media_node") that owns the input.  
`source`| Is the source socket from which the data is being sent.  
`destination`| Is the destination socket at which the data is arriving.  
`format`| Is the format of the data.  
`name`| Is the input's name.  
  
* * *

### media_multistream_format

Declared in: `media/MediaDefs.h`
    
    
    struct media_multistream_format {
        enum {
            B_ANY = 0,
            B_VID = 1,
            B_AVI,
            B_MPEG1,
            B_MPEG2,
            B_QUICKTIME,
            B_PRIVATE = 90000,
            B_FIRST_USER_TYPE = 100000
        };
        float avg_bit_rate;
        float max_bit_rate;
        uint32 avg_chunk_size;
        uint32 max_chunk_size;
        enum {
            B_HEADER_HAS_FLAGS = 0x1,
            B_CLEAN_BUFFERS = 0x2,
            B_HOMOGENOUS_BUFFERS = 0x4
        };
        uint32 flags;
        int32 format;
        uint32 _reserved_[2];
        struct vid_info {
            float frame_rate;
            uint16 width;
            uint16 height;
            color_space space;
            float sampling_rate;
            uint32 sample_format;
            uint16 byte_order;
            uint16 channel_count;
        };
        struct avi_info {
            uint32 us_per_frame;
            uint16 width;
            uint16 height;
            uint16 _reserved_;
            uint16 type_count;
            media_type types[5];
        };
        union {
            vid_info vid;
            avi_info avi;
        } u;
        static media_multistream_format wildcard;
    };

Describes the format of multistream media data, such as a QuickTime, AVI, or MPEG movie.

Field| Description  
---|---  
`avg_bit_rate` and `max_bit_rate`| Indicate the average and maximum bit rates of the media data.  
`avg_chunk_size` and `max_chunk_size`| Indicate the average and maximum buffer sizes; if all buffers are the same size, these two values should be equal.  
`flags`| Provides informational flags for the buffers.

`B_HEADER_HAS_FLAGS`
    Indicates that the buffers' headers have valid flags;
`B_CLEAN_BUFFERS`
    Indicates that each buffer represents an integral number of frames
`B_HOMOGENOUS_BUFFERS`
    Indicates that each buffer has only one format in it.  
`format`| Indicates the multistream data format `B_AVI`, `B_QUICKTIME`, etc).  
The union `u`| contains either a vid_info or an avi_info structure that describes the video stream:  
  
vid_info

Field| Description  
---|---  
`frame_rate`| Specifies the video frame rate (in frames per second).  
`width` and `height`| Indicate the width and height of the video, in pixels.  
`space`| Indicates the video's color space.  
`sampling_rate`, `sample_format`, `byte_order`, and `channel_count`| Specify the sampling rate, sample format, byte order, and number of channels (stereo or mono) of the media's audio.  
  
avi_info

Field| Description  
---|---  
`us_per_frame`| Specifies the number of microseconds each frame should be displayed.  
`width` and `height`| Indicate the width and height of the video in pixels.  
`type_count`| Indicates the number of media_types used in the AVI movie.  
`types`| Is a list of the media_types used.  
  
* * *

### media_multi_audio_format

Declared in: `media/MediaDefs.h`
    
    
    struct media_multi_audio_format {
        media_raw_audio_format raw;
        media_multi_audio_info multi;
    };

Describes a multi-channel audio format. This is constructed by using a media_raw_audio_format to describe the physical format of the sound, followed by a media_multi_audio_info structure that adds the multi-channel information. This allows compatibility with existing BeOS software while adding support for multi-channel audio in new software.

* * *

### media_multi_audio_info

Declared in: `media/MediaDefs.h`
    
    
    struct media_multi_audio_info {
        uint32 channel_mask;
        int16 valid_bits;
        uint16 matrix_mask;
        uint32 _reserved_b[3];
    };

Provides information about multi-channel audio support.

Field| Description  
---|---  
`channel_mask`| Indicates which channels are supported; for each channel (`B_CHANNEL_LEFT`, etc) the corresponding bit is set in this mask.  
`valid_bits`| Indicates how many bits in a 32-bit integer sample actually contain sample data.  
`matrix_mask`| Indicates which multi-channel audio matrices are used.  
  
* * *

### media_multistream_header

Declared in: `media/MediaDefs.h`
    
    
    struct media_multistream_header {
        uint32 _reserved_[14];
        uchar unused_mask;
        uchar _reserved_2[3];
        enum {
           B_MASTER_HEADER = 0x1,
           B_SUBSTREAM_HEADER = 0x2,
           B_COMPLETE_BUFFER = 0x4
        };
        uint32 flags;
    };

Defines the header of a multistream media buffer.

Field| Description  
---|---  
`unused_mask`| Indicates which bits of the last byte in the buffer are unused.  
`flags`| Provides informational flags about the buffer. If `B_MASTER_HEADER` is specified, master stream header data is in the buffer; `B_SUBSTREAM_HEADER` indicates that the buffer contains substream header information. `B_COMPLETE_BUFFER` indicates that the buffer's data represents an integral number of frames.  
  
You may add fields to the end of this structure if your multistream media format requires them.

* * *

### media_node

Declared in: `media/MediaDefs.h`
    
    
    class media_node {
    public:
        media_node();
        ~media_node();
        media_node_id node;
        port_id port;
        uint32 kind;
        static media_node null;
    private:
        uint32 _reserved_[3];
    };

The `media_node` structure is the class that defines a node to client applications; interactions with the [`BMediaRoster`](BMediaRoster.md "BMediaRoster") regarding nodes is done using `media_node` objects.

Field| Description  
---|---  
`node`| Indicates the node's ID number.  
`port`| Specifies the port ID number of the node's control port.  
`kind`| Specifies the node's media kind. See [node_kind](TheMediaKit_Constants.md#Enums_node_kind "node_kind") for a list of node kinds.  
`null`| Represents a null node  
  
Applications will usually treat this as a black box.

* * *

### media_node_attribute

Declared in: `media/MediaDefs.h`
    
    
    struct media_node_attribute {
        enum {
           B_R40_COMPILED = 1,
           B_USER_ATTRIBUTE_NAME = 0x1000000,
           B_FIRST_USER_ATTRIBUTE
        };
        uint32 what;
        uint32 flags;
        uint64 data;
    };

The media_node_attribute structure provides additional information about nodes. Nodes can have as many of these as they want, and will return them from their [`BMediaNode::GetNodeAttributes()`](BMediaNode.md#BMediaNode_GetNodeAttributes "GetNodeAttributes\(\)") function. Applications query the node using the [`BMediaRoster::GetNodeAttributesFor()`](BMediaRoster.md#BMediaRoster_GetNodeAttributesFor "GetNodeAttributesFor\(\)") function.

The `what` field indicates the attribute type. `B_R40_COMPILED` indicates that the node was compiled using BeOS Release 4.0; [`BMediaNode`](BMediaNode.md "BMediaNode") returns this attribute automatically for nodes that were compiled using the Release 4.0 headers. Nodes may create their own attribute types, beginning at `B_FIRST_USER_ATTRIBUTE`.

The `B_USER_ATTRIBUTE_NAME` attribute specifies the name of a protocol for the user attributes implemented by the node. For example, if you devise a third-party standard for attribute values whose attribute IDs are greater than `B_FIRST_USER_ATTRIBUTE`, the value of `B_USER_ATTRIBUTE_NAME` would be the name of the standard.

The `flags` and `data` fields are used for different things depending on the `what` value; the `B_R40_COMPILED` attribute doesn't use them.

* * *

### media_node_id

Declared in: `media/MediaDefs.h`
    
    
    typdef int32 media_node_id;

A node ID number.

* * *

### media_output

Declared in: `media/MediaDefs.h`
    
    
    struct media_output {
        media_output();
        ~media_output();
        media_node node;
        media_source source;
        media_destination destination;
        media_format format;
        char name[B_MEDIA_NAME_LENGTH];
    
    private:
        char _reserved_media_output_[4];
    };

The media_output structure describes a complete connection between a [media_source](TheMediaKit_DefinedTypes.md#media_source "media_source") and [media_destination](TheMediaKit_DefinedTypes.md#media_destination "media_destination"), from the point-of-view of the consumer (the destination).

Field| Description  
---|---  
`node`| Is the [media_node](TheMediaKit_DefinedTypes.md#media_node "media_node") that owns the output.  
`source`| Is the source socket from which the data is being sent.  
`destination`| Is the destination socket to which the data is being sent.  
`format`| Is the format of the data.  
`name`| Is the output's name.  
  
* * *

### media_raw_audio_format

Declared in: `media/MediaDefs.h`
    
    
    struct media_raw_audio_format {
        enum {
            B_AUDIO_FLOAT = 0x24,
            B_AUDIO_INT = 0x4
            B_AUDIO_SHORT = 0x2,
            B_AUDIO_UCHAR = 0x11,
            B_AUDIO_CHAR = 0x1,
            B_AUDIO_SIZE_MASK = 0xf
        }
        float frame_rate;
        uint32 channel_count;
        uint32 format;
        uint32 byte_order;
        size_t buffer_size;
    
        static media_multi_audio_format wildcard;
    };

Describes the format of raw audio data.

Field| Description  
---|---  
`frame_rate`| Indicates the frame rate, in Hertz. One frame is one sample per channel (stereo buffers contain two samples per channel). For audio, each buffer contains multiple audio samples, whereas video contains one frame, or one field (half-frame) per buffer.  
`channel_count`| Indicates the number of channels of audio (typically this is either 1 for mono or 2 for stereo).  
`format`| Specifies the sample format (for compressed formats, see [media_encoded_audio_format](TheMediaKit_DefinedTypes.md#media_encoded_audio_format "media_encoded_audio_format")):

`B_AUDIO_FLOAT`
    Each sample is four bytes; 0 is the middle, -1.0 is the bottom, 1.0 is the top.
`B_AUDIO_INT`
    Each sample is four bytes; 0 is the middle, 0x80000001 is the bottom, 0x7FFFFFFF is the top.
`B_AUDIO_SHORT`
    Each sample is two bytes; 0 is the middle, -32767 is the bottom, 32767 is the top.
`B_AUDIO_UCHAR`
    Each sample is one byte; 128 is the midpoint, 1 is the bottom, 255 is the top.
`B_AUDIO_CHAR`
    Each sample is one byte; 0 is the midpoint, -127 is the bottom, 127 is the top.
It's guaranteed that (`format` & `B_AUDIO_SIZE_MASK`) will always be equal to the size of the sample.  
`byte_order`| Indicates the endianness of the data (either `B_MEDIA_BIG_ENDIAN` or `B_MEDIA_LITTLE_ENDIAN`).  
`buffer_size`| Indicates the size of each buffer, in bytes.  
  
* * *

### media_raw_video_format

Declared in: `media/MediaDefs.h`
    
    
    struct media_raw_video_format {
        float field_rate;
        uint32 interlace;
        uint32 first_active;
        uint32 last_active;
        uint32 orientation;
        uint16 pixel_width_aspect;
        uint16 pixel_height_aspect;
        media_video_display_info display;
    
        static media_raw_video_format wildcard;
    };

Describes the format of raw video data.

Field| Description  
---|---  
`field_rate`| Indicates the field rate. This is 59.94 for NTSC and 50.0 for PAL. If you use BT848 in non-interlaced mode (thereby picking out only every other field), you'd specify 29.97.  
`interlace`| Specifies how many fields there are per frame. A value of 1 indicates that the video is progressive (non-interlaced). NTSC video is interlaced such that there are two fields per frame, so for standard NTSC video, this value is 2.  
`first_active` and `last_active`| Indicate where the VBL lines at the top of the frame end, and where they resume at the bottom of the frame; video data is confined between these two scan lines. Usually you'll use 0 for `first_active` and `line_count`-1 for `last_active`; these indicate wildcard or "don't care" values.  
`orientation`| Indicates the orientation of the video; this is typically `B_VIDEO_TOP_LEFT_RIGHT`.  
`pixel_width_aspect` and `pixel_height_aspect`| Specify the numerator and denominator of the video's aspect ratio. If the aspect ratio is 4:3, these values will be 4 and 3, respectively.  
  
* * *

### media_request_info

Declared in: `media/MediaDefs.h`
    
    
    struct media_request_info {
        enum what_code {
           B_SET_VIDEO_CLIPPING_FOR = 1,
           B_REQUEST_FORMAT_CHANGE,
           B_SET_OUTPUT_ENABLED,
           B_SET_OUTPUT_BUFFERS_FOR,
           B_FORMAT_CHANGED = 4097
        };
        what_code what;
        int32 change_tag;
        status_t status;
        int32 cookie;
        void* user_data;
        media_source source;
        media_destination destination;
        media_format format;
        uint32 _reserved_[32];
    };

This structure contains information that describes a request that has been completed (or has failed). It gets passed into the [`BMediaNode::RequestCompleted()`](BMediaNode.md#BMediaNode_RequestCompleted "RequestCompleted\(\)") function.

Field| Description  
---|---  
`what`| Describes what type of request has been completed:

`B_SET_VIDEO_CLIPPING_FOR`
    Indicates that a [`BBufferConsumer::SetVideoClippingFor()`](BBufferConsumer.md#BBufferConsumer_SetVideoClippingFor "SetVideoClippingFor\(\)") request has been completed. The `source` and `destination` fields contain these values from the request, and `format`.`u`.`raw_video`.`display` describes the new video display settings.
`B_REQUEST_FORMAT_CHANGE`
    Indicates that a [`BBufferConsumer::RequestFormatChange()`](BBufferConsumer.md#BBufferConsumer_RequestFormatChange "RequestFormatChange\(\), FormatChanged\(\)") request has been completed. The `source` and `destination` fields contain these values from the request, and `format` contains the negotiated format (it may have been altered by the producer if there were wildcard fields).
`B_SET_OUTPUT_ENABLED`
    Indicates that a [`BBufferConsumer::SetOutputEnabled()`](BBufferConsumer.md#BBufferConsumer_SetOutputEnabled "SetOutputEnabled\(\)") request has been completed. The `source` and `destination` fields contain these values from the request, and `status` is always `B_OK`.
`B_SET_OUTPUT_BUFFERS_FOR`
    Indicates that a [`BBufferConsumer::SetOutputBuffersFor()`](BBufferConsumer.md#BBufferConsumer_SetOutputBuffersFor "SetOutputBuffersFor\(\)") request has been completed. The `source` and `destination` fields contain these values from the request, and `cookie` is a pointer to the [`BBufferGroup`](BBufferGroup.md "BBufferGroup") that was set. Note that the change was only applied if status is `B_OK`.
`B_FORMAT_CHANGED`
    Indicates that a [`BBufferConsumer::FormatChanged()`](BBufferConsumer.md#BBufferConsumer_FormatChanged) request has been completed. The `source` and `destination` fields contain these values from the request, and `format` indicates the new format. The values seen here are from the initator's point of view.  
`user_data`|  Contains the `userData` pointer passed to the function that initiated the request; this can be anything the node making the request wants.  
`status`| Indicates the status of the function called, and is `B_OK` if no error occurred.  
  
* * *

### media_seek_tag

Declared in: `media/MediaDefs.h`
    
    
    struct media_seek_tag {
        char data[16];
    };

The media_seek_tag type is used to contain the seek tag data placed in buffer headers by producers. This can contain whatever information the producer deems necessary.

* * *

### media_source

Declared in: `media/MediaDefs.h`
    
    
    struct media_source {
        media_source(port_id, int32);
        media_source(const media_source& clone);
        media_source& operator= (const media_source& clone);
        media_source();
        ~media_source();
        port_id port;
        int32 id;
        static media_source null;
    private:
        uint32 _reserved_media_source_[2];
    };

Describes the source end of a media connection between two nodes. The `port` field indicates the port ID on which the source communicates. This must be the same as the `media_node`.`port` value.

* * *

### media_video_display_info

Declared in: `media/MediaDefs.h`
    
    
    struct media_video_display_info {
        color_space format;
        uint32 line_width;
        uint32 line_count;
        uint32 bytes_per_row;
        uint32 pixel_offset;
        uint32 line_offset;
        uint32 flags;
        uint32 _reserved_[3];
        static media_video_display_info wildcard;
    };

Describes a video buffer in terms of how its contents should be interpreted in order to display it on the screen.

Field| Description  
---|---  
`format`| Specifies the buffer's color space.  
`line_width` and `line_count`| Indicate the total width of the buffer in pixels, and the total height of the buffer in lines.  
`bytes_per_row`| Specifies the number of bytes wide the buffer's image is, including any slop space.  
`pixel_offset`| Indicates the number of pixels between the start of the buffer and the start of the video field. The buffer is the framebuffer.  
`line_offset`| Specifies the offset between the start of the buffer and the start of the field, in lines. The buffer is the framebuffer.  
`flags`| Are [media_display_flags](TheMediaKit_Constants.md#Constants_media_display_flags "media_display_flags"), indicating the dominance and order of the video fields.  
  
* * *

### media_video_header

Declared in: `media/MediaDefs.h`
    
    
    struct media_video_header {
        uint32 _reserved_[12];
        float field_gamma;
        uint32 field_sequence;
        uint16 field_number;
        uint16 pulldown_number;
        uint16 first_active_line;
        uint16 line_count;
    };

Defines the header of a video buffer.

Field| Description  
---|---  
`field_gamma`| Specifies the gamma at which the video was captured.  
`field_sequence`| Is an identifying number assigned to the field when it was captured. They progress in sequence; you can tell if a field was dropped by looking for skips in the sequence. Note that this value doesn't necessarily start at 0, and it can wrap back around to 0.  
`field_number`| Indicates which field of a video frame the buffer represents. It's 0 for field 1, 1 for field 2, and so forth.  
`pulldown_number`| Indicates that the field represented by the buffer is a duplicate of a field `pulldown_number` fields back. If this value is 0, it's not a duplicate field. This is used in cases when the video has been converted from one field rate to another, resulting in duplication of fields; these duplicated fields can be marked using pulldown numbers so video hardware and software can optimize their performance by knowing that duplicate fields exist.  
`first_active_line`| Indicates the topmost line that actually contains video data (instead of VBL data); this is an NTSC or PAL line number, which is 1-based.  
`line_count`| Indicates the number of active video lines in the field.  
  
You may add fields to the end of this buffer if your video format requires them.

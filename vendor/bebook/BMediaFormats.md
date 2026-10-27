<!-- Classes And Methods - The Media Kit - BMediaFormats -->

## BMediaFormats

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `media/MediaFormats.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BMediaFormats_Overview.md "BMediaFormats")

  * [Constructor and Destructor](BMediaFormats.md#BMediaFormats_ConstructorDestructor)
  * [Member Functions](BMediaFormats.md#BMediaFormats_MemberFunctions)
  * [Constants](BMediaFormats.md#BMediaFormats_Constants)
  * [Defined Types](BMediaFormats.md#BMediaFormats_DefinedTypes)
  * [Global C Functions](BMediaFormats.md#BMediaFormats_Functions)

  
  
* * *

### Constructor and Destructor

#### BMediaFormats()

`BMediaFormats();`

Constructor. Call [`InitCheck()`](BMediaFormats.md#BMediaFormats_InitCheck "InitCheck\(\)") to ensure that the object was initialized properly after you instantiate it.

#### ~BMediaFormats()

`virtual ~BMediaFormats();`

Destructor.

* * *

### Member Functions

####  GetBeOSFormatFor(), GetAVIFormatFor(), GetQuicktimeFormatFor()

`status_t GetBeOSFormatFor(uint32 format,  
media_format* outFormat,  
media_type type = B_MEDIA_UNKNOWN_TYPE);``status_t GetAVIFormatFor(uint32 format,  
media_format* outFormat,  
media_type type = B_MEDIA_UNKNOWN_TYPE);``status_t GetQuicktimeFormatFor(uint32 vendor,  
uint32 codec,  
media_format* outFormat,  
media_type type = B_MEDIA_UNKNOWN_TYPE);`

`GetBeOSFormatFor()` returns in `outFormat` a [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") structure that describes the media format corresponding to the given BeOS `format` ID and [media_type](TheMediaKit_Constants.md#Constants_media_type "media_type").

`GetAVIFormatFor()` returns in `outFormat` a [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") structure that describes the media format corresponding to the given AVI `format` ID and [media_type](TheMediaKit_Constants.md#Constants_media_type "media_type").

`GetQuicktimeormatFor()` returns in `outFormat` a [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") structure that describes the media format corresponding to the given QuickTime `vendor` ID, `codec` ID, and [media_type](TheMediaKit_Constants.md#Constants_media_type "media_type").

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_BAD_TYPE`| The specified [media_type](TheMediaKit_Constants.md#Constants_media_type "media_type"). isn't supported.  
`B_MEDIA_BAD_FORMAT`| The specified combination of parameters isn't supported.  
Other errors| The format couldn't be determined.  
  
#### GetCodeFor()

`status_t GetCodeFor(const media_format& format,  
media_format_family family,  
media_format_description* outDescription);`

Given the specified media format, returns in `outDescription` the media format description for the specified `format` family.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_MEDIA_BAD_FORMAT`| The specified format isn't recognized.  
Other errors| The code couldn't be determined.  
  
#### GetFormatFor()

`status_t GetFormatFor(const media_format_description& description,  
media_format* outFormat);`

Returns in `outFormat` a [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") structure that describes the media format specified by `description`. This lets you easily obtain a [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") structure from file-native description information.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_MEDIA_BAD_FORMAT`| The specified description isn't recognized.  
Other errors| The format couldn't be determined.  
  
#### GetNextFormat()

`status_t GetFormatFor(media_format* outFormat,  
media_format_description* outDescription);`

Fills `outFormat` and `outDescription` with the [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") and [media_format_description](BMediaFormats.md#media_format_description "media_format_description") of the next supported media format.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_BAD_INDEX`| No more formats to return.  
Other errors| The format couldn't be determined.  
  
#### InitCheck()

`status_t InitCheck();`

Returns the status code from the constructor; you should call this to be sure the object was properly initialized before issuing any other calls.

Return Code| Description  
---|---  
`B_OK`| The object is ready to use.  
`B_BAD_HANDLER`| The object isn't ready.  
Other errors| The Media Server couldn't be contacted.  
  
####  Lock(), Unlock()

`bool Lock();``void Lock();`

`Lock()` locks the `BMediaFormats` object so you can use it with the assurance that it won't change while you're working. You should call this before using any of the other `BMediaFormats` functions. Returns `true` if it's safe to use the `BMediaFormats` object; otherwise returns `false`.

`Unlock()` releases the `BMediaFormats` object when you're done with it; be sure to call it when you've finished your work.

Note

![Note](./images/admonitions/Info_32.png)

You only need to `Lock()` and `Unlock()` the `BMediaFormats` object when you're using [`RewindFormats()`](BMediaFormats.md#BMediaFormats_RewindFormats "RewindFormats\(\)") and [`GetNextFormat()`](BMediaFormats.md#BMediaFormats_GetNextFormat "GetNextFormat\(\)").

#### MakeFormatFor()

`status_t MakeFormatFor(const media_format_description* descriptions,  
int32 descCount,  
media_format* ioFormat,  
uint32 flags = 0,  
void* _reserved = 0);`

Given the media format descriptions in the `descriptions` array, the number of descriptions in the list, `descCount`, and the [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") `ioFormat`, registers your format description and supported [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") with the Media Kit, and returns a [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") you can use from then on.

This function is called to let the Media Server assign you an encoding value for your [media_format_description](BMediaFormats.md#media_format_description "media_format_description"). If you're implementing a codec add-on or a file parser, you should call this function to let the Media Kit know what you handle.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_BAD_VALUE`| The `descriptions` pointer is `NULL`, the `descCount` is less than 1, or `ioFormat` is `NULL`.  
`B_ERROR`| Some other unfortunate incident occurred.  
  
#### RewindFormats()

`status_t RewindFormats();`

Resets format scanning to the first supported format; this causes [`GetNextFormat()`](BMediaFormats.md#BMediaFormats_GetNextFormat "GetNextFormat\(\)") to start at the beginning of the list.

Return Code| Description  
---|---  
`B_OK`| The object is ready to use.  
Other errors.| Unable to communicate with the Media Server.  
  
* * *

### Constants

####  make_format_flags

Declared in: `media/MediaFormats.h`

Constant| Description  
---|---  
`B_EXCLUSIVE`| Fail if this format has already been registered.  
`B_NO_MERGE`| Don't renumber any formats if there are multiple clashing previous registrations, but fail instead.  
`B_SET_DEFAULT`| Set the first format to be the default for the format family when registering more than one in the same family. Use only in encoder add-ons.  
  
These constants provide options for how to go about adding new formats.

* * *

### Defined Types

#### media_aiff_description

Declared in: `media/MediaFormats.h`
    
    
    typedef struct {
       uint32 codec;
    } media_aiff_description;

Describes the format of media data as an AIFF codec ID.

#### media_asf_description

Declared in: `media/MediaFormats.h`
    
    
    typedef struct {
       GUID guid;
    } media_asf_description;

Describes the format of media data as a 128-bit ASF GUID.

#### media_avi_description

Declared in: `media/MediaFormats.h`
    
    
    typedef struct {
       uint32 codec;
    } media_avi_description;

Describes the format of media data as an AVI codec ID.

#### media_avr_description

Declared in: `media/MediaFormats.h`
    
    
    typedef struct {
       uint32 id;
    } media_avr_description;

Describes the format of media data as an AVR codec ID.

#### media_beos_description

Declared in: `media/MediaFormats.h`
    
    
    typedef struct {
       int32 format;
    } media_beos_description;

Describes the format of media data in a way the BeOS Media Kit understands and appreciates. The `format` field has no predetermined meaning; it's a magic number that only the Media Kit understands.

#### media_codec_info

Declared in: `media/MediaFormats.h`
    
    
    struct encoder_info {
       char pretty_name[96];
       char short_name[32];
    
       int32 id;
       int32 sub_id;
    
       int32 pad[63];
    };

Provides information about an encoder. The `pretty_name` is a complete human-readable name (such as "SuperSqueeze by Applied Interdynamic Systems, Inc."), and the `short_name` is a short-form version of the encoder's name (such as "SuperSqueeze").

The `id` is an opaque ID number that gets passed to [`BMediaFile::CreateTrack()`](BMediaFile.md#BMediaFile_CreateTrack "CreateTrack\(\)") so the track can identify the encoder.

The padding is reserved space for future expansion of the structure.

#### media_file_format_info

Declared in: `media/MediaFormats.h`
    
    
    struct media_file_format_info {
       char mimetype[64];
       char pretty_name[64];
       char short_name[32];
       char file_extension[8];
    
       media_format_family family;
    
       int64 capabilities;
       int32 id;
    
       int32 pad[64];
    };

Describes a media file format. This is used when using [`BMediaFile`](BMediaFile.md "BMediaFile") objects to determine the type of media file being used.

Field| Description  
---|---  
`mimetype`| Is the file format's MIME type string.  
`pretty_name`| Is a human-readable file format name, such as "QuickTime file".  
`short_name`| Is a human-readable short name, such as "QuickTime".  
`file_extension`| Indicates the dot-extension that untyped files of this format might have, such as ".mov".  
`family`| Indicates the BeOS file format family of the format, such as `B_MEDIA_QUICKTIME_FAMILY`.  
`capabilities`| Contains flags indicating what features the format supports.  
`id`| Is a unique ID number for the format.  
`pad`| Pads out the structure, and is reserved.  
  
#### media_format_description

Declared in: `media/MediaFormats.h`
    
    
    typedef struct _media_format_description {
    #if defined(__cplusplus)
       _media_format_description();
       ~_media_format_description();
       _media_format_description(const _media_format_description& other);
       _media_format_description& operator=(const _media_format_description& other);
    #endif
       media_format_family family;
       uint32 _reserved_[3];
       union {
          media_beos_description beos;
          media_quicktime_description quicktime;
          media_avi_description avi;
          media_asf_description asf;
          media_mpeg_description mpeg;
          media_wav_description wav;
          media_aiff_description aiff;
          media_misc_description misc;
          media_avr_description avr;
          uint32 _reserved_[12];
       } u;
    } media_format_description;

Describes a media format. The family indicates which media family in the union the description uses to identify the format.

#### media_misc_description

Declared in: `media/MediaFormats.h`
    
    
    typedef struct {
       uint32 file_format;
       uint32 codec;
    } media_misc_description;

Describes the format of a miscellaneous media file.

#### media_mpeg_description

Declared in: `media/MediaFormats.h`
    
    
    typedef struct {
       uint32 id;
    } media_mpeg_description;

Describes the format of media data as an MPEG format ID; this can be any of the following values:

Constant| Description  
---|---  
`B_MPEG_ANY`| An MPEG format not listed among these choices.  
`B_MPEG_1_AUDIO_LAYER_1`| MPEG 1 Layer 1 audio format.  
`B_MPEG_1_AUDIO_LAYER_2`| MPEG 1 Layer 2 audio format.  
`B_MPEG_1_AUDIO_LAYER_3`| MPEG 1 Layer 3 audio format (commonly known as "MP3").  
`B_MPEG_1_VIDEO`| An MPEG 1 viedo stream.  
  
#### media_quicktime_description

Declared in: `media/MediaFormats.h`
    
    
    typedef struct {
       uint32 file_format;
       uint32 codec;
    } media_misc_description;

Describes the format of media data as a file format/codec pair.

#### media_wav_description

Declared in: `media/MediaFormats.h`
    
    
    typedef struct {
       uint32 codec;
    } media_wav_description;

Describes the format of media data as a WAV codec ID.

* * *

### Global C Functions

#### does_file_accept_format()

`bool does_file_accept_format(const media_file_format* mfi,  
const media_format* format);`

Returns `true` if the given [media_file_format](TheMediaKit_DefinedTypes.md#media_file_format "media_file_format") can contain data in the specified [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format"). Otherwise, `false` is returned.

#### get_next_encoder()

`status_t get_next_encoder(int32* cookie,  
media_file_format* mfi,  
media_format* outputFormat,  
media_codec_info* codecInfo);` `status_t get_next_encoder(int32* cookie,  
media_file_format* mfi,  
media_format* inputFormat,  
media_format* outputFormat,  
media_codec_info* codecInfo,  
media_format* acceptedInputFormat,  
media_format* acceptedOutputFormat);` `status_t get_next_encoder(int32* cookie,  
media_codec_info* codecInfo);`

Given a [media_file_format](TheMediaKit_DefinedTypes.md#media_file_format "media_file_format"), `mfi`, describing the format of a media file, a [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format"), `outputFormat`, describing the desired output format, and optionally a [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format"), `inputFormat` describing the desired input format, returns in `codecInfo` information about the next encoder capable of handling the format.

You should set the int32 pointed to by `cookie` to 0 initially, and pass the same pointer each time you call `get_next_encoder()`, to scan through all possible encoders. When there are no more encoders left to check, `B_BAD_INDEX` is returned.

If you use the second form of this function, on return the `acceptedInputFormat` and `acceptedOutputFormat` [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format"), structures will be filled out to indicate the formats that the codec will accept. This form of the function supports wildcards, while the first does not.

The third form of the function is less specific, and can be used to get a complete list of codecs. This can be handy if you're constructing a user interface to allow the user to select the codec they'd like to use, to get a full list of all the possible formats they can create.

Return Code| Description  
---|---  
`B_OK`| An encoder was found and returned.  
`B_BAD_INDEX`| The cookie was invalid, or no more encoders found.  
  
See also: [`get_next_file_format()`](TheMediaKit_Functions.md#get_next_file_format "get_next_file_format\(\)") in the global C functions section.

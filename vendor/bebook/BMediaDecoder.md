<!-- Classes And Methods - The Media Kit - BMediaDecoder -->

## BMediaDecoder

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `media/MediaDecoder.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BMediaEncoderDecoder_Overview.md "BMediaEncoder & BMediaDecoder")

  * [Constructor and Destructor](BMediaDecoder.md#BMediaDecoder_ConstructorDestructor)
  * [Hook Functions](BMediaDecoder.md#BMediaDecoder_HookFunctions)
  * [Member Functions](BMediaDecoder.md#BMediaDecoder_MemberFunctions)

  
  
* * *

### Constructor and Destructor

`BMediaDecoder();` `BMediaDecoder(const media_format* inFormat,  
const void* info = NULL,  
size_t infoSize = 0);` `BMediaDecoder(const media_codec_info* mci);`

The constructor sets up the `BMediaDecoder`. If you use the empty form of the constructor, you'll have to call [`SetTo()`](BMediaDecoder.md#BMediaDecoder_SetTo "SetTo\(\)") to establish the format to be decoded before calling [`Decode()`](BMediaDecoder.md#BMediaDecoder_Decode "Decode\(\)").

The second form accepts a [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") structure, `inFormat`, that indicates the type of media data that will be input into the decoder. `info`, if specified, will be filled out with text information about the node; you must specify a buffer `infoSize` bytes long.

The third form of the constructor accepts a [media_codec_info](BMediaFormats.md#media_codec_info "media_codec_info") structure, `mci`, that determines which codec should be used.

Note

![Note](./images/admonitions/Info_32.png)

If you use either the [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") or [media_codec_info](BMediaFormats.md#media_codec_info "media_codec_info") form of the constructor, you must call [`InitCheck()`](BMediaDecoder.md#BMediaDecoder_InitCheck "InitCheck\(\)") to ensure that construction was successful before using any other functions in this class.

#### ~BMediaDecoder()

`virtual ~BMediaDecoder();`

Releases the decoder add-on being used by the `BMediaDecoder`.

* * *

### Hook Functions

#### GetNextChunk()

##### protected

`status_t GetNextChunk(const void** chunkData,  
size_t* chunkLen,  
media_header* mh);`

In derived classes, you should implement this function to fetch the media data from the source. Set `chunkData` to be a pointer to the next chunk of media data, and `chunkLen` to the size of that buffer. The [media_header](TheMediaKit_DefinedTypes.md#media_header "media_header") structure `mh` provides information you can use while fetching the chunk.

This hook is called by the decoder add-on in order to fetch the data from the source.

Return `B_OK` if the chunk is fetched safely, or an appropriate error code otherwise.

* * *

### Member Functions

#### Decode()

`status_t Decode(void* outBuffer,  
int64* frameCount,  
media_header* outMH,  
media_decode_info* info);`

Decodes a chunk of media data into the output buffer specified by `outBuffer`. On return, `outFrameCount` is set to indicate how many frames of data were decoded, and `outMH` is the header for the decoded buffer.

The [media_decode_info](TheMediaKit_DefinedTypes.md#media_decode_info "media_decode_info") structure info is used on input to specify decoding parameters.

The amount of data decoded is part of the format determined by [`SetTo()`](BMediaDecoder.md#BMediaDecoder_SetTo "SetTo\(\)") or [`SetInputFormat()`](BMediaDecoder.md#BMediaDecoder_SetInputFormat "SetInputFormat\(\), SetOutputFormat\(\)"). For audio, it's the buffer size. For video, it's one frame, which is height*row_bytes. The data to be decoded will be fetched from the source by the decoder add-on calling the derived class' [`GetNextChunk()`](BMediaDecoder.md#BMediaDecoder_GetNextChunk "GetNextChunk\(\)") function.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_NO_INIT`| The `BMediaDecoder` hasn't been initialized.  
Other errors.| The decoder's `Decode()` function can return errors.  
  
#### GetDecoderInfo()

`status_t GetDecoderInfo(media_codec_info* outInfo) const;`

Fills out the [media_codec_info](BMediaFormats.md#media_codec_info "media_codec_info") structure `outInfo` with information about the decoder being used by the `BMediaDecoder`.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_NO_INIT`| The `BMediaDecoder` hasn't been initialized.  
Other errors.| The decoder's `GetCodecInfo()` function can return errors.  
  
#### InitCheck()

`status_t InitCheck() const;`

Returns a status_t value indicating whether or not construction was successful. You must call this function after construction before calling any other `BMediaDecoder` functions.

Return Code| Description  
---|---  
`B_OK`| The constructor was successful.  
Other errors.| See [`SetTo()`](BMediaDecoder.md#BMediaDecoder_SetTo "SetTo\(\)").  
  
####  SetInputFormat(), SetOutputFormat()

`status_t SetInputFormat(const media_format* inFormat,  
const void* info = NULL,  
size_t infoSize = 0);``status_t SetOutputFormat(media_format* outputFormat);`

`SetInputFormat()` sets the input data format to `inFormat`. On return, `info` (if you don't specify `NULL`) is filled out by the decoder to contain whatever textual information the decoder wants to provide. `infoSize` must indicate the size of the buffer pointed to by `info`.

Unlike [`SetTo()`](BMediaDecoder.md#BMediaDecoder_SetTo "SetTo\(\)"), `SetInputFormat()` function does not select a codec, so the currently-selected codec will continue to be used. You should only use `SetInputFormat()` to refine the format settings if it will not require the use of a different decoder.

`SetOutputFormat()` sets the format the decoder should output. On return, the `outputFormat` is changed to match the actual format that will be output; this can be different if you specified any wildcards.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_NO_INIT`| The `BMediaDecoder` hasn't been initialized yet.  
Other errors.| The decoder's `Sniff()`, `Format()`,and `Reset()` functions can return errors through `SetInputFormat()`.  
  
#### SetTo()

`status_t SetTo(const media_format* inFormat,  
const void* info = NULL,  
size_t infoSize = 0);` `status_t SetTo(const media_codec_info* mci);`

`SetTo()` sets the format of media data that will be decoded by the `BMediaDecoder` object. This also causes the `BMediaDecoder` to locate an appropriate codec to use.

The first form accepts a [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") structure, `inFormat`, that indicates the type of media data that will be input into the decoder. `info`, if specified, will be filled out with text information about the node; you must specify a buffer `infoSize` bytes long.

The second form of `SetTo()` accepts a [media_codec_info](BMediaFormats.md#media_codec_info "media_codec_info") structure, `mci`, that determines which codec should be used.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_MEDIA_BAD_FORMAT`| The specified [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") isn't valid.  
`B_BAD_VALUE`| Something else went wrong.  
`B_ERROR`| Unable to instantiate the decoder.

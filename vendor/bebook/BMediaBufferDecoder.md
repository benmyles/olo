<!-- Classes And Methods - The Media Kit - BMediaBufferDecoder -->

## BMediaBufferDecoder

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `media/MediaBufferEncoder.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BMediaBufferEncoderDecoder_Overview.md "BMediaBufferEncoder & BMediaBufferDecoder")

  * [Constructor and Destructor](BMediaBufferDecoder.md#BMediaBufferDecoder_ConstructorDestructor)
  * [Member Functions](BMediaBufferDecoder.md#BMediaBufferDecoder_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BMediaBufferDecoder()

`BMediaBufferDecoder();` `BMediaBufferDecoder(const media_format* inFormat,  
const void* info = NULL,  
size_t infoSize = 0);` `BMediaBufferDecoder(const media_codec_info* mci);`

The constructor sets up the `BMediaBufferDecoder`. If you use the empty form of the constructor, you'll have to call [`SetTo()`](BMediaDecoder.md#BMediaDecoder_SetTo "SetTo\(\)") to establish the format to be decoded before calling [`Decode()`](BMediaDecoder.md#BMediaDecoder_Decode "Decode\(\)").

The second form accepts a [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") structure, `inFormat`, that indicates the type of media data that will be input into the decoder. `info`, if specified, will be filled out with text information about the node; you must specify a buffer `infoSize` bytes long.

The third form of the constructor accepts a [media_codec_info](BMediaFormats.md#media_codec_info "media_codec_info") structure, `mci`, that determines which codec should be used.

Note

![Note](./images/admonitions/Info_32.png)

If you use either the [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") or [media_codec_info](BMediaFormats.md#media_codec_info "media_codec_info") form of the constructor, you must call [`InitCheck()`](BMediaBufferDecoder.md#BMediaBufferDecoder_InitCheck "InitCheck\(\)") to ensure that construction was successful before using any other functions in this class.

#### ~BMediaBufferDecoder()

`virtual ~BMediaBufferDecoder();`

Releases the decoder add-on being used by the `BMediaBufferDecoder`.

* * *

### Member Functions

#### DecodeBuffer()

`status_t DecodeBuffer(const void* inputBuffer,  
size_t inputSize,  
void* outBuffer,  
int64* outFrameCount,  
media_header* outMH,  
media_decode_info* info);`

Decodes a chunk of media data from the input buffer `inputBuffer` into the output buffer specified by `outBuffer`. The input buffer contains `inputSize` bytes of media data. On return, `outFrameCount` is set to indicate how many frames of data were decoded, and outMH is the header for the decoded buffer.

The [media_decode_info](TheMediaKit_DefinedTypes.md#media_decode_info "media_decode_info") structure info is used on input to specify decoding parameters.

Return Code| Description  
---|---  
`B_OK`| No error.  
Other errors.| The decoder's [`Decode()`](BMediaDecoder.md#BMediaDecoder_Decode "Decode\(\)") function can return errors.  
  
#### GetNextChunk()

##### protected

`status_t GetNextChunk(const void** chunkData,  
size_t* chunkLen,  
media_header* mh)  = 0;`

Implementation detail. This function is implemented to fetch memory from the input buffer specified by the call to [`DecodeBuffer()`](BMediaBufferDecoder.md#BMediaBufferDecoder_DecodeBuffer "DecodeBuffer\(\)").

#### InitCheck()

`status_t InitCheck() const;`

Returns a status_t value indicating whether or not construction was successful. You must call this function after construction before calling any other `BMediaBufferDecoder` functions.

Return Code| Description  
---|---  
`B_OK`| The constructor was successful.  
Other errors.| See [`BMediaDecoder`](BMediaDecoder.md "BMediaDecoder")'s [`SetTo()`](BMediaDecoder.md#BMediaDecoder_SetTo "SetTo\(\)") function.

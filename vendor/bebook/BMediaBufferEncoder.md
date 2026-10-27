<!-- Classes And Methods - The Media Kit - BMediaBufferEncoder -->

## BMediaBufferEncoder  
  
| Derived From:| [`BMediaEncoder`](BMediaEncoder.md "BMediaEncoder")  
---|---  
Mix-in Classes:| –  
Declared In:| `media/MediaBufferEncoder.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BMediaBufferEncoderDecoder_Overview.md "BMediaBufferEncoder & BMediaBufferDecoder")

  * [Constructor and Destructor](BMediaBufferEncoder.md#BMediaBufferEncoder_ConstructorDestructor)
  * [Member Functions](BMediaBufferEncoder.md#BMediaBufferEncoder_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BMediaBufferEncoder()

`BMediaBufferEncoder();` `BMediaBufferEncoder(const media_format* outputFormat);` `BMediaBufferEncoder(const media_codec_info* mci);`

The constructor sets up the `BMediaBufferEncoder`. If you use the empty form of the constructor, you'll have to call [`SetTo()`](BMediaEncoder.md#BMediaEncoder_SetTo "SetTo\(\)") to establish the format to be encoded before calling [`Encode()`](BMediaEncoder.md#BMediaEncoder_Encode "Encode\(\)").

The second form accepts a [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") structure, `inFormat`, that indicates the type of media data that will be input into the encoder.

The third form of the constructor accepts a [media_codec_info](BMediaFormats.md#media_codec_info "media_codec_info") structure, `mci`, that determines which codec should be used.

Note

![Note](./images/admonitions/Info_32.png)

If you use either the [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") or [media_codec_info](BMediaFormats.md#media_codec_info "media_codec_info") form of the constructor, you must call [`InitCheck()`](BMediaBufferEncoder.md#BMediaBufferEncoder_InitCheck "InitCheck\(\)") to ensure that construction was successful before using any other functions in this class.

#### ~BMediaBufferEncoder()

`virtual ~BMediaBufferEncoder();`

Releases the encoder add-on being used by the `BMediaBufferEncoder`.

* * *

### Member Functions

#### EncodeToBuffer()

`status_t EncodeToBuffer(void* outputBuffer,  
size_t* outputSize,  
const void* inputBuffer,  
int64 frameCount,  
media_encode_info* info);`

Encodes a chunk of media data from the input buffer by `inputBuffer`, which contains `frameCount` frames of data. The encoded data is written into the buffer indicated by `outputBuffer`. On return, `outputSize` indicates how many bytes of data were written into the buffer.

The [media_encode_info](TheMediaKit_DefinedTypes.md#media_encode_info "media_encode_info") structure info is used on input to specify encoding parameters.

Return Code| Description  
---|---  
`B_OK`| No error.  
Other errors.| The encoder's [`Encode()`](BMediaEncoder.md#BMediaEncoder_Encode "Encode\(\)") function can return errors.  
  
#### InitCheck()

`status_t InitCheck() const;`

Returns a status_t value indicating whether or not construction was successful. You must call this function after construction before calling any other `BMediaBufferEncoder` functions.

Return Code| Description  
---|---  
`B_OK`| The constructor was successful.  
Other errors.| See [`BMediaEncoder`](BMediaEncoder.md "BMediaEncoder")'s [`SetTo()`](BMediaEncoder.md#BMediaEncoder_SetTo "SetTo\(\)") function.  
  
#### WriteChunk()

##### protected

`status_t WriteChunk(const void* chunkData,  
size_t chunkLength,  
media_encode_info* info)  = 0;`

Implemented to write the encoded media data into the buffer specified when [`EncodeToBuffer()`](BMediaBufferEncoder.md#BMediaBufferEncoder_EncodeToBuffer "EncodeToBuffer\(\)") was called.

<!-- Classes And Methods - The Media Kit - BMediaEncoder -->

## BMediaEncoder  
  
| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `media/MediaEncoder.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BMediaEncoderDecoder_Overview.md "BMediaEncoder & BMediaDecoder")

  * [Constructor and Destructor](BMediaEncoder.md#BMediaEncoder_ConstructorDestructor)
  * [Member Functions](BMediaEncoder.md#BMediaEncoder_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BMediaEncoder()

`BMediaEncoder();` `BMediaEncoder(const media_format* outputFormat);` `BMediaEncoder(const media_codec_info* mci);`

The constructor sets up the `BMediaEncoder`. If you use the empty form of the constructor, you'll have to call [`SetTo()`](BMediaEncoder.md#BMediaEncoder_SetTo "SetTo\(\)") to establish the format to be encoded before calling [`Encode()`](BMediaEncoder.md#BMediaEncoder_Encode "Encode\(\)").

The second form accepts a [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") structure, `inFormat`, that indicates the type of media data that will be input into the encoder.

The third form of the constructor accepts a [media_codec_info](BMediaFormats.md#media_codec_info "media_codec_info") structure, `mci`, that determines which codec should be used.

Note

![Note](./images/admonitions/Info_32.png)

If you use either the [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") or [media_codec_info](BMediaFormats.md#media_codec_info "media_codec_info") form of the constructor, you must call [`InitCheck()`](BMediaEncoder.md#BMediaEncoder_InitCheck "InitCheck\(\)") to ensure that construction was successful before using any other functions in this class.

#### ~BMediaEncoder()

`virtual ~BMediaEncoder();`

Releases the encoder add-on being used by the `BMediaEncoder`.

* * *

### Member Functions

#### AddTrackInfo()

##### protected

`status_t AddTrackInfo(uint32 code,  
const char* data,  
size_t size);`

In derived classes, you should implement this function to write track information into the output file. `code` indicates the type of information being added, and `data` is the information data itself. `size` indicates the size of the data to be written.

By default, this function returns `B_ERROR`. Your implementation should return `B_OK` if the information was written successfully; otherwise it should return an appropriate error code.

#### Encode()

`status_t Encode(void* inBuffer,  
int64 frameCount,  
media_encode_info* info);`

Encodes a chunk of media data from the input buffer by `inBuffer`, which contains `frameCount` frames of data.

The [media_encode_info](TheMediaKit_DefinedTypes.md#media_encode_info "media_encode_info") structure info is used on input to specify encoding parameters.

After the data is encoded, the encoder will call the derived class' [`WriteChunk()`](BMediaEncoder.md#BMediaEncoder_WriteChunk "WriteChunk\(\)") function to write the data into the file.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_NO_INIT`| The `BMediaEncoder` hasn't been initialized.  
Other errors.| The encoder's Encode() function can return errors.  
  
####  GetEncodeParameters(), SetEncodeParameters()

`status_t GetEncodeParameters(encode_parameters* inParameters) const;``status_t SetEncodeParameters(encode_parameters* outParameters);`

`GetEncodeParameters()` returns in `inParameters` the [encode_parameters](TheMediaKit_DefinedTypes.md#encode_parameters "encode_parameters") describing how the encoder add-on is configured.

`SetEncodeParameters()` let you specify the [encode_parameters](TheMediaKit_DefinedTypes.md#encode_parameters "encode_parameters") for the add-on.

Return Code| Description  
---|---  
`B_OK`| No error.  
  
#### InitCheck()

`status_t InitCheck() const;`

Returns a status_t value indicating whether or not construction was successful. You must call this function after construction before calling any other `BMediaEncoder` functions.

Return Code| Description  
---|---  
`B_OK`|   
Other Errors| See [`SetTo()`](BMediaEncoder.md#BMediaEncoder_SetTo "SetTo\(\)")  
  
#### SetFormat()

`status_t SetFormat(media_format* inputFormat,  
media_format* outputFormat,  
media_file_format* mfi = NULL);`

`SetFormat()` sets the input and output formats to be used by the encoder, as well as the [media_file_format](TheMediaKit_DefinedTypes.md#media_file_format "media_file_format") describing the file to which the encoded data will be written.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_NO_INIT`| The `BMediaEncoder` hasn't been initialized yet.  
  
#### SetTo()

`status_t SetTo(const media_format* outputFormat);` `status_t SetTo(const media_codec_info* mci);`

`SetTo()` sets the format of media data that will be encoded by the `BMediaEncoder` object. This also causes the `BMediaEncoder` to locate an appropriate codec to use.

The first form accepts a [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") structure, `outputFormat`, that indicates the type of media data that the encoder should output.

The second form of `SetTo()` accepts a [media_codec_info](BMediaFormats.md#media_codec_info "media_codec_info") structure, `mci`, that determines which codec should be used.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_NO_INIT`| The `BMediaEncoder` isn't initialized.  
Other Errors.| See [`get_next_encoder()`](BMediaFormats.md#get_next_encoder "get_next_encoder\(\)") and [`get_image_symbol()`](TheKernelKit_Images.md#get_image_symbol "get_image_symbol\(\), get_nth_image_symbol\(\)")  
  
#### WriteChunk()

##### protected

`virtual status_t SetTo(const void* chunkData,  
size_t chunkLength,  
media_encode_info* info)  = 0;`

In derived classes, you should implement this function to write the chunk of encoded data pointed to by `chunkData` into the destination file or buffer. `chunkLength` indicates the size of the buffer, and info provides information about the encoding process that you might need to reference.

Return `B_OK` if the data is successfully written; otherwise return an appropriate error code.

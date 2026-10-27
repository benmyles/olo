<!-- Classes And Methods - The Media Kit - BMediaTrack -->

## BMediaTrack

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `media/MediaTrack.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BMediaTrack_Overview.md "BMediaTrack")

  * [Constructor and Destructor](BMediaTrack.md#BMediaTrack_ConstructorDestructor)
  * [Member Functions](BMediaTrack.md#BMediaTrack_MemberFunctions)
  * [Constants](BMediaTrack.md#BMediaTrack_Constants)

  
  
* * *

### Constructor and Destructor

#### ~BMediaTrack()

`virtual BMediaTrack();`

Destructor. You shouldn't delete a `BMediaTrack` directly; instead, use [`BMediaFile::ReleaseTrack()`](BMediaFile.md#BMediaFile_ReleaseTrack "ReleaseTrack\(\), ReleaseAllTracks\(\)") or [`BMediaFile::ReleaseAllTracks()`](BMediaFile.md#BMediaFile_ReleaseAllTracks), or let [`BMediaFile`](BMediaFile.md "BMediaFile") destroy the track objects when it's deleted.

* * *

### Member Functions

#### AddCopyright()

`status_t AddCopyright(const char* data) const;`

Sets the track's copyright notice to the text specified by data, replacing the existing copyright notice if one exists.

Warning

![Warning](./images/admonitions/Stop_32.png)

The `BMediaTrack` class doesn't automatically perform any locking to prevent multiple writes to the track from occurring at the same time. If you have multiple threads writing into the same `BMediaTrack`, you must use a locking mechanism (such as a semaphore) to keep writes from overlapping.

Return Code| Description  
---|---  
`B_OK`| The copyright was set.  
`EPERM`| The file isn't opened for writing.  
Other errors.| Are codec dependant.  
  
#### AddTrackInfo()

`status_t AddTrackInfo(uint32 code,  
const void* data,  
size_t size,  
uint32 flags = 0) const;`

Adds an informational record of the specified type to the track. The record is specified by the `data` pointer, and is `size` bytes long. flags contains flags that modify the operation (none are defined at this time).

Warning

![Warning](./images/admonitions/Stop_32.png)

The `BMediaTrack` class doesn't automatically perform any locking to prevent multiple writes to the track from occurring at the same time. If you have multiple threads writing into the same `BMediaTrack`, you must use a locking mechanism (such as a semaphore) to keep writes from overlapping.

Return Code| Description  
---|---  
`B_OK`| The format was successfully returned.  
Other errors.| The error returned depends on the codec interpreting the data.  
  
#### CountFrames()

`int64 CountFrames() const;`

Returns the total number of frames in the track.

#### CurrentFrame()

`int64 CurrentFrame() const;`

Returns the current frame in the track (the frame that will be read next by [`ReadFrames()`](BMediaTrack.md#BMediaTrack_ReadFrames "ReadFrames\(\)").

#### CurrentTime()

`bigtime_t CurrentTime() const;`

Returns the current position within the track, expressed in microseconds since the start of the track.

#### DecodedFormat()

`status_t DecodedFormat(media_format* ioFormat) const;`

Negotiates the format that the codec will output when decoding the track's data. Pass in `ioFormat` the format that you want (with wildcards as applicable). The codec will find and return in `ioFormat` its best matching format; this format will then be used when outputting decoded data via [`ReadFrames()`](BMediaTrack.md#BMediaTrack_ReadFrames "ReadFrames\(\)").

The format is typically of a `B_MEDIA_RAW_AUDIO` or `B_MEDIA_RAW_VIDEO` flavor.

Return Code| Description  
---|---  
`B_OK`| The format was successfully negotiated and returned.  
`B_ERROR`| The `BMediaTrack` wasn't properly initialized, or there's no codec for the track's data.  
Other errors.| The error returned depends on the codec interpreting the data.  
  
#### Duration()

`bigtime_t Duration() const;`

Returns the total duration of the track, in microseconds.

#### EncodedFormat()

`status_t EncodedFormat(media_format* outFormat) const;`

Returns the "native" encoded format of the track's data. This is the format of the data returned by [`ReadChunk()`](BMediaTrack.md#BMediaTrack_ReadChunk "ReadChunk\(\)").

Return Code| Description  
---|---  
`B_OK`| The format was successfully returned.  
Other errors.| The error returned depends on the codec interpreting the data.  
  
####  FindKeyFrameForTime(), FindKeyFrameForFrame()

`status_t FindKeyFrameForTime(bigtime_t* inOutTime,  
int32 flags = 0) const;``status_t FindKeyFrameForFrame(int64* inOutFrame,  
int32 flags = 0) const;`

`FindKeyFrameForTime()` accepts in `inOutTime` a time, and returns in `inOutTime` the time at which the closest key frame to that time begins. Likewise, `FindKeyFrameForFrame()` returns the closest key frame number to the specified frame.

The `flags` argument lets you indicate whether to seek forward or backward for the key frame.

If you want to find the nearest key frame before the indicated frame or time, specify [`B_MEDIA_SEEK_CLOSEST_BACKWARD`](BMediaTrack.md#B_MEDIA_SEEK_CLOSEST_BACKWARD) in flags. If you want to find the nearest key frame after the indicated frame or time, specify [`B_MEDIA_SEEK_CLOSEST_FORWARD`](BMediaTrack.md#B_MEDIA_SEEK_CLOSEST_FORWARD).

#### Flush()

`status_t Flush();`

Flushes all buffered encoded data to disk; you should call this function after you've finished writing the last frame to the track. This ensures that everything's flushed at the right offset into the file.

Return Code| Description  
---|---  
`B_OK`| The buffers were flushed.  
Other errors.| Are codec dependant  
  
#### GetCodecInfo()

`status_t GetCodecInfo(media_codec_info* codecInfo) const;`

Returns information about the codec being used to read or write the track's data.

Return Code| Description  
---|---  
`B_OK`| The codec info was returned.  
`B_BAD_TYPE`| There's no valid codec in use by the track.  
  
####  GetEncodeParameters(), SetEncodeParameters()

`status_t GetEncodeParameters(encode_parameters* parameters) const;``status_t SetEncodeParameters(encode_parameters* parameters);`

`GetEncodeParameters()` returns the encode_parameters being used when encoding data on the track.

`SetEncodeParameters()` changes the encode_parameters being used while encoding data.

#### GetParameterView()

`[`BView`](BView.md "BView")* GetParameterView();`

Returns a [`BView`](BView.md "BView") containing controls for adjusting the codec's and track's parameters. Returns `NULL` if there isn't a view available.

#### InitCheck()

`status_t InitCheck() const;`

Returns a status code indicating whether or not the track was successfully and completely instantiated.

Return Code| Description  
---|---  
`B_OK`| The track was constructed properly.  
`B_BAD_VALUE`| An error occurred setting up the track.  
  
#### ReadChunk()

`status_t ReadChunk(char** outBuffer,  
int32* ioSize,  
media_header* header = NULL);`

Returns in `outBuffer` a pointer to the next `ioSize` bytes of the media track; the actual number of bytes returned is returned in `ioSize`; this may be different if the end of the track is reached. The header is set to describe the returned buffer.

The data returned by this function isn't decoded. Typically you'll only use this function if there's no codec available to decode the media data.

Return Code| Description  
---|---  
`B_OK`| The format was successfully negotiated and returned.  
`B_ERROR`| The `BMediaTrack` wasn't properly initialized, or there's no codec for the track's data.  
Other errors.| The error returned depends on the codec interpreting the data.  
  
#### ReadFrames()

`status_t ReadFrames(void* outBuffer,  
int64* outFrameCount,  
media_header* outHeader = NULL);` `status_t ReadFrames(void* outBuffer,  
int64* outFrameCount,  
media_header* outHeader,  
media_decode_info* info);`

Fills the buffer pointed to by `outBuffer` with the next frames or samples from the track, starting at the current position. For video tracks, the next frame of video is decoded and stored into the output buffer. For audio tracks, the buffer is filled with the number of frames negotiated using [`DecodedFormat()`](BMediaTrack.md#BMediaTrack_DecodedFormat "DecodedFormat\(\)"). If the end of the track is reached before the buffer is filled, a partial buffer will be returned.

On return, `outFrameCount` indicates the number of frames returned, and `outHeader`, if you specified a non-`NULL` value, contains the header of the buffer containing the frame or frames. You can obtain useful information (such as the media start time for the buffer) from the header.

The second form of this function lets you provide a [media_decode_info](TheMediaKit_DefinedTypes.md#media_decode_info "media_decode_info") structure to provide additional information to the decoder, such as how much time it's allowed to use to decode the data and format- and codec-specific information.

Return Code| Description  
---|---  
`B_OK`| The frames have been returned.  
`EPERM`| The file wasn't opened for reading.  
Other errors.| The error returned depends on the codec interpreting the data.  
  
#### ReplaceFrames()

`status_t ReplaceFrames(void* inBuffer,  
int64* ioFrameCount,  
media_header* header);`

Replaces the number of frames specified in `ioFrameCount` in the track. `inBuffer` points to the source buffer for the new frames.

####  SeekToFrame(), SeekToTime()

`status_t SeekToFrame(int64* ioFrame,  
int32 flags = 0);``status_t SeekToTime(bigtime_t* ioTime,  
int32 flags = 0);`

Seeks to the specified position in the track. `SeekToFrame()` accepts a destination position as a frame number, and `SeekToTime()` accepts a destination position as time in microseconds. They each return (in `ioFrame` or in `ioTime`) the position to which they actually moved.

For example, if a video codec is only capable of seeking to key frames, the returned `ioFrame` might be different than the one specified on input.

If you want to seek explicitly to the nearest key frame before the current frame, specify [`B_MEDIA_SEEK_CLOSEST_BACKWARD`](BMediaTrack.md#B_MEDIA_SEEK_CLOSEST_BACKWARD) in `flags`. If you want to find the nearest key frame after the current time, specify [`B_MEDIA_SEEK_CLOSEST_FORWARD`](BMediaTrack.md#B_MEDIA_SEEK_CLOSEST_FORWARD).

Return Code| Description  
---|---  
`B_OK`| The format was successfully returned.  
Other errors.| The error returned depends on the codec interpreting the data.  
  
####  SetParameterValue(), GetParameterValue()

`status_t SetParameterValue(int32 id,  
const void* value,  
size_t* size);``status_t GetParameterValue(int32 id,  
const void* value,  
size_t* size);`

`SetParameterValue()` sets the value of the parameter specified by `id` to the data pointed to by `value`; this data is `size` bytes long.

`GetParameterValue()` returns in `value` the value of the specified parameter, and the size of the value in bytes in the `size` argument.

Return Code| Description  
---|---  
`B_OK`| No error.  
`EPERM`| The codec isn't configurable.  
Other errors.| Are codec dependant.  
  
####  SetQuality(), GetQuality()

`status_t SetQuality(float quality);``status_t GetQuality(float* quality);`

These functions set and return the codec's quality setting (where 1.0 means maximum quality).

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_ERROR`| The codec doesn't support a quality setting.  
Other errors.| Are codec dependant.  
  
#### Web()

`[`BParameterWeb`](BParameterWeb.md "BParameterWeb")* Web();`

Returns a [`BParameterWeb`](BParameterWeb.md "BParameterWeb") that can be used for configuring the track and codec. Returns `NULL` if the codec doesn't support user configuration.

#### WriteChunk()

`status_t WriteChunk(void* data,  
size_t size,  
int32 flags = 0);`

Writes the data pointed to by `data`, which contains `size` bytes of data, into the track. Specify `B_MEDIA_KEY_FRAME` for `flags` if the frame is a key frame. It's assumed that the data is already encoded.

Note

![Note](./images/admonitions/Info_32.png)

The `BMediaTrack` class doesn't automatically perform any locking to prevent multiple writes to the track from occurring at the same time. If you have multiple threads writing into the same `BMediaTrack`, you must use a locking mechanism (such as a semaphore) to keep writes from overlapping.

In general, you should only use `WriteChunk()` if you're reading compressed data from one file and copying it into another, without trying to interpret the data.

Return Code| Description  
---|---  
`B_OK`| The data has been written.  
`EPERM`| The file wasn't opened for writing.  
Other errors.| The error returned depends on the codec interpreting the data.  
  
#### WriteFrames()

`status_t WriteFrames(void* data,  
int32 numFrames,  
int32 flags = 0);`

Writes the data pointed to by `data`, which contains `numFrame` frames, into the track. Specify `B_MEDIA_KEY_FRAME` for `flags` if the frame is a key frame.

Note

![Note](./images/admonitions/Info_32.png)

The `BMediaTrack` class doesn't automatically perform any locking to prevent multiple writes to the track from occurring at the same time. If you have multiple threads writing into the same `BMediaTrack`, you must use a locking mechanism (such as a semaphore) to keep writes from overlapping.

You always have to select an encoder before writing frames into a media track, even if the data is raw audio or video. When writing raw audio or video data into a `BMediaTrack`, you need to use the raw encoder. Even though this doesn't transform the data, it sets up internal data that's necessary for the file to be played back properly after it's created.

Return Code| Description  
---|---  
`B_OK`| The frames have been written.  
`EPERM`| The file wasn't opened for writing.  
Other errors.| The error returned depends on the codec interpreting the data.  
  
* * *

### Constants

####  media_seek_type

Declared in: `media/MediaTrack.h`

Constant| Description  
---|---  
`B_MEDIA_SEEK_CLOSEST_FORWARD`| Seek to the nearest key frame after the current position in the track.  
`B_MEDIA_SEEK_CLOSEST_BACKWARD`| Seek to the nearest key frame before the current position in the track.  
`B_MEDIA_SEEK_DIRECTION_MASK`| Mask the seek flags with this value to obtain the seek direction.  
  
These flags, used when calling [`SeekToTime()`](BMediaTrack.md#BMediaTrack_SeekToTime) and [`SeekToFrame()`](BMediaTrack.md#BMediaTrack_SeekToFrame "SeekToFrame\(\), SeekToTime\(\)"), let you look for the nearest key frame after or before the current position in the file, without having to guess the time or frame number at which it's located.

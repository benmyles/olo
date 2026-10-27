<!-- Classes And Methods - The Media Kit - BMediaFile -->

## BMediaFile

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `media/MediaFile.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BMediaFile_Overview.md "BMediaFile")

  * [Constructor and Destructor](BMediaFile.md#BMediaFile_ConstructorDestructor)
  * [Member Functions](BMediaFile.md#BMediaFile_MemberFunctions)
  * [Constants](BMediaFile.md#BMediaFile_Constants)

  
  
* * *

### Constructor and Destructor

#### BMediaFile()

`BMediaFile(const entry_ref* ref);` `BMediaFile(BDataIO* source);` `BMediaFile(const entry_ref* ref,  
int32 flags = 0);` `BMediaFile(BDataIO* source,  
int32 flags = 0);` `BMediaFile(const entry_ref* ref,  
const media_file_format* fileFormat,  
int32 flags = 0);` `BMediaFile(BDataIO* source,  
const media_file_format* fileFormat,  
int32 flags = 0);`

The first four forms of the constructor initializes the `BMediaFile` to read the file specified by `ref` or by `source`. Once this has been done, be sure to call [`InitCheck()`](BMediaFile.md#BMediaFile_InitCheck "InitCheck\(\)") to ensure that the file was successfully sniffed. If sniffing was successful, the `BMediaFile` object can then be used to instantiate the [`BMediaTrack`](BMediaTrack.md "BMediaTrack") objects necessary to read the file's data.

The second two forms prepare a `BMediaFile` object for writing to a media file, which is specified by either `ref` or `destination`. The format of the file to be written is specified by `fileFormat`.

The flags can be a combination of the following values:

Constant| Description  
---|---  
`B_MEDIA_FILE_REPLACE_MODE`| Replace any existing media file, if writing.  
`B_MEDIA_FILE_NO_READ_AHEAD`| Don't read ahead in the file.  
`B_MEDIA_FILE_UNBUFFERED`| Use smaller reads. This can be handy if you're trying to stream low-bitrate files without waiting for the extractor to fill a large outgoing buffer.  
`B_MEDIA_FILE_BIG_BUFFERS`| Use large buffers.  
  
#### ~BMediaFile()

`virtual ~BMediaFile()();`

Destructor.

* * *

### Member Functions

#### AddChunk()

`status_t AddChunk(int32 type,  
const void* data,  
size_t size);`

Writes a user-defined chunk of data to the file (if doing so is supported by the encoder). The `type` argument indicates the chunk type, `data` points to the data to be written, and `size` indicates the size of the data to write to the file.

Warning

![Warning](./images/admonitions/Stop_32.png)

The `BMediaFile` class doesn't automatically perform any locking to prevent multiple writes to the file from occurring at the same time. If you have multiple threads writing into the same `BMediaFile`, you must use a locking mechanism (such as a [semaphore](TheKernelKit_Semaphores_Overview.md "Semaphores")) to keep writes from overlapping.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_BAD_TYPE`| The `BMediaFile` doesn't reference a valid file.  
Other errors| Are codec specific.  
  
####  AddCopyright(), Copyright()

`status_t AddCopyright(const char* data);``const char* Copyright() const;`

`AddCopyright()` sets the file's copyright notice to the string specified by `data`. Any existing notice is replaced. The string is copied, so the original belongs to you.

`Copyright()` returns the media file's copyright notice, or `NULL` if there isn't one. This string belongs to the `BMediaFile`, so don't delete or change it.

Warning

![Warning](./images/admonitions/Stop_32.png)

The `BMediaFile` class doesn't automatically perform any locking to prevent multiple writes to the file from occurring at the same time. If you have multiple threads writing into the same `BMediaFile`, you must use a locking mechanism (such as a [semaphore](TheKernelKit_Semaphores_Overview.md "Semaphores")) to keep writes from overlapping.

Return Code| Description  
---|---  
`B_OK`| No error. (`AddCopyright()` only)  
`B_BAD_TYPE`| The `BMediaFile` doesn't reference a valid file. (`AddCopyright()` only)  
Other errors| Are codec specific. (`AddCopyright()` only)  
  
#### CloseFile()

`status_t CloseFile();`

Once all media data has been written to all tracks, you need to call `CloseFile()` so the `BMediaFile` object can tidy up the file and finish the writing tasks. If you forget to call `CloseFile()`, the resulting file may not be complete.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_BAD_TYPE`| The `BMediaFile` doesn't reference a valid file.  
Other errors| Are codec specific.  
  
#### CommitHeader()

`status_t CommitHeader();`

After you've finished adding new tracks to a file, you need to call `CommitHeader()` to let the `BMediaFile` set up the file's header. This helps optimize the file writing process.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_BAD_TYPE`| The `BMediaFile` doesn't reference a valid file.  
Other errors| Are codec specific.  
  
#### CountTracks()

`int32 CountTracks() const;`

Returns the number of tracks in the media file. The return value is undefined if the initialization failed.

#### CreateTrack()

`BMediaTrack* CreateTrack(media_format* mediaFormat,  
const media_codec_info* codecInfo);` `BMediaTrack* CreateTrack(media_format* mediaFormat);`

Creates a new track in the file and returns a [`BMediaTrack`](BMediaTrack.md "BMediaTrack") object referencing it. The track is configured to use the media format specified by `mediaFormat`, and will be written using the codec described by `codecInfo`. You can only use [`BMediaTrack::WriteFrames()`](BMediaTrack.md#BMediaTrack_WriteFrames "WriteFrames\(\)") to write into the track.

If you use the second form of the constructor, without a `codecInfo` argument, the track will be written containing raw media data. You can only use [`BMediaTrack::WriteChunk()`](BMediaTrack.md#BMediaTrack_WriteChunk "WriteChunk\(\)") to write into the track.

The `mediaFormat` indicates the format of the buffers you'll be passing along to the write function, and `codecInfo` specifies the codec you want to use to encode the data into the track.

Note

![Note](./images/admonitions/Info_32.png)

The `mediaFormat` can't contain any wildcards; you have to specify the exact format that you're going to be providing. `mediaFormat` must also be the same structure you passed to [`get_next_encoder()`](BMediaFormats.md#get_next_encoder "get_next_encoder\(\)").

When writing new media files, you should create all your tracks at once before writing any media data. Once all the tracks have been created, be sure to call [`CommitHeader()`](BMediaFile.md#BMediaFile_CommitHeader "CommitHeader\(\)") to write the header to disk. This is necessary since the header size may vary depending on the number of tracks you put in the file, and once you start writing media data into the tracks, it would be difficult (and inefficient) to resize the header.

If an error occurs while creating the new track, `NULL` is returned.

#### GetFileFormatInfo()

`status_t GetFileFormatInfo(media_file_format* info) const;`

Fills the specified media_file_format structure with information describing the file format of the file referenced by the `BMediaFile` object.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_NO_INIT`| The `BMediaFile` doesn't reference a valid file.  
  
#### GetParameterView()

`BView* GetParameterView() const;`

Returns a [`BView`](BView.md "BView") containing controls for adjusting the file format's parameters. Returns `NULL` if there isn't a view available.

#### InitCheck()

`status_t InitCheck() const;`

Returns the result code from the constructor. You should always call this after instantiating a `BMediaFile` object, but before using it.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_MEDIA_NO_HANDLER`| No codec available for the required or requested format.  
`B_NO_MEMORY`| Not enough memory to set up the codec (writing).  
`B_BAD_INDEX`| The required add-on wasn't found (writing).  
`B_MISSING_SYMBOL`| The add-on doesn't support writing.  
  
####  ReleaseTrack(), ReleaseAllTracks()

`status_t ReleaseTrack(BMediaTrack* track);``status_t ReleaseAllTracks();`

`ReleaseTrack()` releases the resources used by the specified `track`. Doing so reduces your application's memory usage. If you want to release every track you're using, you can call `ReleaseAllTracks()`.

Once released, a track can't be used any longer until you use [`TrackAt()`](BMediaFile.md#BMediaFile_TrackAt "TrackAt\(\)") again.

If the `BMediaFile` wasn't properly initialized, or the index is invalid, `NULL` is returned.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_ERROR`| Couldn't release the track (it may be invalid, or not actually in use).  
  
####  SetParameterValue(), GetParameterValue()

`status_t SetParameterValue(int32 id,  
const void* value,  
size_t size);``status_t GetParameterValue(int32 id,  
const void* value,  
size_t size);`

`SetParameterValue()` sets the value of the parameter specified by `id` to the data pointed to by `value`; this data is `size` bytes long.

`GetParameterValue()` returns in `value` the value of the specified parameter, and the size of the value in bytes in the `size` argument.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_NO_INIT`| The `BMediaFile` doesn't reference a valid file.  
  
#### TrackAt()

`BMediaTrack* TrackAt(int32 index);`

Returns a [`BMediaTrack`](BMediaTrack.md "BMediaTrack") object referencing the track at the specified `index` into the file. The index must be a value between 0 and [`CountTracks()`](BMediaFile.md#BMediaFile_CountTracks "CountTracks\(\)") \- 1.

If the `BMediaFile` wasn't properly initialized, or the `index` is invalid, `NULL` is returned.

Note

![Note](./images/admonitions/Info_32.png)

You must call [`ReleaseTrack()`](BMediaFile.md#BMediaFile_ReleaseTrack "ReleaseTrack\(\), ReleaseAllTracks\(\)") when you're finished with the track.

#### Web()

`BParameterWeb* Web();`

Returns a [`BParameterWeb`](BParameterWeb.md "BParameterWeb") that can be used for configuring the file format's parameters. Returns `NULL` if the codec doesn't support user configuration.

* * *

### Constants

#### Constructor Flags

Declared in: `media/MediaFile.h`

Constant| Description  
---|---  
`B_MEDIA_FILE_REPLACE_MODE`| Replace any existing media file, if writing.  
`B_MEDIA_FILE_NO_READ_AHEAD`| Don't read ahead in the file.  
`B_MEDIA_FILE_UNBUFFERED`| Use smaller reads. This can be handy if you're trying to stream low-bitrate files without waiting for the extractor to fill a large outgoing buffer.  
`B_MEDIA_FILE_BIG_BUFFERS`| Use large buffers.  
  
These flags control the behavior of the constructor.

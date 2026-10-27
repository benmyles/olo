<!-- Classes And Methods - The Media Kit - BFileInterface -->

## BFileInterface

| Derived From:| [`BMediaNode`](BMediaNode.md "BMediaNode")  
---|---  
Mix-in Classes:| –  
Declared In:| `media/FileInterface.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BFileInterface_Overview.md "BFileInterface")

  * [Constructor and Destructor](BFileInterface.md#BFileInterface_ConstructorDestructor)
  * [Member Functions](BFileInterface.md#BFileInterface_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BFileInterface()

##### protected

`BFileInterface();`

Constructor.

* * *

### Member Functions

#### DisposeFileFormatCookie()

##### protected

`virtual void DisposeFileFormatCookie(int32 cookie)  = 0;`

This function must be implemented to dispose of a `cookie` used for iterating over file formats your node supports. If the cookie is a reference to a data block, you should free it; otherwise, you can just return without doing anything.

#### HandleMessage()

##### protected

`virtual status_t HandleMessage(int32 message,  
const void* data,  
size_t size);`

Implement this function to handle messages that arrive on your control port; see [`BMediaNode::HandleMessage()`](BMediaNode.md#BMediaNode_HandleMessage "HandleMessage\(\)") for details.

#### GetDuration()

##### protected

`virtual status_t GetDuration(bigtime_t* outDuration)  = 0;`

Implement this function to return in `outDuration` the duration, in microseconds of the media data in the file currently being worked on.

If there is no file in use, or the file isn't valid, return a negative error code; otherwise, return `B_OK`.

#### GetNextFileFormat()

##### protected

`virtual status_t GetNextFileFormat(int32* cookie,  
media_file_format* outFormat)  = 0;`

The first time this function is called, `cookie` will be 0. Return information about the first file format you support in `outFormat` and set cookie to some meaningful (non-zero) value that you can use to track where you are in the list of formats you support, then return `B_OK`.

On successive calls to `GetNextFileFormat()`, you should return successive file format information and change `cookie` as necessary to remember where you are in the list. Return `B_OK` each time you successfully return information about a file format.

Once you run out of formats, return `B_ERROR`.

####  GetRef(), SetRef()

##### protected

`virtual status_t GetRef(const entry_ref* outRef,  
BMimeType* outMimeType)  = 0;``virtual status_t SetRef(const entry_ref& file,  
bool create,  
bigtime_t* outDuration)  = 0;`

Implement `GetRef()` function to return, in `outRef`, the entry_ref of the file with which the node is currently working. You should also return the MIME type of the file in `outMimeType`.

When an application or other client wants your node to use a specific file, the `SetRef()` function will be called. The file to be used is specified by `file`, which may or may not exist. If `create` is `true`, you should create a new file (erasing an existing file, if one's already there), initialize the file for writing, then store 0 in `outDuration`.

If `create` is `false`, you should open the existing file and put the actual running time of the file into `outDuration`.

Return `B_OK` if successful; otherwise, return a negative error code, as appropriate. If you forget to implement these functions, they'll always return `B_ERROR`.

#### SniffRef()

##### protected

`virtual status_t SniffRef(const entry_ref* file,  
char* outMimeType,  
float* outQuality)  = 0;`

When the system (or an application program) finds an unknown file, it can call [`BMediaRoster`](BMediaRoster.md "BMediaRoster") to try to identify the file. Your node's `SniffRef()` function will be called so your node can look at the file.

Inspect the file referenced by file. If you can handle the format, set `outMimeType` to the MIME type of the file format (the buffer is 256 bytes long), and set `outQuality` to indicate how well you can process the file (where 0.0 means you can't handle it at all and 1.0 means the file format is a privately-owned format that your application handles perfectly).

If you know how to identify the file, but not how to play or record it, return 0.0 for `outQuality`.

If you know how to decode only some parts of the file, but not all of them, set `outQuality` to 0.4 or to 0.6 if you can handle both audio and video contained in the file, but not other forms of media data that are present.

Set `outQuality` to 0.9 if you can handle the format very well, but it's a publicly-defined format whose specifications you don't control. You should only return an `outQuality` value of 1.0 if the file format specifications are in your control and you implement them perfectly.

Interpolate between these values to give the best estimate you can of your ability to handle the file, so selection of the most suitable node can be made. For instance, if your node does low-quality but high-speed processing of an audio format, you might subtract a little bit from the `outQuality` value you might otherwise return.

Return `B_OK` if you successfully sniff the file and have something to say about its contents; otherwise return an appropriate error code. If you don't know the file format at all, you should return `B_MEDIA_NO_HANDLER`.

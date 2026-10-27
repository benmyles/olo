<!-- Classes And Methods - The Media Kit - BBuffer -->

## BBuffer

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `media/Buffer.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BBuffer_Overview.md "BBuffer")

  * [Constructor and Destructor](BBuffer.md#BBuffer_ConstructorDestructor)
  * [Member Functions](BBuffer.md#BBuffer_MemberFunctions)
  * [Constants](BBuffer.md#BBuffer_Constants)
  * [DefinedTypes](BBuffer.md#BBuffer_DefinedTypes)

  
  
* * *

### Constructor and Destructor

The `BBuffer` constructor and destructor are private; use the appropriate functions in the [`BBufferGroup`](BBufferGroup.md "BBufferGroup") class to create and destroy `BBuffers`.

* * *

### Member Functions

#### AudioHeader()

`[media_audio_header](TheMediaKit_DefinedTypes.md#media_audio_header "media_audio_header")* AudioHeader();`

Returns a pointer to an audio buffer's header; this is just an alias for:
    
    
    &BBuffer::Header()->u.raw_audio;

#### CloneInfo()

`buffer_clone_info CloneInfo() const;`

Returns a [buffer_clone_info](BBuffer.md#buffer_clone_info "buffer_clone_info") structure describing a buffer. This information is primarily for debugging purposes; don't use this [buffer_clone_info](BBuffer.md#buffer_clone_info "buffer_clone_info") structure to add a buffer to a buffer group, because it'll just alias an existing buffer.

#### Data()

`void Data();`

The `Data()` function returns a pointer to the first byte of the buffer, or `NULL` if the buffer somehow couldn't be properly initialized (in which case the buffer can't be used).

#### Flags()

`int32 Flags();`

Returns the buffer's flags. The flags let you specify options for the buffer. For example, you might create a [`BBufferGroup`](BBufferGroup.md "BBufferGroup") in which some buffers are intended for odd video fields (`B_F1_BUFFER`), and other buffers are intended for even video fields (`B_F2_BUFFER`), thereby letting you handle interlaced video much more easily by having each buffer know where the video should be displayed.

#### Header()

`media_header* Header();`

Returns a pointer to the buffer's header. This header describes the media data contained therein. The result is only valid for a buffer received from either [`BBufferGroup::RequestBuffer()`](BBufferGroup.md#BBufferGroup_RequestBuffer "RequestBuffer\(\)") or [`BBufferConsumer::BufferReceived()`](BBufferConsumer.md#BBufferConsumer_BufferReceived "BufferReceived\(\)").

If you put data into a buffer, you should call `Header()` to obtain a pointer to the buffer's header, then fill out the [media_header](TheMediaKit_DefinedTypes.md#media_header "media_header") structure with information describing the buffer's contents.

#### ID()

`media_buffer_id ID();`

If the buffer has been successfully registered with the Media Server, and is available for use by other clients, `ID()` returns a positive buffer ID. Otherwise, a negative number is returned.

#### Recycle()

`void Recycle();`

Sends the buffer back to the BBufferGroup that owns it so the buffer can be reused. You can only call Recycle() on a buffer that you received from either the [`BBufferGroup::RequestBuffer()`](BBufferGroup.md#BBufferGroup_RequestBuffer "RequestBuffer\(\)") call or the [`BBufferConsumer::BufferReceived()`](BBufferConsumer.md#BBufferConsumer_BufferReceived "BufferReceived\(\)") call.

Warning

![Warning](./images/admonitions/Stop_32.png)

Don't call both `BroadcastBuffer()` and `Recycle()` on the same buffer.

#### SizeAvailable()

`size_t SizeAvailable();`

Returns the size, in bytes, of the buffer. The actual number of bytes used might be less than this value, and is stored in the buffer's header, which can be obtained by calling the [`Header()`](BBuffer.md#BBuffer_Header "Header\(\)") function.

####  SizeUsed(), SetSizeUsed()

`size_t SizeUsed();``void SetSizeUsed(ssize_t sizeUsed);`

`SizeUsed()` returns the number of bytes in the buffer that are currently in use.

`SetSizeUsed()` sets this value. This should be called after writing data into the buffer in order to indicate the size of the written data.

#### Type()

`media_type Type();`

Returns the media type of the data contained within the buffer, as specified by the [`BBufferProducer`](BBufferProducer.md "BBufferProducer") from which the buffer originated. This value is only valid if you received the buffer from either the [`BBufferGroup::RequestBuffer()`](BBufferGroup.md#BBufferGroup_RequestBuffer "RequestBuffer\(\)") call or the [`BBufferConsumer::BufferReceived()`](BBufferConsumer.md#BBufferConsumer_BufferReceived "BufferReceived\(\)") call.

#### VideoHeader()

`media_video_header* VideoHeader();`

Returns a pointer to a video buffer's header; this is just an alias for:
    
    
    &BBuffer::Header()->u.raw_video;

* * *

### Constants

#### Flags

Declared in: `media/Buffer.h`

Constant| Description  
---|---  
`B_F1_BUFFER`.| Buffer is for odd fields.  
`B_F2_BUFFER`.| Buffer is for even fields.  
`B_SMALL_BUFFER`.| The buffer is a small buffer.  
  
These flags can be assigned to a buffer; the current possible values let you specify whether the buffer should be used for even video fields or odd video fields; if `B_SMALL_BUFFER` is set, the buffer is a [`BSmallBuffer`](BSmallBuffer.md "BSmallBuffer").

* * *

### DefinedTypes

#### buffer_clone_info

Declared in: `media/Buffer.h`
    
    
    struct buffer_clone_info {
       buffer_clone_info();
       ~buffer_clone_info();
       media_buffer_id buffer;
       area_id area;
       size_t offset;
       size_t size;
       int32 flags;
    private:
       _reserved_[4];
    };

Describes where in memory a `BBuffer` resides (in terms of the memory area and offset into the area at which the buffer is located, and the size of the buffer), as well as the buffer's flags.

<!-- Classes And Methods - The Translation Kit - BBitmapStream -->

## BBitmapStream

| Derived From:| [`BPositionIO`](BPositionIO.md "BPositionIO")  
---|---  
Mix-in Classes:| –  
Declared In:| `translation/BitmapStream.h`  
Library:| `libtranslation.so`  
Allocation:| –  
[Class Overview](BBitmapStream_Overview.md "BBitmapStream")

  * [Constructor and Destructor](BBitmapStream.md#BBitmapStream_ConstructorDestructor)
  * [Member Functions](BBitmapStream.md#BBitmapStream_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BBitmapStream()

`BBitmapStream([`BBitmap`](BBitmap.md "BBitmap")* map = NULL);`

Creates a new instance of `BBitmapStream`. If `map` is `NULL`, the stream is initialized to be empty. Otherwise, the [`BBitmap`](BBitmap.md "BBitmap") is converted to a translator bitmap and placed in the stream. The application shares the [`BBitmap`](BBitmap.md "BBitmap") with the `BBitmapStream` object. It therefore shouldn't delete the [`BBitmap`](BBitmap.md "BBitmap") without first calling [`DetachBitmap()`](BBitmapStream.md#BBitmapStream_DetachBitmap "DetachBitmap\(\)").

#### ~BBitmapStream()

`~BBitmapStream();`

Frees all memory allocated by the [`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster").

* * *

### Member Functions

#### DetachBitmap()

`status_t DetachBitmap([`BBitmap`](BBitmap.md "BBitmap")** outMap);`

Returns, in `outMap`, a [`BBitmap`](BBitmap.md "BBitmap") representing the image contained in the `BBitmapStream`. Once `DetachBitmap()` has been called, no further operations should be performed on the `BBitmapStream`.

Return Code| Description  
---|---  
`B_NO_ERROR`.| Success.  
`B_BAD_VALUE`.|  `outMap` is `NULL`.  
`B_ERROR`.|  There is no [`BBitmap`](BBitmap.md "BBitmap") available.  
  
####  Position(), ReadAt(), Seek(), SetSize(), WriteAt()

`off_t Position() const;``ssize_t ReadAt(off_t position,  
void* buffer,  
size_t size);``off_t Seek(off_t position,  
int32 whence);``status_t SetSize(off_t size) const;``ssize_t WriteAt(off_t pos,  
const void* data,  
size_t size);`

These methods provide the implementation for the [`BPositionIO`](BPositionIO.md "BPositionIO"). The class functions identically to [`BPositionIO`](BPositionIO.md "BPositionIO") with the exception of `ReadAt()` and `WriteAt()`, which read and write only translator bitmaps as described in the class introduction.

#### Size()

`off_t Size() const;`

Returns the number of bytes in the translator bitmap in the stream.

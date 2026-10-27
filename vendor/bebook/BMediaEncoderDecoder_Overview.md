<!-- System Overview - The Media Kit - BMediaEncoder &amp; BMediaDecoder -->

## BMediaEncoder & BMediaDecoder

The [`BMediaEncoder`](BMediaEncoder.md "BMediaEncoder") class is a base class from which encoder classes may be derived. It handles all the work of locating an appropriate encoder add-on and interfacing with it. All you have to do is implement the and [`AddTrackInfo()`](BMediaEncoder.md#BMediaEncoder_AddTrackInfo "AddTrackInfo\(\)") functions to write a chunk of encoded data to the destination and to write information about the track into the file.

The [`BMediaBufferEncoder`](BMediaBufferEncoder.md "BMediaBufferEncoder") class is provided for you; it's a derived class that implements [`WriteChunk()`](BMediaEncoder.md#BMediaEncoder_WriteChunk "WriteChunk\(\)") to write encoded data into a memory buffer.

An application that wants to encode a stream of data that's not originating from a file should use a subclass of [`BMediaEncoder`](BMediaEncoder.md "BMediaEncoder") (such as [`BMediaBufferEncoder`](BMediaBufferEncoder.md "BMediaBufferEncoder")) to do so.

The [`BMediaDecoder`](BMediaDecoder.md "BMediaDecoder") class is a base class from which decoder classes may be derived. It handles all the work of locating an appropriate decoder add-on and interfacing with it. All you have to do is implement the [`GetNextChunk()`](BMediaDecoder.md#BMediaDecoder_GetNextChunk "GetNextChunk\(\)") function to fetch the next chunk of encoded data to be processed.

The [`BMediaBufferDecoder`](BMediaBufferDecoder.md "BMediaBufferDecoder") class is provided for you; it's a derived class that implements [`GetNextChunk()`](BMediaDecoder.md#BMediaDecoder_GetNextChunk "GetNextChunk\(\)") to return data from a memory buffer.

An application that wants to decode a stream of data that's not originating from a file should use a subclass of [`BMediaDecoder`](BMediaDecoder.md "BMediaDecoder") (such as [`BMediaBufferDecoder`](BMediaBufferDecoder.md "BMediaBufferDecoder")) to do so.

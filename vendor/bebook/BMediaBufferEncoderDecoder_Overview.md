<!-- System Overview - The Media Kit - BMediaBufferEncoder &amp; BMediaBufferDecoder -->

## BMediaBufferEncoder & BMediaBufferDecoder

The [`BMediaBufferEncoder`](BMediaBufferEncoder.md "BMediaBufferEncoder") class, derived from [`BMediaEncoder`](BMediaEncoder.md "BMediaEncoder"), can be used to encode media data from a memory buffer (instead of from a file).

It implements the [`BMediaBufferEncoder::WriteChunk()`](BMediaBufferEncoder.md#BMediaBufferEncoder_WriteChunk "WriteChunk\(\)") function to accomplish this task. `BMediaBufferEncoder::AddTrackInfo()` is not implemented.

The [`BMediaBufferDecoder`](BMediaBufferDecoder.md "BMediaBufferDecoder") class can be used to decode media data from a memory buffer. It implements the [`BMediaDecoder::GetNextChunk()`](BMediaDecoder.md#BMediaDecoder_GetNextChunk "GetNextChunk\(\)") function to fetch data from the memory buffer, and adds a new [`DecodeBuffer()`](BMediaBufferDecoder.md#BMediaBufferDecoder_DecodeBuffer "DecodeBuffer\(\)") function that decodes data from one buffer into another.

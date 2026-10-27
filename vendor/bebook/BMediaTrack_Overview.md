<!-- System Overview - The Media Kit - BMediaTrack -->

## BMediaTrack

The [`BMediaTrack`](BMediaTrack.md "BMediaTrack") class provides access to a particular track in a media file. It's always instantiated using the [`BMediaFile::TrackAt()`](BMediaFile.md#BMediaFile_TrackAt "TrackAt\(\)") or [`BMediaFile::CreateTrack()`](BMediaFile.md#BMediaFile_CreateTrack "CreateTrack\(\)") function.

The [`BMediaTrack`](BMediaTrack.md "BMediaTrack") constructor searches for a codec that can handle the encoded data in the track; once that's been done, the track is ready to be used.

If you opened the file for writing, you can write data into the track. If you specified the `B_MEDIA_FILE_REPLACE_MODE` flag when constructing the [`BMediaFile`](BMediaFile.md "BMediaFile"), you can both read and write from the file. If no decoder is available for the track, you can still use [`ReadChunk()`](BMediaTrack.md#BMediaTrack_ReadChunk "ReadChunk\(\)") to access the encoded data directly.

After instantiating the [`BMediaTrack`](BMediaTrack.md "BMediaTrack"), using the [`BMediaFile`](BMediaFile.md "BMediaFile") functions for doing so, you should call [`InitCheck()`](BMediaTrack.md#BMediaTrack_InitCheck "InitCheck\(\)") to be sure the track is valid. You can then use [`ReadFrames()`](BMediaTrack.md#BMediaTrack_ReadFrames "ReadFrames\(\)") and [`WriteFrames()`](BMediaTrack.md#BMediaTrack_WriteFrames "WriteFrames\(\)") to read and write data to the file, as appropriate. For video data, you should work one frame at a time.

You can also seek particular times or frames using [`SeekToTime()`](BMediaTrack.md#BMediaTrack_SeekToTime) or [`SeekToFrame()`](BMediaTrack.md#BMediaTrack_SeekToFrame "SeekToFrame\(\), SeekToTime\(\)").

For an example of how to use [`BMediaTrack`](BMediaTrack.md "BMediaTrack") to read and write tracks in media files, see "[Reading and Writing Media Files](TheMediaKit_Overview_ReadingWriting.md "Reading and Writing Media Files")"

Note

![Note](./images/admonitions/Info_32.png)

As a general rule, you can't use wildcards in any structures used by [`BMediaTrack`](BMediaTrack.md "BMediaTrack") functions. You tell [`BMediaTrack`](BMediaTrack.md "BMediaTrack") what format you have, and [`BMediaTrack`](BMediaTrack.md "BMediaTrack") will simply tell you whether or not that format is supported.

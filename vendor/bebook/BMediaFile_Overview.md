<!-- System Overview - The Media Kit - BMediaFile -->

## BMediaFile

The [`BMediaFile`](BMediaFile.md "BMediaFile") class represents a file containing media data. When instantiated a [`BMediaFile`](BMediaFile.md "BMediaFile") with an [entry_ref](BEntry.md#entry_ref "entry_ref") to an existing media file, it sniffs the file and figures out the right codecs to use when accessing that file.

To read an existing media file, you can then call [`TrackAt()`](BMediaFile.md#BMediaFile_TrackAt "TrackAt\(\)") to instantiate [`BMediaTrack`](BMediaTrack.md "BMediaTrack") objects fot the file's tracks; these can in turn be used to decode media data from the file.

You can also write data to the file. In this case, you construct the object by specifying an [entry_ref](BEntry.md#entry_ref "entry_ref") and a [media_file_format](TheMediaKit_DefinedTypes.md#media_file_format "media_file_format") specification describing the format of the media data you plan to write into the file. You then call [`CreateTrack()`](BMediaFile.md#BMediaFile_CreateTrack "CreateTrack\(\)") to create each track you want to write into the file. Once each track has been created (but is still empty), you call [`CommitHeader()`](BMediaFile.md#BMediaFile_CommitHeader "CommitHeader\(\)") to write the file's header to disk, and you can use [`BMediaTrack`](BMediaTrack.md "BMediaTrack") functions to write the actual media data into the tracks.

Call [`CloseFile()`](BMediaFile.md#BMediaFile_CloseFile "CloseFile\(\)") when you're finished writing to it (you don't need to call this if you're reading the file).

For an example of how to use [`BMediaFile`](BMediaFile.md "BMediaFile") to read and write media files, see "[Reading and Writing Media Files](TheMediaKit_Overview_ReadingWriting.md "Reading and Writing Media Files")"

Note

![Note](./images/admonitions/Info_32.png)

As a general rule, you can't use wildcards in any structures used by [`BMediaFile`](BMediaFile.md "BMediaFile") functions. You tell [`BMediaFile`](BMediaFile.md "BMediaFile") what format you have, and [`BMediaFile`](BMediaFile.md "BMediaFile") will simply tell you whether or not that format is supported.

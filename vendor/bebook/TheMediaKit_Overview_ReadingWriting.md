<!-- System Overview - The Media Kit - Reading and Writing Media Files -->

## Reading and Writing Media Files

Working with media files becomes relatively painless when you use the [`BMediaFile`](BMediaFile.md "BMediaFile") and [`BMediaTrack`](BMediaTrack.md "BMediaTrack") classes. This section looks at a sample program that converts a media file from one format to another, using these classes.

* * *

### Preparing a media_format
    
    
    void BuildMediaFormat(int32 width, int32 height,
                color_space cspace, media_format *format) {
       media_raw_video_format *rvf = &format->u.raw_video;
    
       memset(format, 0, sizeof(*format));
    
       format->type = B_MEDIA_RAW_VIDEO;
       rvf->last_active = (uint32)(height - 1);
       rvf->orientation = B_VIDEO_TOP_LEFT_RIGHT;
       rvf->pixel_width_aspect = 1;
       rvf->pixel_height_aspect = 3;
       rvf->display.format = cspace;
       rvf->display.line_width = (int32)width;
       rvf->display.line_count = (int32)height;
       if (cspace == B_RGB32)
          rvf->display.bytes_per_row = 4 * width;
       else {
          printf("can't build the format!n");
          exit(5);
       }
    }

`BuildMediaFormat()` accepts as input parameters describing a video format—the width, height, and color space of the video—and returns a [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") structure describing that format. For our purposes, we require a `B_RGB32` colorspace, and the frames will be in raw video format.

* * *

### Converting the Files

The `transcode()` function below handles actually converting a media file into another format, writing the newly-converted media into a new file. We'll look at it in chunks to lighten the load.

`transcode()` accepts as input a [`BMediaTrack`](BMediaTrack.md "BMediaTrack") referring to the video track from the original file, and another [`BMediaTrack`](BMediaTrack.md "BMediaTrack") referring to the audio track. `output` is the name of the new file to be created.

`family_name` specifies the file format family to be used when creating the new file, and `video_name` and `audio_name` indicate by name which encoders should be used for the video and audio tracks.
    
    
    void transcode(BMediaTrack *vidtrack, BMediaTrack *audtrack,
             char *output, char *family_name, char *video_name,
             char *audio_name)
    {
       char *chunk;
       char *bitmap = NULL, *sound_buffer = NULL;
       bool found_video_encoder = false, found_audio_encoder = false;
       bool found_family;
       int32 i, sz, cookie;
       int64 numFrames, j;
       int64 framesize;
       status_t err;
       entry_ref ref;
       BMediaFile out;
       BMediaTrack vid = NULL, *aud = NULL;
       media_format format, outfmt;
       media_codec_info mci;
       media_file_format mfi;
       media_header mh;
    
       err = get_ref_for_path(output, &ref);
       if (err) {
          printf("problem with get_ref_for_path() -- %s\n",
                 strerror(err));
          return;
       }

The function begins by creating an [entry_ref](BEntry.md#entry_ref "entry_ref") for the output file. If an error occurs, the function terminates after displaying an error message.
    
    
       cookie = 0;
       while((err = get_next_file_format(&cookie, &mfi)) == B_OK) {
          if (strcmp(mfi.short_name, family_name) == 0)
             break;
       }
    
       if (err != B_OK) {
          printf("failed to find a file format handler !\n");
          return;
       }

Next, a loop is used, calling [`get_next_file_format()`](TheMediaKit_Functions.md#get_next_file_format "get_next_file_format\(\)") to find the appropriate handler for the requested file format family. If the specified `family_name` doesn't match the `short_name` field of any of the media file formats available, an error is printed and the function aborts. Otherwise, `mfi` contains a description of the file format.
    
    
       out = new BMediaFile(&ref, &mfi);
       err = out->InitCheck();
       if (err != B_OK) {
          printf("failed to properly init the output file... (%s)\n",
                 strerror(err));
          delete out;
          return;
       }

Once the appropriate media file format has been determined, a new [`BMediaFile`](BMediaFile.md "BMediaFile") is created. The [entry_ref](BEntry.md#entry_ref "entry_ref") of the output file is specified, and the media file format we selected is provided. This will create the new media file, with no tracks. If this fails ([`BMediaFile::InitCheck()`](BMediaFile.md#BMediaFile_InitCheck "InitCheck\(\)") returns something other than `B_OK`), an error message is displayed, the [`BMediaFile`](BMediaFile.md "BMediaFile") is deleted, and the function returns.

The next chunk of code handles creating the new video track. Note that this only runs if `vidtrack`, the reference to the original [`BMediaTrack`](BMediaTrack.md "BMediaTrack"), isn't `NULL`. If it's `NULL`, the file is assumed not to have a video track.
    
    
       if (vidtrack) {
          vidtrack->EncodedFormat(&format);
    
          if (video_name) {
             int width, height;
             width = format.u.encoded_video.output.display.line_width;
             height = format.u.encoded_video.output.display.line_count;
    
             memset(&format, 0, sizeof(format));
             BuildMediaFormat(width, height, B_RGB32, &format);
    
             vidtrack->DecodedFormat(&format);
    
             bitmap = (char *)malloc(width * height * 4);
    
             cookie = 0;
             while (get_next_encoder(&cookie, &mfi, &format,
                                   &outfmt, &mci) == B_OK) {
                printf("found encoder %s (%d)\n", mci.pretty_name,
                       mci.id);
                if (strcmp(video_name, mci.short_name) == 0) {
                   found_video_encoder = true;
                   break;
                }
             }
          }

If the video track exists, we determine the encoded format by calling [`BMediaTrack::EncodedFormat()`](BMediaTrack.md#BMediaTrack_EncodedFormat "EncodedFormat\(\)"). The returned [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") describes the format of the video frames in the original file.

If a video encoder for the output was specified, a [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") constructed using the `BuildMediaFormat()` function we implemented previously. This format will be used for reading the frames from the original file into a raw, unencoded video buffer. We call [`BMediaTrack::DecodedFormat()`](BMediaTrack.md#BMediaTrack_DecodedFormat "DecodedFormat\(\)") on the original video track, specifying that this format should be used for outputting frames. From this point onward, frames delivered by `vidtrack` will be in raw video format.

A bitmap buffer is created. This will contain each frame of raw video while it's being transcoded, after being decoded and before it's encoded into the new file.

Another loop is used to locate an encoder that can convert raw video frames into the desired output format. This is done by calling [`get_next_encoder()`](BMediaFormats.md#get_next_encoder "get_next_encoder\(\)") in a loop, looking for an encoder that can accept data in the [media_file_format](TheMediaKit_DefinedTypes.md#media_file_format "media_file_format") specified by `mfi`, converting the frames into the [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") specified by `format`. A description of the encoder is returned in `mci`. When a match is found, we check to see if it matches the encoder name, `video_name`, requested by the input arguments. If it does, we accept it by setting the `found_video_encoder` flag to `true`.
    
    
          if (found_video_encoder)
             vid = out->CreateTrack(&format, &mci);
          else
             vid = out->CreateTrack(&format);
    
          if (vid == NULL) {
             printf("Failed to create video track\n");
             delete out;
             return;
          }
       }

If a video encoder has been found, a new video track is created, using the found encoder, `mci`. Otherwise, a raw video track is created. This covers the case where the user indicates that he doesn't want any encoding (if they specify a `NULL` `video_name`). A pointer to the new [`BMediaTrack`](BMediaTrack.md "BMediaTrack") is kept in `vid`.

If the track can't be created, an error message is displayed, the [`BMediaFile`](BMediaFile.md "BMediaFile") for the output is deleted, and the function returns.

Next, the audio track is prepared in the same way: the encoded format is determined, the decoded format is specified, and an encoder is located, followed by the creation of the new track. A `sound_buffer` is allocated to contain the amount of audio data that will be stuffed into each buffer returned as the source track is read.
    
    
       if (audtrack) {
          audtrack->EncodedFormat(&format);
    
          audtrack->DecodedFormat(&format);
          sound_buffer = (char*)malloc(format.u.raw_audio.buffer_size);
          framesize = (format.u.raw_audio.format&15)*
                       format.u.raw_audio.channel_count;
    
          if (audio_name) {
             cookie = 0;
             while (get_next_encoder(&cookie, &mfi, &format,
                                     &outfmt, &mci) == B_OK) {
                printf("found encoder %s (%d)\n", mci.pretty_name, mci.id);
                if (strcmp(audio_name, mci.short_name) == 0) {
                   found_audio_encoder = true;
                   break;
                }
             }
          }
    
          if (found_audio_encoder)
             aud = out->CreateTrack(&format, &mci);
          else
             aud = out->CreateTrack(&format);
    
          if (aud == NULL) {
             printf("Failed to create audio track\n");
             delete out;
             return;
          }
       }

Final touches are then put on the new file's header.
    
    
       // Add the copyright and commit the header
       out->AddCopyright("Copyright 1999 Be Incorporated");
       out->CommitHeader();

In this example, a copyright notice is added to the file by calling [`BMediaFile::AddCopyright()`](BMediaFile.md#BMediaFile_AddCopyright "AddCopyright\(\), Copyright\(\)"). Once everything's ready, we call [`BMediaFile::CommitHeader()`](BMediaFile.md#BMediaFile_CommitHeader "CommitHeader\(\)") to indicate that we're about to begin writing media data into the file.

Then writing of the video track begins, if there is one.
    
    
       // Process the video track, if any
       if (vidtrack) {
          int is_key_frame = 0;
    
          if (found_video_encoder) {
             numFrames = vidtrack->CountFrames();
             for(j = 0; j < numFrames; j++) {
                int64 framecount = 1;
                printf(" r");
                printf("processing frame: %5Ld", j);
                fflush(stdout);
                err = vidtrack->ReadFrames(bitmap, &framecount, &mh);
                if (err) {
                   printf("video: GetNextChunk error -- %s\n",
                          strerror(err));
                   break;
                }
                err = vid->WriteFrames(bitmap, 1,
                      mh.u.encoded_video.field_flags);
                if (err) {
                   printf("err %s (0x%x) writing video frame %Ld\n",
                          strerror(err), err, j);
                   break;
                }
             }

If there's a video encoder in use (the output data will be encoded), we count up the number of frames in the video track by calling [`BMediaTrack::CountFrames()`](BMediaTrack.md#BMediaTrack_CountFrames "CountFrames\(\)"), then loop over all of the frames.

For each frame, we display a status notice to let the user know what we're doing, then read in the next frame from the source file by calling [`BMediaTrack::ReadFrames()`](BMediaTrack.md#BMediaTrack_ReadFrames "ReadFrames\(\)"). We specify the `bitmap` buffer as the buffer into which the frame should be read, and `framecount` will contain the number of read frames (it should always be 1 for video tracks). `mh`, a [media_header](TheMediaKit_DefinedTypes.md#media_header "media_header"), will receive a description of the data in the buffer.

If an error occurs reading the frame, an error message is printed, and the video conversion is terminated.

Otherwise, the output video track, `vid`, is written into by calling [`BMediaTrack::WriteFrames()`](BMediaTrack.md#BMediaTrack_WriteFrames "WriteFrames\(\)"). The new frame is automatically encoded and appended to the new file. If an error occurs doing this, an error message is displayed and processing is terminated.

If there's no encoder being used (the output file is going to be written as raw or unknown-format data), the following code is used:
    
    
          } else {
             numFrames = vidtrack->CountFrames();
             for(j = 0; j < numFrames; j++) {
                printf(" r");
                printf("processing frame: %5Ld", j);
                fflush(stdout);
    
                err = vidtrack->ReadChunk(&chunk, &sz, &mh);
                if (err) {
                   printf("video: GetNextChunk error -- %s\n",
                          strerror(err));
                   break;
                }
    
                err = vid->WriteChunk(chunk, sz,
    mh.u.encoded_video.field_flags);
                if (err) {
                   printf("err %s (0x%x) writing video frame %Ld\n",
                         strerror(err), err, j);
                   break;
                }
             }
          }
          printf("r r");
       }

The only real difference here is that instead of using [`BMediaTrack::ReadFrames()`](BMediaTrack.md#BMediaTrack_ReadFrames "ReadFrames\(\)") and [`BMediaTrack::WriteFrames()`](BMediaTrack.md#BMediaTrack_WriteFrames "WriteFrames\(\)"), we use [`BMediaTrack::ReadChunk()`](BMediaTrack.md#BMediaTrack_ReadChunk "ReadChunk\(\)") and [`BMediaTrack::WriteChunk()`](BMediaTrack.md#BMediaTrack_WriteChunk "WriteChunk\(\)"). These work with media data without attempting to interpret the data in any way.

One or the other of these loops will continue until either an error occurs, or the entire video track is converted.

Next, the audio track is converted, if there is one. As you can see, this is done in almost exactly the same manner, except that each buffer we receive will have more than one frame of audio in it. Note how the loop that iterates over the frames adds `framecount` to its counter variable each pass; `framecount` indicates the number of frames returned by the last call to [`BMediaTrack::ReadFrames()`](BMediaTrack.md#BMediaTrack_ReadFrames "ReadFrames\(\)") or [`BMediaTrack::ReadChunk()`](BMediaTrack.md#BMediaTrack_ReadChunk "ReadChunk\(\)").
    
    
       // Process the audio track, if any
       if (audtrack) {
          int64       framecount = 0;
    
          if (found_audio_encoder) {
             // Decode and encode all the frames
             numFrames = audtrack->CountFrames();
             printf("Total frame count : %Ld\n", numFrames);
             for (j = 0; j < numFrames; j+=framecount) {
                err = audtrack->ReadFrames(sound_buffer, &framecount, &mh);
                if (err) {
                   printf("video: GetNextChunk error -- %s\n", strerror(err));
                   break;
                }
    
                err = aud->WriteFrames(sound_buffer, framecount);
                if (err) {
                   printf("err %s (0x%x) writing audio frame %Ld\n",
                         strerror(err), err, j);
                   break;
                }
             }
          } else {
             printf("processing chunks...\n");
             while (true) {
                err = audtrack->ReadFrames(sound_buffer, &framecount, &mh);
                if (err) {
                   printf("audio: GetNextChunk error -- %s\n", strerror(err));
                   break;
                }
    
                err = aud->WriteChunk(sound_buffer, framecount*framesize);
                if (err) {
                   printf("err %s (0x%x) writing audio chunk %Ld\n",
                         strerror(err), err, j);
                   break;
                }
             }
          }
          printf("r r");
       }

Once converting the audio is done, there's very little left to do but release the video and audio buffers we've allocated, close the output [`BMediaFile`](BMediaFile.md "BMediaFile"), and delete it.
    
    
       if (bitmap)
          free(bitmap);
       if (sound_buffer)
          free(sound_buffer);
    
       out->CloseFile();
       delete out;
       out = NULL;
    }

After this function returns, the caller is responsible for deleting the source [`BMediaFile`](BMediaFile.md "BMediaFile").

* * *

### Using the transcode() Function

Let's look at a `main()` that uses the `transcode()` function to provide a command-line utility for converting media files from one format to another.
    
    
    int main(int argc, char **argv) {
       status_t err;
       entry_ref ref;
       media_format format;
       BMediaFile mediaFile;
       BMediaTrack *track = NULL, *vidtrack = NULL, *audtrack = NULL;
       int32 i, numTracks;
       char *input = NULL, *output = NULL;
       char *video_encoder_name = NULL, *audio_encoder_name = NULL;
       char *family_name = NULL;
    
       if (argc < 2) {
          printf("usage: %s [-info][-avi|-qt][-wav][-aiff][-v
    <encoder_name>][-a <encoder_name>] <filename> [<output>]n", argv[0]);
          return 1;
       }

If the number of arguments is less than 2, a usage notice is printed.
    
    
       for (i=1; i < argc; i++) {
          if (strcmp(&argv[i][0], "-info") == 0) {
             dump_info();
             exit(0);
          } else if (strcmp(&argv[i][0], "-avi") == 0 ||
                   strcmp(&argv[i][0], "-wav") == 0 ||
                   strcmp(&argv[i][0], "-aiff") == 0 ||
                   strcmp(&argv[i][0], "-quicktime") == 0) {
             family_name = &argv[i][1];
          } else if (strcmp(&argv[i][0], "-qt") == 0) {
             family_name = "quicktime";
          } else if (strcmp(&argv[i][0], "-v") == 0 && argv[i+1]) {
             video_encoder_name = argv[i+1];
             i++;
          } else if (strcmp(&argv[i][0], "-a") == 0 && argv[i+1]) {
             audio_encoder_name = argv[i+1];
             i++;
          } else if (input == NULL) {
             input = &argv[i][0];
          } else if (output == NULL) {
             output = &argv[i][0];
          } else {
             printf("%s: extra argument %s\n", &argv[0][0], &argv[i][0]);
          }
       }

The arguments are interpreted here. The arguments are:

Argument| Description  
---|---  
`-info`:| Dumps information about the available media file formats and encoders; this function can be found in the description of the [`get_next_file_format()`](TheMediaKit_Functions.md#get_next_file_format "get_next_file_format\(\)") function.  
`-avi`:| Specifies that the output file should be in AVI format; this sets the `family_name` to "avi".  
`-wav`:| Specifies that the output file should be in WAV format; this sets the `family_name` to "wav".  
`-aiff`:| Specifies that the output file should be in AIFF format; this sets the `family_name` to "aiff".  
`-quicktime`:| Specifies that the output file should be a QuickTime movie; this sets the `family_name` to "quicktime".  
`-qt`:| A shorthand form of `-quicktime`.  
`-v`:| Lets you specify a video encoder name other than one of the above, such as "-v myformat".  
`-a`:| Lets you specify an audio encoder name other than one of the above, such as "-a audiowonderness".  
  
`input` is the input file's name, and `output` is the name of the new media file to be created.
    
    
       if (output == NULL)
          output = "output";
    
       err = get_ref_for_path(input, &ref);
       if (err) {
          printf("problem with get_ref_for_path() -- %s\n", strerror(err));
          return 1;
       }

If no output file name is specified, the name "output" is assumed. An [entry_ref](BEntry.md#entry_ref "entry_ref") to the input file is constructed; if the file isn't found, an error message is printed and the program exits.
    
    
       mediaFile = new BMediaFile(&ref);
       err = mediaFile->InitCheck();
       if (err) {
          printf("cannot contruct BMediaFile object -- %s\n", strerror(err));
          return 1;
       }

A [`BMediaFile`](BMediaFile.md "BMediaFile") is then instantiated, referencing the input file. If an error occurs, an error is displayed and the program terminates.
    
    
       numTracks = mediaFile->CountTracks();
       printf("%s has %d media tracks\n", input, numTracks);
       const char *copyright = mediaFile->Copyright();
       if (copyright)
          printf("#### copyright info: %s\n", copyright);

The number of tracks in the source file is obtained by calling [`BMediaFile::CountTracks()`](BMediaFile.md#BMediaFile_CountTracks "CountTracks\(\)"), and the file's copyright notice is obtained by calling [`BMediaFile::Copyright()`](BMediaFile.md#BMediaFile_Copyright). This information is printed for the user to read.
    
    
       for(i=0; i < numTracks; i++) {
          track = mediaFile->TrackAt(i);
          if (!track) {
             printf("cannot get track %d?!?\n", i);
             return 1;
          }
    
          // get the encoded format
          err = track->EncodedFormat(&format);
          if (err) {
             printf("BMediaTrack::EncodedFormat error -- %s\n", strerror(err));
             return 1;
          }
    
          if (format.type == B_MEDIA_RAW_VIDEO ||
             format.type == B_MEDIA_ENCODED_VIDEO) {
    
             vidtrack = track;
          } else if (format.type == B_MEDIA_RAW_AUDIO ||
                   format.type == B_MEDIA_ENCODED_AUDIO) {
    
             audtrack = track;
          } else {
             mediaFile->ReleaseTrack(track);
             track = NULL;
          }
       }

Next, a loop iterates over all the tracks in the file by calling [`BMediaFile::TrackAt()`](BMediaFile.md#BMediaFile_TrackAt "TrackAt\(\)") to obtain [`BMediaFile`](BMediaFile.md "BMediaFile") objects referencing them, one by one. Each track's encoded format is obtained, and is checked to see if the track represents a video or audio track. If it's a video track, it's kept in the `vidtrack` variable. If it's an audio track, it's kept in `audtrack`. If it's neither, the track is released.

This serves to search all the tracks for a video and an audio track to be converted; most movies will only have one of each, but may have other informational tracks, which need to be ignored for our purposes.
    
    
       if (vidtrack == NULL && audtrack == NULL) {
          printf("%s has no audio or video tracks?!?\n", input);
          return 1;
       }

If there's neither a video nor an audio track, the source file is empty and isn't worth converting, so an error message is displayed and the program terminates.
    
    
       if (family_name == NULL && vidtrack == NULL)
          family_name = "wav";
       else if (family_name == NULL)
          family_name = "quicktime";

If the user didn't specify an output family, and the file has no video track, the WAV file format is assumed. You can change the program to assume AIFF for audio-only files by changing "wav" to "aiff" here.

For any file that has a video track, if no family is specified by the user, QuickTime format is assumed. Again, if you want the default to be another format, you can change that here.
    
    
       transcode(vidtrack, audtrack, output, family_name,
                 video_encoder_name, audio_encoder_name);

The conversion is performed by passing all these parameters to `transcode()` to do the real work.
    
    
       delete mediaFile;
    
       return 0;
    }

Once `transcode()` returns, we simply delete the source file and exit the program. The file has been converted (assuming no errors occurred in `transcode()`.

* * *

### Integrating Into a Real Application

When creating a real application, you'll need to provide a way for the user to review a list of the various media file formats available, as well as the encoders provided for each format. The `dump_info()` sample function discussed in the description of the [`get_next_file_format()`](TheMediaKit_Functions.md#get_next_file_format "get_next_file_format\(\)") function shows how this can be done.

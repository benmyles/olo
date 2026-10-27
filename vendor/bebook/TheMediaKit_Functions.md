<!-- Classes And Methods - The Media Kit - Functions -->

## Functions

* * *

### format_is_compatible()

Declared in: `media/MediaDefs.h`

`bool format_is_compatible(const media_format& format1,  
const media_format& format2);`

Returns `true` if media data of format `format1` can be fed into a consumer that accepts data in [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") `format2`; otherwise, returns `false`.

* * *

### get_next_file_format()

Declared in: `media/MediaDefs.h`

`status_t get_next_file_format(int32* cookie,  
media_file_format* info);`

If you need to search through all available file formats, you can do so using this function. Point `cookie` at an int32 containing 1, and `info` at a [media_file_format](TheMediaKit_DefinedTypes.md#media_file_format "media_file_format") structure, and call `get_next_file_format()`; on return, `info` will contain information about the first available file format.

You can call `get_next_file_format()` repeatedly until `B_BAD_INDEX` is returned, which indicates that there aren't any more file formats to scan.

The following snippet dumps a list of all the file formats, including all the audio and video codecs each file format supports.
    
    
    void dump_info(void) {
       int32 cookie = 0, cookie2;
       media_format format, outfmt;
       media_file_format mfi;
       media_codec_info mci;
    
       while(get_next_file_format(&cookie, &mfi) == B_OK) {
          printf("%s (%s, id %d)\n", mfi.pretty_name,
                   mfi.short_name, mfi.id);
    
          cookie2 = 0;
          memset(&format, 0, sizeof(format));
          format.type = B_MEDIA_RAW_VIDEO;
    
          format.u.raw_video.last_active = (uint32) (320 - 1);
          format.u.raw_video.orientation = B_VIDEO_TOP_LEFT_RIGHT;
          format.u.raw_video.pixel_width_aspect = 1;
          format.u.raw_video.pixel_height_aspect = 3;
          format.u.raw_video.display.format = B_RGB32;
          format.u.raw_video.display.line_width = (int32) 320;
          format.u.raw_video.display.line_count = (int32) 240;
          format.u.raw_video.display.bytes_per_row = 4 * 320;
    
          printf(" Video Encoders:\n");
          while (get_next_encoder(&cookie2, &mfi, &format,
                                  &outfmt, &mci) == B_OK) {
             printf(" %s / %s (%d)\n", mci.pretty_name, mci.short_name,
                      mci.id);
          }
    
          cookie2 = 0;
          format.type = B_MEDIA_RAW_AUDIO;
          format.u.raw_audio.format =
             media_raw_audio_format::B_AUDIO_UCHAR;
          format.u.raw_audio.channel_count = 1;
          printf(" Audio Encoders:\n");
          while (get_next_encoder(&cookie2, &mfi, &format,
                                  &outfmt, &mci) == B_OK) {
             printf(" %s / %s (%d)\n", mci.pretty_name, mci.short_name,
                      mci.id);
          }
       }
    }

Return Code| Description  
---|---  
`B_OK`| A format was returned.  
`B_BAD_VALUE`| An invalid `cookie` was specified.  
`B_BAD_INDEX`| No more file formats, or the `cookie` was invalid.  
  
* * *

###  launch_media_server(), shutdown_media_server()

Declared in: `media/MediaDefs.h`

`status_t launch_media_server(uint32 flags = 0);``status_t shutdown_media_server(bigtime_t timeout = B_INFINITE_TIMEOUT,  
bool  (*progress)(int stage, const char* message, void* cookie)  = NULL,  
void* cookie = NULL);`

`launch_media_server()` starts up the Media Servers (and the old Audio Server); you can use this if you discover that the Media Server isn't started but you need to use it.

`shutdown_media_server()` shuts down the Media Servers (and the old Audio Server). If you'd like to be able to keep track of the shutdown process, specify a pointer to a function in `progress`; this function will receive as input an integer value, `stage`, indicating how far through the shutdown process things are (100 means the shutdown is complete), `message` will be a test string describing what's going on, and `cookie` will be the same value as the cookie you specified as input into `shutdown_media_server()`.

The shutdown process will abort if the `timeout` expires.

* * *

### media_realtime_init_image()

Declared in: `media/MediaDefs.h`

`status_t media_realtime_init_image(image_id image,  
uint32 flags);`

Given the specified image, this function prepares the image for realtime media. The flags indicate what type of media the image will be handling; this must match the realtime flags specified by the [`BMediaRoster::SetRealtimeFlags()`](BMediaRoster.md#BMediaRoster_SetRealtimeFlags) function (or by the Media preference application, which calls it). If the flags don't match up, `B_MEDIA_REALTIME_DISABLED` is returned.

Return Code| Description  
---|---  
`B_OK`| The image is ready for realtime media.  
`B_MEDIA_REALTIME_DISABLED`| The media type indicated by `flags` isn't enabled for realtime (see [`BMediaRoster::SetRealtimeFlags()`](BMediaRoster.md#BMediaRoster_SetRealtimeFlags)).  
`B_MEDIA_REALTIME_UNAVAILABLE`| There aren't enough system resources available to enable realtime performance.  
  
* * *

### media_realtime_init_thread()

Declared in: `media/MediaDefs.h`

`status_t media_realtime_init_thread(thread_id thread,  
size_t stackUsed,  
uint32 flags);`

Prepares the specified `thread` for realtime media. The `stackUsed` argument specifies how much of the stack should be locked down in memory. It's very unlikely that your thread actually uses the entire 256kB stack allocated for it, so you can preserve valuable system resources by indicating an upper bound for the amount of stack to lock in memory. If you need to lock down the entire stack, you can pass 0.

The `flags` argument indicates the type of media the thread will be handling.

Return Code| Description  
---|---  
`B_OK`| The thread is ready for realtime media.  
`B_MEDIA_REALTIME_DISABLED`| The media type indicated by `flags` isn't enabled for realtime (see [`BMediaRoster::SetRealtimeFlags()`](BMediaRoster.md#BMediaRoster_SetRealtimeFlags)).  
`B_MEDIA_REALTIME_UNAVAILABLE`| There aren't enough system resources available to enable realtime performance.  
  
* * *

### play_sound()

Declared in: `media/PlaySound.h`

`sound_handle play_sound(const entry_ref* soundRef,  
bool willMix,  
bool willWait,  
bool background);`

`play_sound()` plays the sound file identified by the entry_ref pointed to by the `soundRef` parameter. The `willMix` and `willWait` arguments are used to determine how the function behaves with regard to other sounds.

  * If you want the sound to play all by itself, set `willMix` to `false`. If you don't care if it gets mixed with other sounds, set it to `true`.

  * If you want your sound to play immediately (whether or not you're willing to mix), set `willWait` to `false`. If you're willing to wait for the sound playback resources to become available, set `willWait` to `true`.

Note that setting `willMix` to true doesn't guarantee that your sound will play immediately. If the sound playback resources are claimed for exclusive access by some other process, you'll be blocked, even if you're willing to mix.

The `background` argument, if `true`, tells the function to spawn a thread in which to play the sound. In this case, the function returns immediately. If `background` is `false`, the sound is played synchronously and `play_sound()` won't return until the sound has finished.

The sound_handle value that's returned is a token that represents the sound that's being played back, and is only valid if you're playing in the background. You would use this token in a subsequent call to [`stop_sound()`](TheMediaKit_Functions.md#stop_sound "stop_sound\(\)") or [`wait_for_sound()`](TheMediaKit_Functions.md#wait_for_sound "wait_for_sound\(\)"). If the entry_ref doesn't represent a file, or if the sound couldn't be played, for whatever reason, `play_sound()` returns a negative integer.

* * *

### stop_sound()

Declared in: `media/PlaySound.h`

`status_t stop_sound(sound_handle handle);`

`stop_sound()` stops the playback of a sound identified by `handle`, a value that was returned by a previous call to [`play_sound()`](TheMediaKit_Functions.md#play_sound "play_sound\(\)"). The return value can be ignored.

* * *

### string_for_format()

Declared in: `media/MediaDefs.h`

`bool string_for_format(const media_format& format,  
char* outBuffer,  
size_t bufferSize);`

Fills the buffer specified by `outBuffer` with a string describing the specified format. The buffer's size is specified by the `bufferSize` argument. The string may not be pretty, but it will list all kinds of interesting information about the format.

If a description is returned, this function returns `true`. Otherwise, the result is `false`.

* * *

### wait_for_sound()

Declared in: `media/PlaySound.h`

`status_t wait_for_sound(sound_handle handle);`

Causes the calling thread to block until the sound specified by `handle` has finished playing. The `handle` value should be a value returned by a previous call to [`play_sound()`](TheMediaKit_Functions.md#play_sound "play_sound\(\)"). `wait_for_sound()` currently always returns `B_OK`.

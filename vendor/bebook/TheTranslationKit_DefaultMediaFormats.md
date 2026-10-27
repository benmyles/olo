<!-- Classes And Methods - The Translation Kit - Default Media Formats -->

## Default Media Formats

Declared in: `translation/TranslatorFormats.h`

This section describes the default media formats for each of the media groups defined by the Translation Kit.

* * *

### Media Formats

#### B_TRANSLATOR_BITMAP
    
    
    struct TranslatorBitmap {
        int32 magic;
        BRect bounds;
        uint32 rowBytes;
        color_space colors;
        uint32 dataSize;
    }

TranslatorBitmap holds the header information for a translator bitmap. `magic` should always be set to `B_TRANSLATOR_BITMAP`. The uncompressed image data immediately follows the header in the stream. This image data should be precisely `dataSize` bytes long.

All data in the header is stored in big endian format. The data is stored in either big- or little- endian format, depending on the value of the `colors` field.

#### B_TRANSLATOR_MIDI

This is a standard type 1 MIDI file, as read by and written to the [`BMidiStore`](BMidiStore.md "BMidiStore") class.

#### B_TRANSLATOR_MEDIA

As of this writing, a standard for this format has not been finalized, although it will be in a future release.

#### B_TRANSLATOR_NONE

Obviously, there is no standard format for the no media type.

#### B_TRANSLATOR_PICTURE

Streams in this format are stored as `BPicture::Flatten()`.

####  B_TRANSLATOR_SOUND
    
    
    struct TranslatorSound {
        int32 magic;
        uint32 channels;
        float sampleFreq;
        uint32 numFrames;
    }

TranslatorSound holds the header information for the default sound format. `magic` should always be set to `B_TRANSLATOR_SOUND`. The raw sound data stored in 16 bit linear format, left channel first, immediately follows the header. The data in both the header and the stream are stored in big endian format.

#### B_TRANSLATOR_TEXT

This is a plain ASCII text file.

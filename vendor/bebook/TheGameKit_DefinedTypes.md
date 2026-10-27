<!-- Classes And Methods - The Game Kit - Defined Types -->

## Defined Types  
  
* * *

### frame_buffer_info

Declared in: `add-ons/graphics/GraphicsCard.h`
    
    
    typedef struct {
        short bits_per_pixel;
        short bytes_per_row;
        short width;
        short height;
        short display_width;
        short display_height;
        short display_x;
        short display_y;
    } frame_buffer_info

This structure is used to report the current configuration of the frame buffer.

See also: [`FrameBufferInfo()`](BWindowScreen.md#BWindowScreen_FrameBufferInfo "FrameBufferInfo\(\)")

* * *

### graphics_card_hook

Declared in: `add-ons/graphics/GraphicsCard.h`
    
    
    typedef void (*graphics_card_hook)(void)

This is the general type declaration for a graphics card hook function. Specific hook functions will in fact declare various sets of arguments and all return a status_t error code rather than void.

See also: [`CardHookAt()`](BWindowScreen.md#BWindowScreen_CardHookAt "CardHookAt\(\)"), "[Graphics Card Hook Functions](TheGameKit_CardHooks.md "Graphics Card Hook Functions")"

* * *

### graphics_card_info

Declared in: `add-ons/graphics/GraphicsCard.h`
    
    
    typedef struct {
        short version;
        short id;
        void* frame_buffer;
        char  rgba_order[4];
        short flags;
        short bits_per_pixel;
        short bytes_per_row;
        short width;
        short height;
    } graphics_card_info

Drivers use this structure to supply information about themselves and the current configuration of the frame buffer to the Application Server and the [`BWindowScreen`](BWindowScreen.md "BWindowScreen") class.

See also: [`CardInfo()`](BWindowScreen.md#BWindowScreen_CardInfo "CardInfo\(\)")

* * *

### gs_attribute

Declared in: `game/GameSoundDefs.h`
    
    
    struct gs_attribute {
        int32 attribute;
        bigtime_t duration;
        float value;
        uint32 flags;
    };

Defines an attribute. An attribute consists of an attribute number from gs_attributes, a `duration` indicating the period of time, in microseconds, over which the attribute's change takes effect, and a target `value`. Additional `flags` can be specified for the attribute as well; these vary depending on the attribute.

Currently, there are no flags defined for any of the predefined attributes.

* * *

### gs_attribute_info

Declared in: `game/GameSoundDefs.h`
    
    
    struct gs_attribute_info {
        int32 attribute;
        float granularity;
        float minimum;
        float maximum;
    };

Describes the possible values the attribute can take. The granularity field indicates how finely the value of the attribute can be controlled, and minimum and maximum specify the minimum and maximum values the attribute can take on.

* * *

### gs_audio_format

Declared in: `game/GameSoundDefs.h`
    
    
    struct gs_audio_format {
        enum format {
           B_GS_U8  = 0x11,
           B_GS_S16 = 0x2,
           B_GS_F   = 0x24,
           B_GS_S32 = 0x4
        };
        float frame_rate;
        uint32 channel_count;
        uint32 format;
        uint32 byte_order;
        size_t buffer_size;
    };

This structure describes the format of a game sound.

Field| Description  
---|---  
`format`| The format enum lists the possible sound sample formats supported by the Game Kit:

`B_GS_U8`
    Unsigned 8-bit integer.
`B_GS_S16`
    Signed 16-bit integer.
`B_GS_F`
    Floating-point.
`B_GS_S32`
    Signed 32-bit integer.  
`frame_rate`| Indicates how many frames per second of audio should be played.  
`channel_count`| Indicates the number of audio channels the sound uses.  
`format`| Specifies the format of the audio data to be played must be one of the values declared in the format enum.  
`byte_order`| Specifies the byte order of the sound to be played.  
`buffer_size`| Used to specify how large the audio buffers used to play the sound should be. You can specify zero if you want the Game Kit to determine an appropriate size for you.  
  
* * *

### gs_id

Declared in: `game/GameSoundDefs.h`
    
    
    typedef int32 gs_id;

This type is used for game sound ID numbers.

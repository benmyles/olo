<!-- System Overview - The Game Kit - Graphics Card Hook Functions -->

## Graphics Card Hook Functions

A graphics card driver can implement a [set of hook functions](TheGameKit_CardHooks.md "Graphics Card Hook Functions") that perform specific tasks on behalf of the Application Server and the Game Kit's [`BWindowScreen`](BWindowScreen.md "BWindowScreen") object. Drivers should implement as many of these functions as they can.

The Application Server asks the driver for the locations of its hook functions by passing the `B_GET_GRAPHICS_CARD_HOOKS` opcode. In response to this request, the driver writes an array of function pointers to data, cast as a graphics_card_hook pointer (described below). The `B_GET_GRAPHICS_CARD_HOOKS` request is made whenever the configuration of the frame buffer changes. The driver can thus provide hook functions that are tailored to specific frame buffer configurations.

The system can accommodate `B_GRAPHICS_HOOK_COUNT` (48) hook functions. Currently, only the first 14 functions are used. These functions fall into four groups:

[Indices 0–2: Cursor management.](TheGameKit_CardHooks.md#define_cursor "define_cursor\(\)")
    

Drivers must implement all three of these functions, or none of them.

[Indices 3–9, 12, and 13: Drawing functions.](TheGameKit_CardHooks.md#draw_line_with_8_bit_depth "draw_line_with_8_bit_depth\(\)")
    

These are specific drawing tasks, such as drawing a line or filling a rectangle. A driver can implement as many of these as it wishes.

[Index 10: Driver/Application Server synchronization.](TheGameKit_CardHooks.md#sync "sync\(\)")
    

Drivers should implement this function only if the other hook functions are performed asynchronously.

[Index 11: Color inversion.](TheGameKit_CardHooks.md#invert_rect "invert_rect\(\)")
    

This function inverts the colors in a rectangle.

* * *

### The graphics_card_hook Type

All pointers in the hook function array are declared to be of type graphics_card_hook:
    
    
    typedef void (*graphics_card_hook)(void)

The code that fills data will look something like this:
    
    
    int32 control_graphics_card(uint32 opcode, void *data)
    {
       switch (opcode) {
       ...
       case B_GET_GRAPHICS_CARD_HOOKS:
          ((graphics_card_hook *)data)[0] =
             (graphics_card_hook)define_cursor;
    
          ((graphics_card_hook *)data)[1] =
             (graphics_card_hook)move_cursor;
       ...
       }
    }

Despite the graphics_card_hook declaration, each function has its own set of arguments and returns an integer error code. All functions should return `B_OK` if they're successful, and `B_ERROR` if not.

* * *

### Unimplemented Functions

All hook functions that the driver doesn't implement (including indices 14–47) should be set to `NULL`. For example:
    
    
    ((graphics_card_hook *)data)[14] = (graphics_card_hook)NULL;

A driver can chose to nullify functions it does implement if it wants to defer to the Application Server version.

Warning

![Warning](./images/admonitions/Stop_32.png)

Don't implement a function to simply (always) return `B_ERROR`. If you want to declare a hook as a no-op, you must pass `NULL` as the pointer.

* * *

### Coordinate Spaces

Most hook function coordinates are in depth-independent "frame buffer space" (the exceptions are well noted below). In other words, a coordinate pair gives the location of a pixel in the frame buffer independent of the buffer's depth; pixel (0, 0) is at the left top corner of the frame buffer.

* * *

### Naming

You can name the functions whatever you wish—the Application Server finds the functions by index in the hook function array. For convenience and clarity, the following descriptions suggest default names.

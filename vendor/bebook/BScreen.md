<!-- Classes And Methods - The Interface Kit - BScreen -->

## BScreen

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/Screen.h`  
Library:| `libbe.so`  
Allocation:| Constructor or on the stack  
[Class Overview](BScreen_Overview.md "BScreen")

  * [Constructor and Destructor](BScreen.md#BScreen_ConstructorDestructor)
  * [Member Functions](BScreen.md#BScreen_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BScreen()

`BScreen([`BWindow`](BWindow.md "BWindow")* window);` `BScreen(screen_id id = B_MAIN_SCREEN_ID);`

Initializes the `BScreen` object so that it represents the screen where `window` is displayed or the screen identified by `id`. If `window` is `NULL` or hidden, or if the `id` is invalid, the `BScreen` will represent the main screen.

Note

![Note](./images/admonitions/Info_32.png)

Since multiple monitors aren't currently supported, there's no API for screen identifiers other than for the main screen.

To be sure the new object was correctly constructed, call [`IsValid()`](BScreen.md#BScreen_IsValid "IsValid\(\)").

#### ~BScreen()

`~BScreen();`

Unlocks the screen and invalidates the `BScreen` object.

* * *

### Member Functions

####  ColorMap(), IndexForColor(), ColorForIndex()

`const color_map* ColorMap();`

`inline uint8 IndexForColor(rgb_color color);` `uint8 IndexForColor(uint8 red,  
uint8 green,  
uint8 blue,  
uint8 alpha = 255);`

`rgb_color ColorForIndex(const uint8 index);``uint8 InvertIndex(uint8 index);`

These functions return information from the color_map structure for this screen. The [color_map](TheInterfaceKit_DefinedTypes.md#color_map "color_map") structure defines the set of 256 colors that can be displayed in an `B_CMAP8` color space. A single color_map is shared by all applications that display on the same screen. See the color_map structure for more information about the structure.

`ColorMap()` returns a pointer to the color_map itself. The structure belongs to the `BScreen` object; you can't modify or free it. (Note that the the `system_colors()` function retrieves the color_map structure for the main screen without reference to a `BScreen` object.)

`IndexForColor()` returns the "index" of the 8-bit color that, in this screen's color map, most closely matches the given 32-bit color. You can pass the `index` to functions such [`BBitmap::SetBits()`](BBitmap.md#BBitmap_SetBits "SetBits\(\)") to set an 8-bit color. Note that `IndexForColor()` knows how to convert `B_TRANSPARENT_32_BIT` into `B_TRANSPARENT_8_BIT`.

`ColorForIndex()` returns the 32-bit color representation of a given 8-bit color `index`. This function doesn't convert `B_TRANSPARENT_8_BIT` into `B_TRANSPARENT_32_BIT`.

`InvertIndex()` takes an 8-bit `index` and returns an index that represents the color's "inversion." Inverted colors are typically used for highlighting.

Important

![Important](./images/admonitions/Warning_32.png)

The information gained through `IndexForColor()`, `ColorForIndex()`, and `InvertIndex()` can be retrieved more efficiently from the color_map structure. If you're repeatedly calling these functions, you should consider accessing the color_map structure, instead. Note, however, that the intelligent `B_TRANSPARENT_32_BIT` to `B_TRANSPARENT_8_BIT` conversion is not supported by the structure.

#### ColorSpace()

`color_space ColorSpace();`

Returns the color space of the screen display—typically `B_CMAP8`, `B_RGB15`, or `B_RGB32`—or `B_NO_COLOR_SPACE` if the `BScreen` object is invalid.

The color space is set by the user through the Screen preferences application. You can set it programatically through the [`SetMode()`](BScreen.md#BScreen_SetMode) function.

#### Frame()

`[`BRect`](BRect.md "BRect") Frame();`

Returns the rectangle that locates the screen in the screen coordinate system. For example, the frame for a 1,024 * 768 main screen looks like this:
    
    
    [BRect](BRect.md "BRect")(0.0, 0.0, 1023.0, 767.0)

If the `BScreen` object is invalid, all sides of the rectangle are set to 0.0.

The screen's frame rectangle is set by the user through the Screen preferences application. You can set it programatically through the [`SetMode()`](BScreen.md#BScreen_SetMode) function.

#### GetDeviceInfo()

`status_t GetDeviceInfo(accelerant_device_info* info);`

Returns information about the graphics card.

####  GetModeList(), SetMode(), GetMode()

`status_t GetModeList(display_mode** mode_list,  
uint32* count);``status_t SetMode(display_mode* mode,  
bool makeDefault = false);``status_t GetMode(display_mode* mode);`

These functions set and get the screen's display mode. Each display_mode structure (defined in `add-ons/graphics/Accelerant.h`) is a distinct combination of screen size, pixel depth, and display timing.

`GetModeList()` allocates and returns, in `mode_list`, a list of the display_mode structures that the graphics card is guaranteed to support; `count` is set to the number of display_mode elements in the list. The caller is responsible for freeing `mode_list`.

Warning

![Warning](./images/admonitions/Stop_32.png)

There's no guarantee that the monitor can support all of the modes that `GetModeList()` retrieves.

`SetMode()` resets the screen to the given `mode`. If `makeDefault` is `true`, the mode becomes the default for the current workspace.

`GetMode()` copies the current display_mode into `mode`.

The display_mode structure is:
    
    
    typedef struct {
          display_timing timing;
          uint32 space;
          uint16 virtual_width;
          uint16 virtual_height;
          uint16 h_display_start;
          uint16 v_display_start;
          uint32 flags;
    } display_mode;

Field| Description  
---|---  
`timing`| Provides CTRC timing information.  
`space`| Is the color space of the display.  
`virtual_width`| Is the screen's virtual width in pixels  
`virtual_height`| Is the screen's virtual height in lines.  
`h_display_start`| Is the first displayed pixel in a line  
`v_display_start`| Is the first displayed line.  
`flags`| Are mode flags:

`B_SCROLL`
    Scrolling display; a large display is being simulated by scrolling around on a smaller screen.
`B_8_BIT_DAC`
    The DAC is in 8-bit mode.
`B_HARDWARE_CURSOR`
    The mode supports a hardware cursor.
`B_PARALLEL_ACCESS`
    The mode supports parallel access.
`B_DPMS`
    The mode supports power management.
`B_IO_FB_NA`
    The graphics card's frame buffer shouldn't be touched by the Application Server while the card's acceleration engine might be doing so.  
  
The display_timing structure is:
    
    
    typedef struct {
          uint32 pixel_clock;
          uint16 h_display;
          uint16 h_sync_start;
          uint16 h_sync_end;
          uint16 h_total;
          uint16 v_display;
          uint16 h_display;
          uint16 v_sync_start;
          uint16 v_sync_end;
          uint16 v_total;
          uint32 flags;
    } display_timing;

Field| Description  
---|---  
`pixel_clock`| Is in kHz.  
`h_display`| Is in pixels, not in character clocks.  
`v_display`| is in lines.  
`flags`| are:

`B_BLANK_PEDESTAL`
    Use a 7.5 IRE blanking pedestal instead of a 0.0 IRE blanking pedestal. Usually 0.0 IRE.
`B_TIMING_INTERLACED`
    The mode is interlaced instead of progressively scanned. Rarely set; most modern displays don't need this.
`B_POSITIVE_HSYNC`
    If set, the mode uses a positive (high) sync polarity.
`B_POSITIVE_VSYNC`
    If set, the mode uses a negative (low) sync polarity.
`B_SYNC_ON_GREEN`
    The mode generates sync information on the green color signal.  
  
See also: [`ProposeMode()`](BScreen.md#BScreen_ProposeMode "ProposeMode\(\)")

#### GetPixelClockLimits()

`status_t GetPixelClockLimits(display_mode* mode,  
uint32* low,  
uint32* high);`

This function returns, in `low` and `high`, the minimum and maximum "pixel clock" rates (in thousands-of-pixels per second) that are possible for the given `mode`. Given the pixel clock and a display mode, you can determine the refresh rate range by dividing the pixel clock by the "real" size of the screen, thus:
    
    
    uint32 hi_clock, lo_clock;
    float hi_refresh, lo_refresh;
    float real_size;
    display_mode mode;
    
    GetMode(&mode);
    GetPixelClockLimits(&mode, &lo_clock, &hi_clock);
    
    /* The real screen dimensions (i.e. the dimensions for the purposes
    * of the gun) are given by the 'timing.h_total' and
    * 'timing.v_total' fields.
    */
    total_size = mode.timing.h_total * mode.timing.v_total
    
    /* Get the refresh rate by dividing the pixel clock by the total
    * screen size. Remember -- the pixel clock values are given in
    * kHz; we multiply by 1000.0 to retrieve refresh rates in Hz.
    */
    hi_refresh = ((float) hi_clock*1000.0)/(float) total_size;
    lo_refresh = ((float) lo_clock*1000.0)/(float) total_size;

Return Code| Description  
---|---  
`B_OK`.| Pixel clock limits returned successfully.  
`B_ERROR`.| No clock limits known.  
  
#### GetTimingConstraints()

`status_t GetTimingConstraints(display_timing_constraints* dtc);`

This function fills out the `dtc` structure with the timing constraints of the current display mode.

Return Code| Description  
---|---  
`B_OK`.| Constraints returned successfully.  
`B_ERROR`.| No constraints known.  
  
#### ID()

`screen_id ID();`

Returns the identifier for the screen. The main screen is identified as `B_MAIN_SCREEN_ID`.

The ID isn't presistent across boots, and may change if the monitor is diconnected and then reconnected.

Warning

![Warning](./images/admonitions/Stop_32.png)

Currently, this function always returns `B_MAIN_SCREEN_ID`, even if the `BScreen` object is invalid.

#### IsValid()

`bool IsValid();`

Returns `true` if the `BScreen` object is valid (if it represents a real screen connected to the computer), and `false` if not.

#### ProposeMode()

`status_t ProposeMode(display_mode* candidate,  
const display_mode* low,  
const display_mode* high);`

`ProposeMode()` is a convenience function that attempts to adjust `candidate` so that it's a supported mode (as listed by the [`GetModeList()`](BScreen.md#BScreen_GetModeList "GetModeList\(\), SetMode\(\), GetMode\(\)") function). It then compares the possibly-adjusted `candidate` to the limits declared in `low` and `high` and expresses this comparison in the return value. Note that the function doesn't adjust `candidate` so that it is, of necessity, between `low` and `high`.

Exactly how `ProposeMode()` works is up to the individual graphics driver. It's expected that the function will adjust `candidate`'s screen size fields while holding the color space constant.

Note

![Note](./images/admonitions/Info_32.png)

This function was formerly called `ProposeDisplayMode()`.

Return Code| Description  
---|---  
`B_OK`.| Candidate (as returned) is supported and falls within the limits.  
`B_BAD_VALUE`.| Candidate (as returned) is supported, but doesn't fall within the limits.  
`B_ERROR`.| candidate isn't supported.  
  
####  ReadBitmap(), GetBitmap()

`status_t ReadBitmap([`BBitmap`](BBitmap.md "BBitmap")* buffer,  
bool draw_cursor = true,  
[`BRect`](BRect.md "BRect")* bounds = NULL);``status_t GetBitmap([`BBitmap`](BBitmap.md "BBitmap")** buffer,  
bool draw_cursor = true,  
[`BRect`](BRect.md "BRect")* bounds = NULL);`

These functions provide read-only access to the screen by copying the screen's contents into the first argument [`BBitmap`](BBitmap.md "BBitmap"). The difference between them is that `ReadBitmap()` expects you to allocate the [`BBitmap`](BBitmap.md "BBitmap") before passing it in, while `GetBitmap()` allocates a new [`BBitmap`](BBitmap.md "BBitmap") for you. The caller is responsible for freeing the [`BBitmap`](BBitmap.md "BBitmap") allocated by `GetBitmap()`.

The `draw_cursor` argument determines whether the cursor is drawn in the screen shot; `bounds` let you specify the region, in screen coordinates, that you want copied. If `bounds` is `NULL`, the entire screen is copied. The functions fail if the `bounds` rectangle doesn't fall wholly within the screen's frame.

The functions return `B_OK` on success or `B_ERROR` on failure.

####  SetDesktopColor() , DesktopColor()

`void SetDesktopColor(rgb_color color,  
bool makeDefault = true);``rgb_color DesktopColor();`

These functions set and return the color of the desktop—the backdrop against which windows are displayed on the screen. `SetDesktopColor()` makes an immediate change in the desktop color displayed on-screen; `DesktopColor()` returns the color currently displayed.

If the `makeDefault` flag is `true`, the color that's set becomes the default color for the screen; it's the color that will be shown the next time the machine is booted. If the flag is `false`, the color is set only for the current session.

Note

![Note](./images/admonitions/Info_32.png)

The "Background Images" section tells you how to convince the desktop to display a bitmap image.

Typically, users choose the desktop color with the Screen preferences application.

####  SetDPMS() , DPMSState() , DPMSCapabilities()

`status_t SetDPMS(uint32 dpmsState);``uint32 DPMSState();``uint32 DPMSCapabilities();`

`SetDPMS()` lets you set the VESA Display Power Management Signaling state for the screen. The state can be one of the following values:

Constant| Description  
---|---  
`B_DPMS_ON`| Image is visible, normal screen operation.  
`B_DPMS_STAND_BY`| Image is not visible, but can be restored "instantly." Saves around 30% of the power used by the monitor in `B_DPMS_ON` mode.  
`B_DPMS_SUSPEND`| Image is not visible, but can be restored in less than five seconds. Saves more power by turning off the CRT's heater. The amount of savings (or if there's any) depends on the display.  
`B_DPMS_OFF`| Image is not visible and will take some time to restore. Typically turns off all monitor power except the processor watching the sync signals for a higher power state (typically `B_DPMS_ON`).  
  
`DPMSState()` returns the current display state, indicating whether the monitor is on or off or in one of the two sleep modes.

`DPMSCapabilities()` indicates which of the above modes the monitor supports.

#### SetToNext()

`status_t SetToNext();`

In the current BeOS release, this function always returns `B_ERROR`.

#### WaitForRetrace()

`status_t WaitForRetrace();` `status_t WaitForRetrace(bigtime_t timeout);`

Blocks until the monitor has finished the current vertical retrace, then returns `B_OK`. There are a few milliseconds available before it begins another retrace. Drawing changes made to the frame buffer in this period won't cause any "flicker" on-screen.

For some graphics card drivers, this function will wait for vertical sync; for others it will wait until vertical blank, providing a few extra milliseconds.

The `timeout` argument lets you provide a timeout in microseconds—if the screen hasn't retraced within the limit, the function returns `B_ERROR`.

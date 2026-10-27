<!-- Classes And Methods - The Interface Kit - Functions -->

## Functions

This section describes the global C functions defined in the Interface Kit. All these functions deal with aspects of the system-wide environment for the user interface—the screen, workspaces, installed fonts, the list of possible colors, and various user preferences. For mouse and keyboard functions, see "[Input Functions](TheInputServer_Functions.md "Functions")" in [The Input Server](TheInputServer.md "The Input Server").

In almost all cases, you need a valid (but not necessarily running) `be_app` object in order to call these functions.

* * *

###  activate_workspace(), current_workspace()

`void activate_workspace(int32 workspace);``int32 current_workspace();`

Declared in: `interface/InterfaceDefs.h`

These functions set and return the active workspace, the one that's currently displayed on-screen. The workspace is identified by an index, 0-based.

See also: [`BWindow::WorkspaceActivated()`](BWindow.md#BWindow_WorkspaceActivated "WorkspaceActivated\(\)")

* * *

### bitmaps_support_space()

`bool bitmaps_support_space(color_space space,  
uint32* supportFlags);`

Declared in: `interface/InterfaceDefs.h`

Returns `true` if [`BBitmap`](BBitmap.md "BBitmap")s can contain graphics in the specified color space, or `false` if not.

The uint32 pointed to by `supportFlags` will be set to a bit field of flags, further describing the support for the specified color space by the [`BBitmap`](BBitmap.md "BBitmap")s class. The flags are:

Constant| Description  
---|---  
`B_VIEWS_SUPPORT_DRAW_BITMAP`| [`BView::DrawBitmap()`](BView.md#BView_DrawBitmap "DrawBitmap\(\), DrawBitmapAsync\(\)") supports drawing [`BBitmap`](BBitmap.md "BBitmap")s of the specified color space.  
`B_BITMAPS_SUPPORT_ATTACHED_VIEWS`| Indicates that [`BBitmap`](BBitmap.md "BBitmap")s in the specified color space support attached [`BView`](BView.md "BView")s.  
  
* * *

### get_deskbar_frame()

`status_t get_deskbar_frame(color_space space,  
[`BRect`](BRect.md "BRect")* frame);`

Declared in: `interface/InterfaceDefs.h`

Writes the screen coordinates of the current frame of the Deskbar window into `frame`. Returns `B_OK` on success or an appropriate error code on failure.

* * *

###  get_font_family(), count_font_families(), get_font_style(), count_font_styles()

`status_t get_font_family(int32 index,  
font_family* family,  
uint32* flags = NULL);``int32 count_font_families();``status_t get_font_style(font_family family,  
int32 index,  
font_style* style,  
uint32* flags = NULL);``int32 count_font_styles(font_family family);`

Declared in: `interface/Font.h`

These functions are used in combination to get the names of the families and styles of all installed fonts. For example:
    
    
    int32 numFamilies = count_font_families();
    for ( int32 i = 0; i < numFamilies; i++ ) {
       font_family family;
       uint32 flags;
       if ( get_font_family(i, &family, &flags) == B_OK ) {
          . . .
          int32 numStyles = count_font_styles(family);
          for ( int32 j = 0; j < numStyles; j++ ) {
             font_style style;
             if ( get_font_style(family, j, &style, &flags) == B_OK ) {
                . . .
             }
          }
       }
    }

`get_font_family()` reads one family name from the list of installed fonts, the name at `index`, and copies it into the `family` buffer; `count_font_families()` returns the number of font families currently installed. Similarly, `get_font_style()` reads the name of the style at `index` and copies into the `style` buffer. Since each family can have a different set of styles, a family name must be passed to `get_font_style()`; `count_font_styles()` returns the number of styles for the particular `family`. Family and style names can be up to 64 bytes long including a null terminator. Indices begin at 0.

The names of installed font families and styles are not indexed in any particular order. You might want to alphabetize them before displaying them to the user in a menu or list.

If you pass a `flags` argument to `get_font_family()` and `get_font_style()`, they will place a mask with useful information about the particular family or style in the variable that the argument refers to. Currently there are just two flags:

Constant| Description  
---|---  
`B_IS_FIXED`| Indicates that the font is a nonproportional, or fixed-width, font—one for which all characters have the same width.  
`B_HAS_TUNED_FONT`| Indicates that the family or style has versions of the font especially adapted or "tuned" for on-screen display.  
  
If neither flag applies, the variable that `flags` points to will be set to 0.

If you find a family and style that has a tuned font, you can set a [`BFont`](BFont.md "BFont") object to that family and style, then call the object's [`GetTunedInfo()`](BFont.md#BFont_GetTunedInfo "GetTunedInfo\(\), CountTuned\(\), tuned_font_info") function to get details about exactly which combination of font properties (for example, which font sizes) have tuned counterparts. If you set a [`BFont`](BFont.md "BFont") so that it has those properties and make it a [`BView`](BView.md "BView")'s current font, the tuned version will be used when the [`BView`](BView.md "BView") draws to the screen.

It's possible for the user to install or remove fonts while the application is running. However, unless [`update_font_families()`](TheInterfaceKit_Functions.md#update_font_families "update_font_families\(\)") has been called to get the updated list, `get_font_family()` will provide information on the same set of fonts each time it's called. The list isn't automatically updated.

See also: [`update_font_families()`](TheInterfaceKit_Functions.md#update_font_families "update_font_families\(\)"), [`BView::SetFont()`](BView.md#BView_SetFont "SetFont\(\), GetFont\(\)"), [`BFont::SetFamilyAndStyle()`](BFont.md#BFont_SetFamilyAndStyle "SetFamilyAndStyle\(\), GetFamilyAndStyle\(\), FamilyAndStyle\(\), font_family, font_style")

* * *

### get_pixel_size_for()

`status_t get_pixel_size_for(color_space space,  
size_t* pixelChunk,  
size_t* rowAlignment,  
size_t* pixelsPerChunk);`

Declared in: `interface/GraphicsDefs.h`

Given the specified color space, returns information about pixel and row alignment for that color space.

* * *

### idle_time()

`bigtime_t idle_time();`

Declared in: `interface/InterfaceDefs.h`

Returns the number of microseconds since the user last manipulated the mouse or keyboard. This information isn't specific to a particular application; `idle_time()` tells you when the user last directed an action at any application, not just yours.

* * *

### keyboard_navigation_color()

Declared in: `interface/InterfaceDefs.h`

`rgb_color keyboard_navigation_color();`

Returns the color that should be used to mark the [`BView`](BView.md "BView") that's currently in focus, when the user can change the focus from the keyboard. The keyboard navigation color is typically used to underline the labels of control devices and to outline text fields where the user can type.

* * *

###  run_add_printer_panel(), run_select_printer_panel()

`void run_add_printer_panel();``void run_select_printer_panel();`

Declared in: `interface/InterfaceDefs.h`

These two functions have the Print Server place panels on-screen where the user can set up a printer and choose which printer to use. `run_add_printer_panel()` displays a panel that informs the server about a new printer. `run_select_printer_panel()` displays a panel that lists all known printers and lets the user select one.

See also: The [`BPrintJob`](BPrintJob.md "BPrintJob") class

* * *

###  set_menu_info(), get_menu_info()

`status_t set_menu_info(menu_info* info);``status_t get_menu_info(menu_info* info);`

Declared in: `interface/Menu.h`

These functions set and get the user's preferences for how menus should look and work. User's express their preferences with the Menu application, which calls `set_menu_info()`. `get_menu_info()` writes the current preferences into the menu_info structure that into refers to. This structure contains the following fields:

Field| Description  
---|---  
float `font_size`| The size of the font that will be used to display menu items.  
font_name `font`| The name of the font that's used to display menu items.  
rgb_color `background_color`| The background color of the menu.  
int32 `separator`| The style of horizontal line that separates groups of items in a menu. The value is an index ranging from 0 through 2; there are three possible separators.  
bool `click_to_open`| Whether it's possible to open a menu by clicking in the item that controls it. The default value is true.  
bool `triggers_always_shown`| Whether trigger characters are always marked in menus and menu bars, regardless of whether the menu hierarchy is the target for keyboard actions. The default value is false.  
  
(At present, both functions always return `B_OK`.)

See also: The [`BMenu`](BMenu.md "BMenu") class

* * *

### set_screen_space()

`status_t set_screen_space(int32 index,  
uint32 space,  
bool makeDefault = true);`

Declared in: `interface/InterfaceDefs.h`

Changes the configuration of the screen—its depth and dimensions—to match the values specified in the `space` constant, which can be any of the following:

  * `B_8_BIT_640x400`
  * `B_8_BIT_640x480`
  * `B_8_BIT_800x600`
  * `B_8_BIT_1024x768`
  * `B_8_BIT_1152x900`
  * `B_8_BIT_1280x1024`
  * `B_8_BIT_1600x1200`
  * `B_15_BIT_640x480`
  * `B_15_BIT_800x600`
  * `B_15_BIT_1024x768`
  * `B_15_BIT_1152x900`
  * `B_15_BIT_1280x1024`
  * `B_15_BIT_1600x1200`
  * `B_16_BIT_640x480`
  * `B_16_BIT_800x600`
  * `B_16_BIT_1024x768`
  * `B_16_BIT_1152x900`
  * `B_16_BIT_1280x1024`
  * `B_16_BIT_1600x1200`
  * `B_32_BIT_640x480`
  * `B_32_BIT_800x600`
  * `B_32_BIT_1024x768`
  * `B_32_BIT_1152x900`
  * `B_32_BIT_1280x1024`
  * `B_32_BIT_1600x1200`

The first part of the constant designates the screen depth and color space. `B_8_BIT…` refers to the `B_CMAP8` color space. The other values correspond to the `B_RGB15`, `B_RGB16`, and `B_RGB32` color spaces, as appropriate. Although constants are defined for 15-bit screen depths, the operating system currently doesn't support them. The second part of the constant designates the pixel resolution of the screen. For example, `B_32_BIT_1280x1024` means that the frame buffer is 32 bits deep (`B_RGB32`) while the screen grid is 1,280 pixels wide and 1,024 pixels high.

This function affects the screen at `index`. Since the BeOS currently doesn't support more than one screen, the only index that makes sense is 0.

The change to the screen takes effect immediately. If the `makeDefault` flag is `true`, the new configuration also becomes the default and will be used the next time the machine is turned on. If `makeDefault` is `false`, the configuration is in effect for the current session only.

Since not all configurations are possible for all graphics cards, `set_screen_space()` can fail. It returns `B_OK` if successful, and `B_ERROR` if not.

This function is designed for preferences applications—like the Screen application—that permit users to make system-wide choices about the screen. Other applications should respect those choices and refrain from modifying them.

The current screen configuration can be obtained from the [`BScreen`](BScreen.md "BScreen") object.

See also: [`BWindow::ScreenChanged()`](BWindow.md#BWindow_ScreenChanged "ScreenChanged\(\)"), [The Game Kit](TheGameKit.md "The Game Kit") chapter

* * *

###  set_scroll_bar_info(), get_scroll_bar_info()

`status_t set_scroll_bar_info(scroll_bar_info* info);``status_t get_scroll_bar_info(scroll_bar_info* info);`

Declared in: `interface/InterfaceDefs.h`

These functions set and report preferences that the [`BScrollBar`](BScrollBar.md "BScrollBar") class uses when it creates a new scroll bar. `set_scroll_bar_info()` reads the values contained in the scroll_bar_info structure that `info` refers to and sets the system-wide preferences accordingly; `get_scroll_bar_info()` writes the current preferences into the structure provided.

The scroll_bar_info structure contains the following fields:

Field| Description  
---|---  
bool `proportional`| `true` if scroll bars should have a knob that grows and shrinks to show what proportion of the document is currently visible on-screen, and `false` if not. Scroll knobs are proportional by default.  
bool `double_arrows`| `true` if a set of double arrows (for scrolling in both directions) should appear at each end of the scroll bar, or `false` if only single arrows (for scrolling in one direction only) should be used. Double arrows are the default.  
int32 `knob`| An index that picks the pattern for the knob. Only values of 0, 1, and 2 are currently valid. The patterns can be seen in the ScrollBar preferences application. The pattern at index 1 is the default.  
int32 `min_knob_size`| The length of the scroll knob, in pixels. This is the minimum size for a proportional knob and the fixed size for one that's not proportional. The default is 15.  
  
The user can set these preferences with the ScrollBar application. Applications can call `get_scroll_bar_info()` to find out what choices the user made, but should refrain from calling `set_scroll_bar_info()`. That function is desigined for utilities, like the ScrollBar application, that enable users to set preferences that are respected system-wide.

If successful, these functions return `B_OK`; if not, they return `B_ERROR`.

See also: The [`BScrollBar`](BScrollBar.md "BScrollBar") class

* * *

###  set_workspace_count(), count_workspaces()

`void set_workspace_count(int32 numWorkspaces);``int32 count_workspaces();`

Declared in: `interface/InterfaceDefs.h`

These functions set and return the number of workspaces the user has available. There can be as many as 32 workspaces and as few as 1. The choice of how many there should be is usually left to the user and the Workspaces application.

See also: [`activate_workspace()`](TheInterfaceKit_Functions.md#activate_workspace "activate_workspace\(\), current_workspace\(\)")

* * *

### system_colors()

`const color_map* system_colors();`

Declared in: `interface/InterfaceDefs.h`

Returns a pointer to the system color map. This function duplicates the [`BScreen`](BScreen.md "BScreen") [`ColorMap()`](BScreen.md#BScreen_ColorMap "ColorMap\(\), IndexForColor\(\), ColorForIndex\(\)") function, but it permits software that isn't concerned about the on-screen display to get the color map without referring to a particular screen. (Actually it returns the color map for the main screen.)

The color_map structure returned by this function belongs to the operating system.

* * *

### tint_color()

`rgb_color tint_color(rgb_color color,  
float tint);`

Declared in: `interface/InterfaceDefs.h`

`tint_color()` lightens or darkens the color `color` by the `tint` amount.

Tints less than 1.0 lighten the color and tints greater than 1.0 darken it.

Various `tint` constants are defined by the Interface Kit as seen [here](TheInterfaceKit_Functions.md#tint_color "tint_color\(\)").

* * *

### ui_color()

`rgb_color ui_color(color_which which);`

Declared in: `interface/InterfaceDefs.h`

Converts the supplied [color_which](TheInterfaceKit_GeneralConstants.md#color_which "color_which Constants") constant `which` to the corresponding [rgb_color](TheInterfaceKit_DefinedTypes.md#rgb_color "rgb_color") for the specified user interface element.

* * *

### update_font_families()

`bool update_font_families(bool update_font_families);`

Declared in: `interface/Font.h`

Updates the list of installed fonts, so that it reflects any that have been added or removed since the last time the list was updated. Until the list is updated, [`get_font_family()`](TheInterfaceKit_Functions.md#get_font_family "get_font_family\(\), count_font_families\(\), get_font_style\(\), count_font_styles\(\)") operates assuming the set of fonts that were installed when the application started up. If the list is unchanged since the last update (or since startup), this function returns `false`; if a font has been installed or an installed font has been removed, it returns `true`.

If the `checkOnly` flag is `true`, [`get_font_family()`](TheInterfaceKit_Functions.md#get_font_family "get_font_family\(\), count_font_families\(\), get_font_style\(\), count_font_styles\(\)") only reports whether the list has changed; it doesn't modify the current list. If the flag is `false`, it contacts the Application Server to get the updated list, a much more expensive operation.

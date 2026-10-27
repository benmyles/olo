<!-- Classes And Methods - The Interface Kit - General Constants -->

## General Constants

* * *

### alignment Constants

Declared in: `interface/InterfaceDefs.h`
    
    
    enum alignment {
        B_ALIGN_LEFT,
        B_ALIGN_RIGHT,
        B_ALIGN_CENTER

These constants define the alignment data type. They determine how lines of text and labels are aligned by [`BTextView`](BTextView.md "BTextView"), [`BStringView`](BStringView.md "BStringView"), and [`BMenuField`](BMenuField.md "BMenuField") objects.

See also: [`BTextView::SetAlignment()`](BTextView.md#BTextView_SetAlignment "SetAlignment\(\), Alignment\(\)")

* * *

### border_style Constants

Declared in: `interface/InterfaceDefs.h`
    
    
    enum border_style {
        B_PLAIN_BORDER,
        B_FANCY_BORDER,
        B_NO_BORDER

These constants define how [`BBox`](BBox.md "BBox") objects and [`BScrollView`](BScrollView.md "BScrollView")s are bordered.

* * *

### Character Constants

Declared in: `interface/InterfaceDefs.h`

Constant| Character value  
---|---  
`B_BACKSPACE`| 0x08 (same as 'b')  
`B_ENTER`| 0x0a (same as 'n')  
`B_RETURN`| 0x0a (synonym for `B_ENTER`)  
`B_SPACE`| 0x20 (same as ' ')  
`B_TAB`| 0x09 (same as 't')  
`B_ESCAPE`| 0x1b  
`B_LEFT_ARROW`| 0x1c  
`B_RIGHT_ARROW`| 0x1d  
`B_UP_ARROW`| 0x1e  
`B_DOWN_ARROW`| 0x1f  
`B_INSERT`| 0x05  
`B_DELETE`| 0x7f  
`B_HOME`| 0x01  
`B_END`| 0x04  
`B_PAGE_UP`| 0x0b  
`B_PAGE_DOWN`| 0x0c  
`B_FUNCTION_KEY`| 0x10  
`B_UTF8_ELLIPSIS`| "xE2x80xA6"  
`B_UTF8_OPEN_QUOTE`| "xE2x80x9C"  
`B_UTF8_CLOSE_QUOTE`| "xE2x80x9D"  
`B_UTF8_COPYRIGHT`| "xC2xA9"  
`B_UTF8_REGISTERED`| "xC2xAE"  
`B_UTF8_TRADEMARK`| "xE2x84xA2"  
`B_UTF8_SMILING_FACE`| "xE2x98xBB"  
`B_UTF8_HIROSHI`| "xE5xBCx98"  
  
Constants in the first group stand for the ASCII characters they name. They're defined only for characters that normally don't have visible symbols.

Constants in the second group are Unicode UTF-8 encodings of common characters that have multibyte representations. These constants are strings that can be concatenated with other strings—for example, to set a button label that ends in an ellipsis:
    
    
    myButton->SetLabel("Options"B_UTF_ELLIPSIS);

See also: "[Function Key Constants](TheInterfaceKit_GeneralConstants.md#Constants_Function_Key "Function Key Constants")" below

* * *

### color_space

Declared in: `interface/GraphicsDefs.h`

Constant| Description  
---|---  
`B_NO_COLOR_SPACE`| Undefined color space  
`B_RGB32`| xRGB 8:8:8:8, stored as little endian uint32  
`B_RGBA32`| ARGB 8:8:8:8, stored as little endian uint32  
`B_RGB24`| Currently unused  
`B_RGB16`| xRGB 5:6:5, stored as little endian uint16  
`B_RGB15`| RGB 1:5:5:5, stored as little endian uint16  
`B_RGBA15`| ARGB 1:5:5:5, stored as little endian uint16  
`B_CMAP8`| 256-color index into the color table.  
`B_GRAY8`| 256-color greyscale value.  
`B_GRAY1`| Each bit represents a single pixel, where 0 is the view's low color and 1 is the high color.  
`B_RGB32_BIG`| xRGB 8:8:8:8, stored as big endian uint32  
`B_RGBA32_BIG`| ARGB 8:8:8:8, stored as big endian uint32  
`B_RGB24_BIG`| Currently unused  
`B_RGB16_BIG`| RGB 5:6:5, stored as big endian uint16  
`B_RGB15_BIG`| xRGB 1:5:5:5, stored as big endian uint16  
`B_RGBA15_BIG`| ARGB 1:5:5:5, stored as big endian uint16  
`B_YCbCr422`| Currently unused.  
`B_YCbCr411`| Currently unused.  
`B_YCbCr444`| Currently unused.  
`B_YCbCr420`| Currently unused.  
`B_YUV422`| Currently unused.  
`B_YUV411`| Currently unused.  
`B_YUV420`| Currently unused.  
`B_YUV444`| Currently unused.  
`B_YUV9`| Currently unused.  
`B_YUV12`| Currently unused.  
`B_UVL24`| Currently unused.  
`B_UVL32`| Currently unused.  
`B_UVLA32`| Currently unused.  
`B_LAB24`| Currently unused.  
`B_LAB32`| Currently unused.  
`B_LABA32`| Currently unused.  
`B_HSI24`| Hue, saturation, and intensity: x:8:8:8 format  
`B_HSI32`| Hue, saturation, and intensity: 8:8:8 format  
`B_HSIA32`| Hue, saturation, intensity, and alpha: 8:8:8:8 format  
`B_HSV24`| Hue, saturation, and value: x:8:8:8 format  
`B_HSV32`| Hue, saturation, and value: 8:8:8 format  
`B_HSVA32`| Hue, saturation, value, and alpha: 8:8:8:8 format  
`B_HLS24`| Hue, lightness, and saturation: x:8:8:8 format  
`B_HLS32`| Hue, lightness, and saturation: 8:8:8 format  
`B_HLSA32`| Hue, lightness, saturation, and alpha: 8:8:8:8 format  
`B_CMY24`| Cyan, magenta, and yellow: x:8:8:8 format  
`B_CMY32`| Cyan, magenta, and yellow: 8:8:8 format  
`B_CMYA32`| Cyan, magenta, yellow, and alpha: 8:8:8:8 format  
`B_CMYK32`| Cyan, magenta, yellow, and black: 8:8:8:8 format  
`B_MONOCHROME_1_BIT`| For compatibility only: use B_GRAY1  
`B_GRAYSCALE_8_BIT`| For compatibility only; use B_GRAY8  
`B_COLOR_8_BIT`| For compatibility only; use B_CMAP8  
`B_RGB_32_BIT`| For compatibility only; use B_RGB32  
`B_RGB_16_BIT`| For compatibility only; use B_RGB15  
`B_BIG_RGB_32_BIT`| For compatibility only; use B_RGB32_BIG  
`B_BIG_RGB_16_BIT`| For compatibility only; use B_RGB15_BIG  
  
These constants define the color_space data type. A color space describes three properties of screens and bitmap images:

  * How many bits of information there are per pixel (the depth of the image)

  * How those bits are to be interpreted (whether as colors or on a grayscale, what the color components are, and so on).

  * How are components are arranged

A number of these aren't actually usable as screen display color spaces, but are intended for use in identifying the color format for video media buffers and other offscreen graphics data.

See the "[Color Spaces](TheInterfaceKit_Drawing.md#TheInterfaceKit_Drawing_Color_Spaces "Color Spaces")" section in the "[Drawing](TheInterfaceKit_Drawing.md "Drawing")" section of this chapter for a fuller explanation of these color spaces.

See also: the [`BBitmap`](BBitmap.md "BBitmap") and [`BScreen`](BScreen.md "BScreen") classes

* * *

### Control Values

Declared in: `interface/Control.h`

Constant| Value  
---|---  
`B_CONTROL_ON`| 1  
`B_CONTROL_OFF`| 0  
  
These constants define the bipolar states of a typical control device.

See also: [`BControl::SetValue()`](BControl.md#BControl_SetValue "SetValue\(\), Value\(\)")

* * *

### Cursor Transit Constants

Declared in: `interface/View.h`

Constant| Description  
---|---  
`B_ENTERED_VIEW`| The cursor has just entered a view.  
`B_INSIDE_VIEW`| The cursor has moved within the view.  
`B_EXITED_VIEW`| The cursor has left the view  
  
These constants describe the cursor's transit through a view. Each [`MouseMoved()`](BView.md#BView_MouseMoved "MouseMoved\(\)") notification includes one of these constants as an argument, to inform the [`BView`](BView.md "BView") whether the cursor has entered the view, moved while inside the view, or exited the view.

* * *

### Dead-Key Mapping

Declared in: `interface/InterfaceDefs.h`

Constant  
---  
`B_CONTROL_TABLE`  
`B_OPTION_CAPS_SHIFT_TABLE`  
`B_OPTION_CAPS_TABLE`  
`B_OPTION_SHIFT_TABLE`  
`B_OPTION_TABLE`  
`B_CAPS_SHIFT_TABLE`  
`B_CAPS_TABLE`  
`B_SHIFT_TABLE`  
`B_NORMAL_TABLE`  
  
These constants determine which combinations of modifiers can cause a key to be the "dead" member of a two-key combination.

See also: [`get_key_map()`](TheInputServer_Functions.md#get_key_map "get_key_map\(\)")

* * *

### drawing_mode Constants

Declared in: `interface/GraphicsDefs.h`
    
    
    enum drawing_mode {
        B_OP_COPY,
        B_OP_OVER,
        B_OP_ERASE,
        B_OP_INVERT,
        B_OP_SELECT,
        B_OP_ALPHA,
        B_OP_ADD,
        B_OP_SUBTRACT,
        B_OP_BLEND,
        B_OP_MIN,
        B_OP_MAX

These constants define the drawing_mode data type. The drawing mode is a [`BView`](BView.md "BView") graphics parameter that determines how the image being drawn interacts with the image already in place in the area where it's drawn. The various modes are explained under "[Drawing Modes](TheInterfaceKit_Drawing.md#TheInterfaceKit_Drawing_Drawing_Modes "Drawing Modes")" in the "[Drawing](TheInterfaceKit_Drawing.md "Drawing")" section of this chapter.

See also: [`BView::SetDrawingMode()`](BView.md#BView_SetDrawingMode "SetDrawingMode\(\), DrawingMode\(\)")

* * *

### Function Key Constants

Declared in: `interface/InterfaceDefs.h`

Constant  
---  
`B_F1_KEY`  
`B_F2_KEY`  
`B_F3_KEY`  
`B_F4_KEY`  
`B_F5_KEY`  
`B_F6_KEY`  
`B_F7_KEY`  
`B_F8_KEY`  
`B_F9_KEY`  
`B_F10_KEY`  
`B_F11_KEY`  
`B_F12_KEY`  
`B_PRINT_KEY` (the "Print Screen" key)  
`B_SCROLL_KEY` (the "Scroll Lock" key)  
`B_PAUSE_KEY`  
  
These constants stand for the various keys that are mapped to the `B_FUNCTION_KEY` character. When the `B_FUNCTION_KEY` character is reported in a key-down event, the application can determine which key produced the character by testing the key code against these constants. ( Control+p also produces the `B_FUNCTION_KEY` character.)

See also: "Character Mapping" in the [Keyboard Information](TheKeyboard.md "The Keyboard") appendix

* * *

### Interface Messages

Declared in: `app/AppDefs.h`

Constant  
---  
`B_ZOOM`  
`B_MINIMIZE`  
`B_WINDOW_RESIZED`  
`B_WINDOW_MOVED`  
`B_WINDOW_ACTIVATED`  
[`B_QUIT_REQUESTED`](TheApplicationKit_MessageConstants.md#B_QUIT_REQUESTED "B_QUIT_REQUESTED")  
`B_SCREEN_CHANGED`  
`B_WORKSPACE_ACTIVATED`  
`B_WORKSPACES_CHANGED`  
`B_KEY_DOWN`  
`B_KEY_UP`  
`B_MOUSE_DOWN`  
`B_MOUSE_UP`  
`B_MOUSE_MOVED`  
`B_VIEW_RESIZED`  
`B_VIEW_MOVED`  
`B_VALUE_CHANGED`  
`B_PULSE`  
  
These constants identify interface messages—system messages that are delivered to [`BWindow`](BWindow.md "BWindow") objects. Each constant conveys an instruction to do something in particular (`B_ZOOM`) or names a type of event (`B_KEY_DOWN`).

See also: "[Interface Messages](TheInterfaceKit_Responding.md#TheInterfaceKit_Responding_InterfaceMessages "Interface Messages")" in the "[Responding to the User](TheInterfaceKit_Responding.md "Responding to the User")" section of this chapter

* * *

### list_view_type Constants

Declared in: `interface/ListView.h`

Constant  
---  
`B_SINGLE_SELECTION_LIST`  
`B_MULTIPLE_SELECTION_LIST`  
  
These constants distinguish between lists that permit the user to select only one item at a time and those that allow multiple items to be selected.

See also: The [`BListView`](BListView.md "BListView") class

* * *

### Main Screen

Declared in: `interface/Screen.h`

const screen_id `B_MAIN_SCREEN_ID`

This constant stands for the main screen, the screen that has the origin of the screen coordinate system at its left top corner. (Currently only one screen can be attached to the computer and it is the main screen.)

See also: [screen_id](TheInterfaceKit_DefinedTypes.md#screen_id "screen_id")

* * *

### menu_bar_border Constants

Declared in: `interface/MenuBar.h`

Constant| Description  
---|---  
`B_BORDER_FRAME`| Put a border inside the entire frame rectangle.  
`B_BORDER_CONTENTS`| Put a border around the group of items only.  
`B_BORDER_EACH_ITEM`| Put a border around each item.  
  
These constants can be passed as an argument to [`BMenuBar`](BMenuBar.md "BMenuBar")'s [`SetBorder()`](BMenuBar.md#BMenuBar_SetBorder "SetBorder\(\), Border\(\)") function.

* * *

### menu_layout Constants

Declared in: `interface/Menu.h`

Constant| Description  
---|---  
`B_ITEMS_IN_ROW`| Menu items are arranged horizontally, in a row.  
`B_ITEMS_IN_COLUMN`| Menu items are arranged vertically, in a column.  
`B_ITEMS_IN_MATRIX`| Menu items are arranged in a custom fashion.  
  
These constants define the menu_layout data type. They distinguish the ways that items can be arranged in a menu or menu bar—they can be laid out from end to end in a row like a typical menu bar, stacked from top to bottom in a column like a typical menu, or arranged in some custom fashion like a matrix.

See also: The [`BMenu`](BMenu.md "BMenu") and [`BMenuBar`](BMenuBar.md "BMenuBar") constructors

* * *

### Modifier States

Declared in: `interface/InterfaceDef.h`

Constant  
---  
`B_SHIFT_KEY`  
`B_LEFT_SHIFT_KEY`  
`B_RIGHT_SHIFT_KEY`  
`B_CONTROL_KEY`  
`B_LEFT_CONTROL_KEY`  
`B_RIGHT_CONTROL_KEY`  
`B_CAPS_LOCK`  
`B_SCROLL_LOCK`  
`B_NUM_LOCK`  
`B_OPTION_KEY`  
`B_LEFT_OPTION_KEY`  
`B_RIGHT_OPTION_KEY`  
`B_COMMAND_KEY`  
`B_LEFT_COMMAND_KEY`  
`B_RIGHT_COMMAND_KEY`  
`B_MENU_KEYB_PULSE`  
  
These constants designate the Shift, Option, Control, Command, and Menu modifier keys and the lock states set by the Caps Lock, Scroll Lock, and Num Lock keys. They're typically used to form a mask that describes the current, or required, modifier states.

For each variety of modifier key, there are constants that distinguish between the keys that appear at the left and right of the keyboard, as well as one that lumps both together. For example, if the user is holding the left Control key down, both `B_CONTROL_KEY` and `B_LEFT_CONTROL_KEY` will be set in the mask.

See also: [`modifiers()`](TheInputServer_Functions.md#modifiers "modifiers\(\)"), [`BWindow::AddShortcut()`](BWindow.md#BWindow_AddShortcut "AddShortcut\(\), RemoveShortcut\(\)"), the [`BMenu`](BMenu.md "BMenu") constructor

* * *

### Mouse Buttons

Declared in: `interface/View.h`

Constant  
---  
`B_PRIMARY_MOUSE_BUTTON`  
`B_SECONDARY_MOUSE_BUTTON`  
`B_TERTIARY_MOUSE_BUTTON`  
  
These constants name the mouse buttons. Buttons are identified, not by their physical positions on the mouse, but by their roles in the user interface.

See also: [`BView::GetMouse()`](BView.md#BView_GetMouse "GetMouse\(\)"), [`set_mouse_map()`](TheInputServer_Functions.md#set_mouse_map "set_mouse_map\(\), get_mouse_map\(\), set_mouse_type\(\), get_mouse_type\(\) , set_mouse_speed\(\), get_mouse_speed\(\)")

* * *

### Orientation Constants

Declared in: `interface/InterfaceDef.h`

Constant  
---  
`B_HORIZONTAL`  
`B_VERTICAL`  
  
These constants define the orientation data type that distinguishes between the vertical and horizontal orientation of graphic objects. It's currently used only to differentiate scroll bars.

See also: The [`BScrollBar`](BScrollBar.md "BScrollBar") and [`BScrollView`](BScrollView.md "BScrollView") classes

* * *

### Pattern Constants

Declared in: `interface/GraphicsDef.h`
    
    
    const pattern B_SOLID_HIGH =
        { 0xff, 0xff, 0xff, 0xff, 0xff,0xff, 0xff, 0xff }
    
    const pattern B_SOLID_LOW =
        { 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00 }
    
    const pattern B_MIXED_COLORS =
        { 0xaa, 0x55, 0xaa, 0x55, 0xaa, 0x55, 0xaa, 0x55 }

These constants name the three standard patterns defined in the Interface Kit.

`B_SOLID_HIGH` is a pattern that consists of the high color only. It's the default pattern for all BView drawing functions that stroke lines and fill shapes.

`B_SOLID_LOW` is a pattern with only the low color. It's used mainly to erase images (to replace them with the background color).

`B_MIXED_COLORS` alternates pixels between the high and low colors in a checkerboard pattern. The result is a halftone midway between the two colors. This pattern can produce fine gradations of color, especially when the high and low colors are set to two colors that are already quite similar.

See also: "[Patterns](TheInterfaceKit_Drawing.md#TheInterfaceKit_Drawing_Patterns "Patterns")" in the "[Drawing](TheInterfaceKit_Drawing.md "Drawing")" section of this chapter

* * *

### Resizing Modes

Declared in: `interface/View.h`

Constant  
---  
`B_FOLLOW_LEFT`  
`B_FOLLOW_RIGHT`  
`B_FOLLOW_LEFT_RIGHT`  
`B_FOLLOW_H_CENTER`  
`B_FOLLOW_TOP`  
`B_FOLLOW_BOTTOM`  
`B_FOLLOW_TOP_BOTTOM`  
`B_FOLLOW_V_CENTER`  
`B_FOLLOW_ALL`  
`B_FOLLOW_NONE`  
  
These constants are used to set the behavior of a view when its parent is resized. They're explained under the `BView` [constructor](BView.md#BView_Constructor "BView\(\)").

See also: [`BView::SetResizingMode()`](BView.md#BView_SetResizingMode "SetResizingMode\(\), ResizingMode\(\)")

* * *

### Screen Spaces

Declared in: `interface/GraphicsDef.h`
    
    
    B_8_BIT_640x400
    B_8_BIT_640x480    B_15_BIT_640x480
    B_16_BIT_640x480   B_32_BIT_640x480
    
    B_8_BIT_800x600    B_15_BIT_800x600
    B_16_BIT_800x600   B_32_BIT_800x600
    
    B_8_BIT_1024x768   B_15_BIT_1024x768
    B_16_BIT_1024x768  B_32_BIT_1024x768
    
    B_8_BIT_1152x900   B_15_BIT_1152x900
    B_16_BIT_1152x900  B_32_BIT_1152x900
    
    B_8_BIT_1280x1024  B_15_BIT_1280x1024
    B_16_BIT_1280x1024 B_32_BIT_1280x1024
    
    B_8_BIT_1600x1200  B_15_BIT_1600x1200
    B_16_BIT_1600x1200 B_32_BIT_1600x1200

These constants are currently used to configure the screen—to set its depth and the size of the pixel grid it displays—as well as to report which configurations are possible. However, they may not be supported in the future. 15-bit depths are not currently supported

See also: [`set_screen_space()`](TheInterfaceKit_Functions.md#set_screen_space "set_screen_space\(\)"), `get_screen_info()`.

* * *

### ScrollBar Constants

Declared in: `interface/ScrollBar.h`

Constant  
---  
`B_H_SCROLL_BAR_HEIGHT`  
`B_V_SCROLL_BAR_WIDTH`  
  
These constants record the recommended thickness of scroll bars. They should be used to help define the frame rectangles passed to the `BScrollBar` [constructor](BScrollBar.md#BScrollBar_Constructor "BScrollBar\(\)").

* * *

### String Truncation Constants

Declared in: `interface/Font.h`

Constant  
---  
`B_TRUNCATE_END`  
`B_TRUNCATE_BEGINNING`  
`B_TRUNCATE_MIDDLE`  
`B_TRUNCATE_SMART`  
  
These constants instruct a [`BFont`](BFont.md "BFont") where it should remove characters from a set of strings to shorten them.

See also: [`BFont::GetTruncatedStrings()`](BFont.md#BFont_GetTruncatedStrings "GetTruncatedStrings\(\), TruncateString\(\)")

* * *

### tint_color() Constants

Declared in: `interface/InterfaceDefs.h`

Constant| Description  
---|---  
`B_LIGHTEN_MAX_TINT`| The maximum amount of tinting lighter. A color component value of 216 will become 255.  
`B_LIGHTEN_2_TINT`| The second largest amount of lightening. A color component value of 216 will become 240.  
`B_LIGHTEN_1_TINT`| A small amount of lightening. A color component value of 216 will become 232.  
`B_NO_TINT`| Has no effect on the color.  
`B_DARKEN_1_TINT`| The least amount of darkening. A color component value of 184 will become zero.  
`B_DARKEN_2_TINT`| Darker. A color component value of 216 will become 152.  
`B_DARKEN_3_TINT`| Even darker. A color component value of 216 will become 128.  
`B_DARKEN_4_TINT`| Darker still A color component value of 216 will become 96.  
`B_DARKEN_MAX_TINT`| Wow, that's really dark. A color component value of 216 will become zero.  
`B_DISABLED_LABEL_TINT`| Use this constant when generating colors with which to disable text labels.  
`B_HIGHLIGHT_BACKGROUND_TINT`| Use this constant to create a dark color for use as a background color on selected data.  
`B_DISABLED_MARK_TINT`| Use this to create a color for a disabled checkmark or radio button mark (or other tick-mark type items).  
  
These constants are used when calling [`tint_color()`](TheInterfaceKit_Functions.md#tint_color "tint_color\(\)") to alter an existing color, especially for creating disabled objects or highlighting backgrounds.

* * *

### Transparency Constants

Declared in: `interface/GraphicsDefs.h`
    
    
    const uint8 B_TRANSPARENT_8_BIT
    const rgb_color B_TRANSPARENT_32_BIT

These constants set transparent pixel values in a bitmap image. `B_TRANSPARENT_8_BIT` designates a transparent pixel in the `B_CMAP8` color space, and `B_TRANSPARENT_32_BIT` designates a transparent pixel in the `B_RGB32` color space.

Transparency is explained in the "Drawing Modes" part of the "Drawing" section of this chapter. Drawing modes other than B_OP_COPY preserve the destination image where a source bitmap is transparent.

See also: "[Drawing Modes](TheInterfaceKit_Drawing.md#TheInterfaceKit_Drawing_Drawing_Modes "Drawing Modes")" in the "[Drawing](TheInterfaceKit_Drawing.md "Drawing")" section of this chapter, the [`BBitmap`](BBitmap.md "BBitmap") class, [`BView::SetViewColor()`](BView.md#BView_SetViewColor "SetViewColor\(\), ViewColor\(\)")

* * *

### color_which Constants

Declared in: `interface/InterfaceDefs.h`

Constant| Description  
---|---  
`B_PANEL_BACKGROUND_COLOR`| The background color used for view panels.  
`B_MENU_BACKGROUND_COLOR`| The background color used for menus.  
`B_MENU_SELECTION_BACKGROUND_COLOR`| The color used for highlighting menu items when selected.  
`B_MENU_ITEM_TEXT_COLOR`| The color used for drawing menu item labels when not selected.  
`B_MENU_SELECTED_ITEM_TEXT_COLOR`| The color used for drawing menu item labels when selected.  
`B_WINDOW_TAB_COLOR`| The color used to draw the window's title tab.  
`B_KEYBOARD_NAVIGATION_COLOR`| The color used to draw the mark indicating which control is focused for keyboard navigation.  
`B_DESKTOP_COLOR`| The current desktop color.  
  
When calling [`ui_color()`](TheInterfaceKit_Functions.md#ui_color "ui_color\(\)") to obtain the color for a user interface element, these constants are used to select the desired element.

* * *

### View Flags

Declared in: `interface/View.h`

Constant| Description  
---|---  
`B_FULL_UPDATE_ON_RESIZE`| Include the entire view in the clipping region.  
`B_WILL_DRAW`| Allow the `BView` to draw.  
`B_PULSE_NEEDED`| Report pulse events to the `BView`.  
`B_FRAME_EVENTS`| Report view-resized and view-moved events.  
`B_NAVIGABLE`| Let users navigate to the view with the Tab key.  
`B_NAVIGABLE_JUMP`| Mark the view for Control+Tab navigation.  
  
These constants can be combined to form a mask that sets the behavior of a [`BView`](BView.md "BView") object. They're explained in more detail under the class constructor. The mask is passed to the [constructor](BView.md#BView_Constructor "BView\(\)"), or to the [`SetFlags()`](BView.md#BView_SetFlags "SetFlags\(\) , Flags\(\)") function.

* * *

### Window Flags

Declared in: `interface/Window.h`

Constant  
---  
`B_NOT_MOVABLE B_NOT_CLOSABLE`  
`B_NOT_H_RESIZABLE`  
`B_NOT_ZOOMABLE`  
`B_NOT_V_RESIZABLE`  
`B_NOT_MINIMIZABLE`  
`B_NOT_RESIZABLE`  
`B_WILL_FLOAT`  
`B_WILL_ACCEPT_FIRST_CLICK`  
  
These constants set the behavior of a window. They can be combined to form a mask that's passed to the `BWindow` [constructor](BWindow.md#BWindow_Constructor "BWindow\(\)").

* * *

### window_type Constants

Declared in: `interface/Window.h`

Constant| Description  
---|---  
`B_MODAL_WINDOW`| The window is a modal window.  
`B_BORDERED_WINDOW`| The window has a border but no title tab.  
`B_TITLED_WINDOW`| The window has a border and a title tab.  
`B_DOCUMENT_WINDOW`| The window has a border, tab and resize knob.  
  
These constants describe the various kinds of windows that can be requested from the Application Server.

See also: The `BWindow` [constructor](BWindow.md#BWindow_Constructor "BWindow\(\)")

* * *

### Workspace Constants

Declared in: `interface/Window.h`

Constant  
---  
`B_CURRENT_WORKSPACE`  
`B_ALL_WORKSPACES`  
  
These constants are used—along with designations of specific workspaces—to associate a set of one or more workspaces with a [`BWindow`](BWindow.md "BWindow").

See also: The `BWindow` [constructor](BWindow.md#BWindow_Constructor "BWindow\(\)"), [`BWindow::SetWorkspaces()`](BWindow.md#BWindow_SetWorkspaces "SetWorkspaces\(\), Workspaces\(\)")

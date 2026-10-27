<!-- Classes And Methods - The Interface Kit - Defined Types -->

## Defined Types

* * *

### button_width

Declared in: `interface/Alert.h`
    
    
    enum button_width {
        B_WIDTH_AS_USUAL,
        B_WIDTH_FROM_LABEL,
        B_WIDTH_FROM_WIDEST

These constants name the methods that can be used to determine how wide to make the buttons in an alert panel.

See also: [button_width Constants](TheInterfaceKit_GeneralConstants.md#alignment "alignment Constants"), the `BAlert` [constructor](BAlert.md#BAlert_Constructor "BAlert\(\)")

* * *

### color_map

Declared in: `interface/GraphicsDefs.h`
    
    
    typedef struct {
        int32 id;
        rgb_color color_list[256];
        uint8 inversion_map[256];
        uint8 index_map[32768];
    } color_map

This structure contains information about the 8-bit color context for a particular screen. All applications that display on the screen share the same color map.

See also: [`BScreen::ColorMap()`](BScreen.md#BScreen_ColorMap "ColorMap\(\), IndexForColor\(\), ColorForIndex\(\)")

* * *

### edge_info

Declared in: `interface/Font.h`
    
    
    typedef struct {
        float left;
        float right;
    } edge_info

This structure records information about the location of a character outline within the horizontal space allotted to the character. Edges separate one character from adjacent characters on the left and right. They're explained under the [`GetEdges()`](BFont.md#BFont_GetEdges) function in the [`BFont`](BFont.md "BFont") class.

See also: [`BFont::GetEscapements()`](BFont.md#BFont_GetEscapements "GetEscapements\(\), escapement_delta, GetEdges\(\), edge_info")

* * *

### escapement_delta

Declared in: `interface/Font.h`
    
    
    typedef struct {
        float nonspace;
        float space;
    } escapement_delta

This structure contains values that should be added to the width of each character in a string when the string is drawn.

See also: [`BView::DrawString(),`](BView.md#BView_DrawString "DrawString\(\)") [`BFont::GetEscapements()`](BFont.md#BFont_GetEscapements "GetEscapements\(\), escapement_delta, GetEdges\(\), edge_info")

* * *

### font_family

Declared in: `interface/InterfaceDefs.h`
    
    
    typedef char font_family[B_FONT_FAMILY_LENGTH + 1]

This type defines a string long enough to hold the name of a font family—64 characters including the null terminator.

See also: [`get_font_family()`](TheInterfaceKit_Functions.md#get_font_family "get_font_family\(\), count_font_families\(\), get_font_style\(\), count_font_styles\(\)")

* * *

### font_height

Declared in: `interface/Font.h`
    
    
    typedef struct {
        float ascent;
        float descent;
        float leading;
    } font_height

This type combines the three vertical measurements that determine the height of a line of text.

See also: [`BFont::GetHeight()`](BFont.md#BFont_GetHeight "GetHeight\(\), font_height")

* * *

### font_style

Declared in: `interface/InterfaceDefs.h`
    
    
    typedef char font_style[B_FONT_STYLE_LENGTH + 1]

This type defines a string long enough to hold the name of a font style—64 characters including the null terminator.

See also: [`get_font_style()`](TheInterfaceKit_Functions.md#get_font_style)

* * *

### key_info

Declared in: `interface/View.h`
    
    
    typedef struct {
        uint32 modifiers;
        uint8 key_states[16];
    } key_info

This structure is used to get information about the current state of the keyboard in the absence of `B_KEY_DOWN` messages.

See also: [`get_key_info()`](TheInputServer_Functions.md#get_key_info "get_key_info\(\)")

* * *

### key_map

Declared in: `interface/InterfaceDefs.h`
    
    
    typedef struct {
        uint32 version;
        uint32 caps_key;
        uint32 scroll_key;
        uint32 num_key;
        uint32 left_shift_key;
        uint32 right_shift_key;
        uint32 left_command_key;
        uint32 right_command_key;
        uint32 left_control_key;
        uint32 right_control_key;
        uint32 left_option_key;
        uint32 right_option_key;
        uint32 menu_key;
        uint32 lock_settings;
        int32 control_map[128];
        int32 option_caps_shift_map[128];
        int32 option_caps_map[128];
        int32 option_shift_map[128];
        int32 option_map[128];
        int32 caps_shift_map[128];
        int32 caps_map[128];
        int32 shift_map[128];
        int32 normal_map[128];
        int32 acute_dead_key[32];
        int32 grave_dead_key[32];
        int32 circumflex_dead_key[32];
        int32 dieresis_dead_key[32];
        int32 tilde_dead_key[32];
        uint32 acute_tables;
        uint32 grave_tables;
        uint32 circumflex_tables;
        uint32 dieresis_tables;
        uint32 tilde_tables;
    } key_map

This structure maps the physical keys on the keyboard to their functions in the user interface. It holds the tables that assign characters to key codes, set up dead keys, and determine which keys function as modifiers. There's just one key map shared by all applications running on the same machine.

See also: [`get_key_map()`](TheInputServer_Functions.md#get_key_map "get_key_map\(\)")

* * *

### menu_bar_border

Declared in: `interface/MenuBar.h`
    
    
    typedef enum {. . .} menu_bar_border

This type enumerates the ways that a menu bar can be bordered.

See also: [`BMenuBar::SetBorder()`](BMenuBar.md#BMenuBar_SetBorder "SetBorder\(\), Border\(\)"), "[menu_bar_border Constants](TheInterfaceKit_DefinedTypes.md#menu_bar_border "menu_bar_border")" above

* * *

### menu_info

Declared in: `interface/Menu.h`
    
    
    typedef struct {
        float font_size;
        font_name font;
        rgb_color background_color;
        int32 separator;
        bool click_to_open;
        bool triggers_always_shown;
    } menu_info

This structure records the user's menu preferences.

See also: [`set_menu_info()`](TheInterfaceKit_Functions.md#set_menu_info "set_menu_info\(\), get_menu_info\(\)"), the [`BMenu`](BMenu.md "BMenu") class

* * *

### menu_layout

Declared in: `interface/Menu.h`
    
    
    typedef enum {. . .} menu_layout

This type distinguishes the various ways that items can arranged in a menu or menu bar.

See also: The [`BMenu`](BMenu.md "BMenu") class, "[menu_layout Constants](TheInterfaceKit_DefinedTypes.md#menu_layout "menu_layout")" above

* * *

### mouse_map

Declared in: `interface/InterfaceDefs.h`
    
    
    typedef struct {
        uint32 left;
        uint32 right;
        uint32 middle;
    } mouse_map

This structure maps mouse buttons to their roles as the `B_PRIMARY_MOUSE_BUTTON`, `B_SECONDARY_MOUSE_BUTTON`, or `B_TERTIARY_MOUSE_BUTTON`.

See also: [`set_mouse_map()`](TheInputServer_Functions.md#set_mouse_map "set_mouse_map\(\), get_mouse_map\(\), set_mouse_type\(\), get_mouse_type\(\) , set_mouse_speed\(\), get_mouse_speed\(\)")

* * *

### orientation

Declared in: `interface/InterfaceDefs.h`
    
    
    typedef enum {. . .} orientation

This type distinguishes between the `B_VERTICAL` and `B_HORIZONTAL` orientation of scroll bars.

See also: The [`BScrollBar`](BScrollBar.md "BScrollBar") and [`BScrollView`](BScrollView.md "BScrollView") classes

* * *

### pattern

Declared in: `interface/GraphicsDefs.h`
    
    
    typedef struct {
        uchar data[8];
    } pattern

A pattern is a arrangement of two colors—the high color and the low color—in an 8-pixel by 8-pixel square. Pixels are specified in rows, with one byte per row and one bit per pixel. Bits marked 1 designate the high color; those marked 0 designate the low color. An example and an illustration are given under "Patterns" in the "Drawing section of this chapter.

See also: "[Pattern Constants](TheInterfaceKit_GeneralConstants.md#Constants_Pattern "Pattern Constants")" above

* * *

### print_file_header

Declared in: `interface/PrintJob.h`
    
    
    typedef struct {
        long version;
        long page_count;
        long _reserved_1_;
        long _reserved_2_;
        long _reserved_3_;
        long _reserved_4_;
        long _reserved_5_;
    } print_file_header

This structure defines the header information for a print job. Although declared publicly, it currently is used only internally by the [`BPrintJob`](BPrintJob.md "BPrintJob") class.

* * *

### rgb_color

Declared in: `interface/GraphicsDefs.h`
    
    
    typedef struct {
        uint8 red;
        uint8 green;
        uint8 blue;
        uint8 alpha;
    } rgb_color

This type specifies a full 32-bit color. Each component can have a value ranging from a minimum of 0 to a maximum of 255.

The alpha component, which is designed to specify the coverage of the color (how transparent or opaque it is), is currently ignored. However, an rgb_color can be made completely transparent by assigning it the special value, `B_TRANSPARENT_32_BIT`.

See also: [`BView::SetHighColor()`](BView.md#BView_SetHighColor "SetHighColor\(\), HighColor\(\) , SetLowColor\(\) , LowColor\(\)")

* * *

### screen_id

Declared in: `interface/Screen.h`
    
    
    typedef struct {
        int32 id;
    } screen_id

This type is a unique identifier for a screen. The constant `B_MAIN_SCREEN_ID` is a screen_id that identifies the main screen.

See also: The [`BScreen`](BScreen.md "BScreen") class

* * *

### screen_info

Declared in: `interface/InterfaceDefs.h`
    
    
    typedef struct {
        color_space mode;
        BRect frame;
        uint32 spaces;
        float min_refresh_rate;
        float max_refresh_rate;
        float refresh_rate;
        uchar h_position;
        uchar v_position;
        uchar h_size;
        uchar v_size;
    } screen_info

This structure holds information about a screen. Its fields are explained under the `get_screen_info()` global function.

* * *

### scroll_bar_info

Declared in: `interface/InterfaceDefs.h`
    
    
    typedef struct {
        bool proportional;
        bool double_arrows;
        int32 knob;
        int32 min_knob_size;
    } scroll_bar_info

This structure captures the user's preferences for how scroll bars should behave and appear.

See also: [`set_scroll_bar_info()`](TheInterfaceKit_Functions.md#set_scroll_bar_info "set_scroll_bar_info\(\), get_scroll_bar_info\(\)"), the [`BScrollBar`](BScrollBar.md "BScrollBar") class

* * *

### text_run

Declared in: `interface/TextView.h`
    
    
    typedef struct {
        int32 offset;
        BFont font;
        rgb_color color;
    } text_run

The [`BTextView`](BTextView.md "BTextView") class uses this structure to keep track of a sequence of characters that are displayed in the same font and color.

See also: [`BTextView::SetRunArray()`](BTextView.md#BTextView_SetRunArray "SetRunArray\(\), RunArray\(\)")

* * *

### text_run_array

Declared in: `interface/TextView.h`
    
    
    typedef struct {
        int32 count;
        text_run runs[];
    } text_run_array

The [`BTextView`](BTextView.md "BTextView") class uses this structure to associate character formats with the text it displays.

See also: [`BTextView::SetRunArray()`](BTextView.md#BTextView_SetRunArray "SetRunArray\(\), RunArray\(\)")

* * *

### tuned_font_info

Declared in: `interface/Font.h`
    
    
    typedef struct {
        float size;
        float shear;
        float rotation;
        int32 flags;
        int16 face;
    } tuned_font_info

This structure lists the properties of a font that has a counterpart that's been tuned to improve its appearance on-screen. The tuned counterpart will be used whenever the font is the [`BView`](BView.md "BView")'s current font and [`DrawString()`](BView.md#BView_DrawString "DrawString\(\)") draws to the screen.

See also: [`BFont::GetTunedInfo()`](BFont.md#BFont_GetTunedInfo "GetTunedInfo\(\), CountTuned\(\), tuned_font_info"), [`get_font_family()`](TheInterfaceKit_Functions.md#get_font_family "get_font_family\(\), count_font_families\(\), get_font_style\(\), count_font_styles\(\)")

* * *

### window_type

Declared in: `interface/Window.h`
    
    
    typedef enum {. . .} window_type

This type describes the various kinds of windows that can be requested from the Application Server.

See also: The [`BWindow`](BWindow.md "BWindow") constructor, "[window_type Constants](TheInterfaceKit_DefinedTypes.md#window_type "window_type")"

<!-- Special Topics - The Keyboard - Character Constants -->

## Character Constants

The Interface Kit defines constants for characters that aren't normally represented by a visible symbol. This includes the usual space and backspace characters, but most invisible characters are produced by the function keys and the navigation keys located between the main keyboard and the numeric keypad. The character values associated with these keys are more or less arbitrary, so you should always use the constant in your code rather than the actual character value. Many of these characters are also produced by alphabetic keys when a Control key is held down.

The table below lists character constants defined in the kit and the keys they're associated with.

Key label| Key code| Character reported  
---|---|---  
Backspace| 0x1e| `B_BACKSPACE`  
Tab| 0x26| `B_TAB`  
Enter| 0x47| `B_ENTER`  
space bar| 0x5e| `B_SPACE`  
Escape| 0x01| `B_ESCAPE`  
F1–F12 0x02 through| 0x0d| `B_FUNCTION_KEY`  
Print Screen| 0x0e| `B_FUNCTION_KEY`  
Scroll Lock| 0x0f| `B_FUNCTION_KEY`  
Pause| 0x10| `B_FUNCTION_KEY`  
System Request| 0x7e| 0xc8  
Break| 0x7f| 0xca  
Insert| 0x1f| `B_INSERT`  
Home| 0x20| `B_HOME`  
Page Up| 0x21| `B_PAGE_UP`  
Delete| 0x34| `B_DELETE`  
End| 0x35| `B_END`  
Page Down| 0x36| `B_PAGE_DOWN`  
up arrow| 0x57| `B_UP_ARROW`  
left arrow| 0x61| `B_LEFT_ARROW`  
down arrow| 0x62| `B_DOWN_ARROW`  
right arrow| 0x63| `B_RIGHT_ARROW`  
  
Several keys are mapped to the `B_FUNCTION_KEY` character. An application can determine which function key was pressed to produce the character by testing the key code against these constants:

`B_F1_KEY`| `B_F6_KEY`| `B_F11_KEY`  
---|---|---  
`B_F2_KEY`| `B_F7_KEY`| `B_F12_KEY`  
`B_F3_KEY`| `B_F8_KEY`| `B_PRINT_KEY` (the "Print Screen" key)  
`B_F4_KEY`| `B_F9_KEY`| `B_SCROLL_KEY` (the "Scroll Lock" key)  
`B_F5_KEY`| `B_F10_KEY`| `B_PAUSE_KEY`  
  
Note that key 0x30 P is also mapped to `B_FUNCTION_KEY` when the Control key is held down.

Each of the character constants listed above is a one-byte value falling in the range of values where ASCII and Unicode™ overlap. For convenience, the Interface Kit also defines some constants for common characters that fall outside that range. These characters have multibyte representations in UTF-8, so the constant is defined as a character string. For example:
    
    
    #define B_UTF8_OPEN_QUOTE  "xE2x80x9C"
    #define B_UTF8_CLOSE_QUOTE "xE2x80x9D"
    #define B_UTF8_COPYRIGHT   "xC2xA9"

See "[Character Constants](TheInterfaceKit_GeneralConstants.md#Constants_Character "Character Constants")" in the "[General Constants](TheInterfaceKit_GeneralConstants.md "General Constants")" section of The Interface Kit for a full list of these constants.

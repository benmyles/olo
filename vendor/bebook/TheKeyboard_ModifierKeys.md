<!-- Special Topics - The Keyboard - Modifier Keys -->

## Modifier Keys  
  
Conceptually, there are eight modifier keys. The global [`modifiers()`](TheInputServer_Functions.md#modifiers "modifiers\(\)") function returns a value that encodes the current status of all eight of these keys. You can test for the state of a particular modifier key by comparing [`modifiers()`](TheInputServer_Functions.md#modifiers "modifiers\(\)") with the constants listed in the table below. For example, here's how you can see if the user is holding down one of the Shift keys:
    
    
    if (modifiers() | B_SHIFT_KEY) {
       /* a shift key is down */
    }

The modifier mask is also included in the "modifiers" field in [`BMessage`](BMessage.md "BMessage")s that report keyboard and mouse events.

How your application uses modifier keys is up to you. The list below lists the eight keys and how they're typically used.

Constant| Description  
---|---  
`B_SHIFT_KEY`| Maps character keys to their uppercase or alternative versions.  
`B_COMMAND_KEY`| In combination with a character key, performs a keyboard shortcut.  
`B_CONTROL_KEY`| Turns alphabetic keys into control characters.  
`B_OPTION_KEY`| Maps keys to alternative characters, typically characters in an extended set—multibyte UTF-8 characters.  
`B_MENU_KEY`| Pops open a menu.  
`B_CAPS_LOCK`| Reverses the effect of the Shift key on alphabetic characters.  
`B_NUM_LOCK`| Reverses the effect of the Shift key on numeric characters.  
`B_SCROLL_LOCK`| Stops the display from scrolling.  
  
If you're interested in whether the left or right modifer key of a given type was pressed, you can check against these additional values:
    
    
    B_LEFT_SHIFT_KEY   B_RIGHT_SHIFT_KEY
    B_LEFT_CONTROL_KEY B_RIGHT_CONTROL_KEY
    B_LEFT_OPTION_KEY  B_RIGHT_OPTION_KEY
    B_LEFT_COMMAND_KEY B_RIGHT_COMMAND_KEY

Most of the modifiers map to single, specific keys: Shift and Caps Lock are examples. But others don't: the Menu key maps to _`Shortcut`_ +Escape (where _`Shortcut`_ is either Alt, Command, or Control, depending on the user's preferences and keyboard).

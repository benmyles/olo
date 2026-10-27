<!-- Special Topics - The Keyboard - Keyboard Messages -->

## Keyboard Messages

* * *

### B_KEY_DOWN, B_KEY_UP, B_UNMAPPED_KEY_DOWN, B_UNMAPPED_KEY_UP

Source:| The system.  
---|---  
Target:| The focus view's [`BWindow`](BWindow.md "BWindow").  
Hook:| [`BView::KeyDown()`](BView.md#BView_KeyDown "KeyDown\(\)") (`B_KEY_DOWN`)  
[`BView::KeyUp()`](BView.md#BView_KeyUp "KeyUp\(\)") (`B_KEY_UP`)  
(The `…UNMAPPED…` messages don't map to hook functions.)  
  
`B_KEY_DOWN` is sent when the user presses (or holds down) a key that's mapped to a character; `B_KEY_UP` is sent when the user releases the key. `B_UNMAPPED_KEY_DOWN` and `B_UNMAPPED_KEY_UP` are sent if the key isn't mapped to a character. This doesn't include modifier keys, which are reported in the `B_MODIFIERS_CHANGED` message.

Field| Type code| Description  
---|---|---  
`when`| `B_INT64_TYPE`| Event time, in microseconds since 01/01/70  
`key`| `B_INT32_TYPE`| The code for the physical key that was pressed. See "[More On Keyboard Mapping](TheKeyboard_MoreOnKeyboardMapping.md "More On Keyboard Mapping")" for a discussion of the keymap.  
`be:key_repeat`| `B_INT32_TYPE`| The "iteration number" of this key down. When the user holds the key down, successive messages are sent with increasing key repeat values. This field isn't present in the initial event; the first repeat message (i.e., the second key down message) has a key repeat value of 1. (`B_KEY_DOWN` only)  
`modifiers`| `B_INT32_TYPE`| The modifier keys that were in effect at the time of the event. See "[Modifier Keys](TheKeyboard_ModifierKeys.md "Modifier Keys")" for a list of values.  
`states`| `B_UINT8_TYPE`| The state of all keys at the time of the event. See "[Key States](TheKeyboard_KeyStates.md "Key States")."  
`byte`[3]| `B_INT8_TYPE`| The UTF-8 data that's generated. (`B_KEY_DOWN` and `B_KEY_UP` only)  
`bytes`| `B_STRING_TYPE`| The string that's generated. (The string usually contains a single character.) (`B_KEY_DOWN` and `B_KEY_UP` only)  
`raw_char`| `B_INT32_TYPE`| Modifier-independent ASCII code for the character. (`B_KEY_DOWN` and `B_KEY_UP` only)

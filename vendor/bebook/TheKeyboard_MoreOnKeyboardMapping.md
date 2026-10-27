<!-- Special Topics - The Keyboard - More On Keyboard Mapping -->

## More On Keyboard Mapping

The key map records character values using the UTF-8 encoding of the Unicode Standard, making it possible to map keys to characters in any of the world's scripts. UTF-8 encodes 16-bit Unicode values in a variable number of bytes (from one to four). The main benefit to UTF-8 is that one-byte UTF characters $00 through $7F are identical to the ASCII standard that's been around for decades.

A `B_KEY_DOWN` message holds the character mapped to the key the user pressed as an array of bytes named, simply, `byte`. The array is passed as a string to the [`KeyDown()`](BView.md#BView_KeyDown "KeyDown\(\)") hook function along with a count of the number of bytes in the string:
    
    
    virtual void KeyDown(const char* bytes, int32 numBytes)

See "[Character Encoding](TheInterfaceKit_Character_Encoding.md "Character Encoding")" in the Interface Kit chapter for a description of UTF-8 encoding and [`get_key_map()`](TheInputServer_Functions.md#get_key_map "get_key_map\(\)") for an explanation of the key map.

Most keys are mapped to more than one character. The precise character that the key produces depends on which modifier keys are being held down and which lock states the keyboard is in at the time the key is pressed.

A few examples are given in the table below:

Key| No modifiers| Shift alone| Option alone| Shift + Option| Control  
---|---|---|---|---|---  
0x15| 4| $|  |  | 4  
0x18| 7| &| ¦| ¤| 7  
0x26| `B_TAB`| `B_TAB`| `B_TAB`| `B_TAB`| `B_TAB`  
0x2e| i| I|  |  | `B_TAB`  
0x40| g| G| "|  | 0x07  
0x43| k| K|  |  | `B_PAGE_UP`  
0x51| n| N| ñ| Ñ| 0x0e  
0x55| /| ?| ¸| À| /  
0x64| `B_INSERT`| 0| `B_INSERT`| 0| `B_INSERT`  
  
The mapping follows some fixed rules, including these:

  * If a Command key is held down, the Control keys are ignored. Command trumps Control. Otherwise, Command doesn't affect the character that's reported for the key. If only Command is held down, the character that's reported is the same as if no modifiers were down; if Command and Option are held down, the character that's reported is the same as for Option alone; and so on.

  * If a Control key is held down (without a Command key), Shift, Option, and all keyboard locks are ignored. Control trumps the other modifiers (except for Command).

  * Num Lock applies only to keys on the numerical keypad. While this lock is on, the effect of the Shift key is inverted. Num Lock alone yields the same character that's produced when a Shift key is down (and Num Lock is off). Num Lock plus Shift yields the same character that's produced without either Shift or the lock.

  * Menu and Scroll Lock play no role in determining how keys are mapped to characters.

The default key map also follows the conventional rules for Caps Lock and Control:

  * Caps Lock applies only to the 26 alphabetic keys on the main keyboard. It serves to map the key to the same character as Shift. Using Shift while the lock is on undoes the effect of the lock; the character that's reported is the same as if neither Shift nor Caps Lock applied. For example, Shift+g and Caps Lock+g both are mapped to uppercase 'G', but Shift+Caps Lock+g is mapped to lowercase 'g'.

  * However, if the lock doesn't affect the character, Shift plus the lock is the same as Shift alone. For example, Caps Lock+7 produces '7' (the lock is ignored) and Shift+7 produces '&' (Shift has an effect), so Shift+Caps Lock+7 also produces '&' (only Shift has an effect).

  * When Control is used with a key that otherwise produces an alphabetic character, the character that's reported has a value 0x40 less than the value of the uppercase version of the character (0x60 less than the lowercase version of the character). This often results in a character that is produced independently by another key. For example, Control+i produces the `B_TAB` character and Control+l produces `B_PAGE_DOWN`.

  * When Control is used with a key that doesn't produce an alphabetic character, the character that's reported is the same as if no modifiers were on. For example, Control+7 produces a '7'.

<!-- Classes And Methods - The Input Server - Functions -->

## Functions

Declared In:| `interface/InterfaceDefs.h`  
---|---  
Library:| `libbe.so`  
  
This section describes the global mouse and keyboard functions.

* * *

### Mouse Functions

####  set_click_speed(), get_click_speed()

Declared in: `interface/InterfaceDefs.h`

`status_t set_click_speed(bigtime_t interval);``status_t get_click_speed(bigtime_t* interval);`

These functions set and report the timing for multiple-clicks. For successive mouse-down events to count as a multiple-click, they must occur within the interval set by `set_click_speed()` and provided by `get_click_speed()`. The `interval` is measured in microseconds; it's usually set by the user in the Mouse preferences application. The smallest possible interval is 100,000 microseconds (0.1 second).

If successful, these functions return `B_OK`; if unsuccessful, they return an error code, which may be just `B_ERROR`.

####  set_mouse_map(), get_mouse_map(), set_mouse_type(), get_mouse_type() , set_mouse_speed(), get_mouse_speed()

Declared in: `interface/InterfaceDefs.h`

`status_t set_mouse_map(mouse_map map);``status_t get_mouse_map(mouse_map* map);``status_t set_mouse_type( numButtons);``status_t get_mouse_type(int32* numButtons);``status_t set_mouse_speed(int32 speed);``status_t get_mouse_speed(int32* speed);``status_t set_mouse_acceleration(int32 acceleration);``status_t get_mouse_acceleration(int32* acceleration);`

These functions configure the mouse and supply information about the current configuration. The configuration should usually be left to the user and the Mouse preferences application.

`set_mouse_map()` maps the buttons of the mouse to their roles in the user interface, and `get_mouse_map()` writes the current map into the variable referred to by `map`. The mouse_map structure has a field for each button on a three-button mouse:

Field| Description  
---|---  
uint32 `left`| The button on the left of the mouse  
uint32 `right`| The button on the right of the mouse  
uint32 `middle`| The button in the middle, between the other two buttons  
  
Each field is set to one of the following constants:

  * `B_PRIMARY_MOUSE_BUTTON`
  * `B_SECONDARY_MOUSE_BUTTON`
  * `B_TERTIARY_MOUSE_BUTTON`

The same role can be assigned to more than one physical button. If all three buttons are set to `B_PRIMARY_MOUSE_BUTTON`, they all function as the primary button; if two of them are set to `B_SECONDARY_MOUSE_BUTTON`, they both function as the secondary button; and so on.

`set_mouse_type()` informs the system of how many buttons the mouse actually has. If it has two buttons, only the left and right fields of the mouse_map are operative. If it has just one button, only the left field is operative. `set_mouse_type()` writes the current number of buttons into the variable referred to by `numButtons`.

`set_mouse_speed()` sets the speed of the mouse—the rate at which the cursor image moves on-screen relative to the actual speed at which the user moves the mouse on its pad. A speed value of 0 is the slowest movement rate. The maximum rate is 20, though even 10 is too fast for most users. `get_mouse_speed()` writes the current speed into the variable referred to by `speed`.

`set_mouse_acceleration()` sets the mouse's acceleration—the rate at which the cursor image gains and loses speed as the user begins and ceases moving the mouse. An acceleration value of 0 is the slowest movement rate. The maximum rate is 20, though even 10 is too fast for most users. `get_mouse_acceleration()` writes the current acceleration into the variable referred to by `acceleration`.

All six functions return `B_OK` if successful, and an error code, typically `B_ERROR`, if not.

* * *

### Keyboard Functions

#### get_key_info()

Declared in: `interface/InterfaceDefs.h`

`status_t get_key_info(key_info* keyInfo);`

Writes information about the state of the keyboard into the key_info structure referred to by `keyInfo`. This function lets you get information about the keyboard in the absence of `B_KEY_DOWN` messages. The key_info structure has just two fields:

Field| Description  
---|---  
uint32 `modifiers`| A mask indicating which modifier keys are down and which keyboard locks are on.  
uint8 `key_states` [16]| A bit array that records the state of all the keys on the keyboard, and all the keyboard locks. This array works identically to the "states" array passed in a key-down message. See "[Key States](TheKeyboard_KeyStates.md "Key States")" in the [The Keyboard](TheKeyboard.md "The Keyboard") chapter for information on how to read information from the array.  
  
`get_key_info()` returns `B_OK` if it was able to get the requested information, and `B_ERROR` if the return results are unreliable.

See also: [`BView::KeyDown()`](BView.md#BView_KeyDown "KeyDown\(\)"), [`modifiers()`](TheInputServer_Functions.md#modifiers "modifiers\(\)")

#### get_key_map()

`void get_key_map(key_map** keys,  
char** chars);`

Declared in: `interface/InterfaceDefs.h`

Provides a pointer to a copy of the system key map—the structure that describes the role of each key on the keyboard. The pointers returned by the function are yours; you must `free()` them when you're finished with them.

Note

![Note](./images/admonitions/Info_32.png)

In versions of the BeOS before Release 4, the pointers used to belong to the operating system. Now they're yours to do with as you please. Please update your applications as necessary to avoid leaking memory.

Through the Keymap preferences application, users can configure the keyboard to their liking. The user's preferences are stored in a file (`Key_map` within the `B_USER_SETTINGS_DIRECTORY`, returned by the [`find_directory()`](BDirectory.md#find_directory "find_directory\(\)") function). When the machine reboots, the key map is read from this file. If the file doesn't exist, the original map encoded in the Application Server is used.

The key_map structure contains a large number of fields, but it can be broken down into these six parts:

  1. A version number.

  2. A series of fields that determine which keys will function as modifier keys—such as Shift, Control, or Num Lock.

  3. A field that sets the initial state of the keyboard locks in the default key map.

  4. A series of ordered tables that assign character values to keys. Except for a handful of modifier keys, all keys are mapped to characters, though they may not be mapped for all modifier combinations.

  5. A series of tables that locate the dead keys for diacritical marks and determine how a combination of a dead key plus another key is mapped to a particular character.

  6. A set of masks that determine which modifier keys are required for a key to be considered dead.

The following sections describe the parts of the key_map structure.

##### Version

The first field of the key map is a version number:

uint32 `version`| An internal identifier for the key map.  
---|---  
  
The version number doesn't change when the user configures the keyboard, and shouldn't be changed programmatically either. You can ignore it.

##### Modifiers

Modifier keys set states that affect other user actions on the keyboard and mouse. Eight modifier states are defined—Shift, Control, Option, Command, Menu, Caps Lock, Num Lock, and Scroll Lock. These states are discussed under "[Modifier Keys](TheKeyboard_ModifierKeys.md "Modifier Keys")" in the [Keyboard Information](TheKeyboard.md "The Keyboard") appendix. They fairly closely match the key caps found on a Macintosh keyboard, but only partially match those on a standard PC keyboard—which generally has a set of Alt(ernate) keys, rarely Option keys, and only sometimes Command and Menu keys. Because of these differences, the mapping of keys to modifiers is the area of the key map most open to the user's personal judgement and taste, and consequently to changes in the default configuration.

Since two keys, one on the left and one on the right, can be mapped to the Shift, Control, Option, and Command modifiers, the keyboard can have as many as twelve modifier keys. The key_map structure has one field for each key:

Field| Description  
---|---  
uint32 `caps_key`| The key that functions as the Caps Lock key; by default, this is the key labeled "Caps Lock," key 0x3b.  
uint32 `scroll_key`| The key that functions as the Scroll Lock key; by default, this is the key labeled "Scroll Lock," key 0x0f.  
uint32 `num_key`| The key that functions as the Num Lock key; by default, this is the key labeled "Num Lock," key 0x22.  
uint32 `left_shift_key`| A key that functions as a Shift key; by default, this is the key on the left labeled "Shift," key 0x4b.  
uint32 `right_shift_key`| Another key that functions as a Shift key; by default, this is the key on the right labeled "Shift," key 0x56.  
uint32 `left_command_key`| A key that functions as a Command key; by default, this is key 0x5d, sometimes labeled "Alt."  
uint32 `right_command_key`| Another key that functions as a Command key; by default, this is key 0x5f, sometimes labeled "Alt."  
uint32 `left_control_key`| A key that functions as a Control key; by default, this is the key labeled "Control" on the left, key 0x5c.  
uint32 `right_control_key`| Another key that functions as a Control key; by default on keyboards that have Option keys, this key is the key labeled "Control" on the right, key 0x60. For keyboards that don't have Option keys, this field is unmapped (its value is 0); key 0x60 is used as an Option key.  
uint32 `left_option_key`| A key that functions as an Option key; by default, this is key 0x66, which has different labels on different keyboards—"Option," "Command," or a Windows symbol. This key doesn't exist on, and therefore isn't mapped for, a standard 101-key keyboard.  
uint32 `right_option_key`| A key that functions as an Option key; by default, this is key 0x67, which has different labels on different keyboards—"Option," "Command," or a Windows symbol. For keyboards without this key, the field is mapped to the key labeled "Control" on the right, key 0x60.  
uint32 `menu_key`| A key that initiates keyboard navigation of the menu hierarchy; by default, this is the key labeled with a menu symbol, key 0x68. This key doesn't exist on, and therefore isn't mapped for, a standard 101-key keyboard.  
  
Each field names the key that functions as that modifier. For example, when the user holds down the key whose code is set in the `right_option_key` field, the `B_OPTION_KEY` and `B_RIGHT_OPTION_KEY` bits are turned on in the modifiers mask that the [`modifiers()`](TheInputServer_Functions.md#modifiers "modifiers\(\)") function returns. When the user then strikes a character key, the `B_OPTION_KEY` state influences the character that's generated.

If a modifier field is set to a value that doesn't correspond to an actual key on the keyboard (including 0), that field is not mapped. No key fills that particular modifier role.

##### Keyboard locks

One field of the key map sets initial modifier states:

Field| Description  
---|---  
uint32 `lock_settings`| A mask that determines which keyboard locks are turned on when the machine reboots or when the default key map is restored.  
  
The mask can be 0 or may contain any combination of these three constants:

  * `B_CAPS_LOCK`
  * `B_SCROLL_LOCK`
  * `B_NUM_LOCK`

It's 0 by default; there are no initial locks.

Altering the `lock_settings` field has no effect unless the altered key map is made the default.

##### Character Maps

The principal job of the key map is to assign character values to keys. This is done in a series of nine tables:

Field| Description  
---|---  
uint32 `control_map` [128]| The characters that are produced when a Control key is down but both Command keys are up.  
uint32 `option_caps_shift_map` [128]| The characters that are produced when Caps Lock is on and both a Shift key and an Option key are down.  
uint32 `option_caps_map` [128]| The characters that are produced when Caps Lock is on and an Option key is down.  
uint32 `option_shift_map` [128]| The characters that are produced when both a Shift key and an Option key are down.  
uint32 `option_map` [128]| The characters that are produced when an Option key is down.  
uint32 `caps_shift_map` [128]| The characters that are produced when Caps Lock is on and a Shift key is down.  
uint32 `caps_map` [128]| The characters that are produced when Caps Lock is on.  
uint32 `shift_map` [128]| The characters that are produced when a Shift key is down.  
uint32 `normal_map` [128]| The characters that are produced when none of the other tables apply.  
  
Each of these tables is an array of 128 offsets into another array, the chars array of Unicode UTF-8 character encodings. `get_key_map()` provides a pointer to the chars array as its second argument.

Key codes are used as indices into the character tables. The offset stored at any particular index maps a character to that key. For example, the code assigned to the M key is 0x52; at index 0x52 in the `option_caps_map` is an offset; at that offset in the chars array, you'll find the character that's mapped to the M key when an Option key is held down and Caps Lock is on.

This indirection—an index to an offset to a character—is required because characters are encoded as Unicode UTF-8 strings. Character values of 127 or less (7-bit ASCII) are just a single byte, but UTF-8 takes two, three, or (rarely) four bytes to encode values over 127.

The chars array represents each character as a Pascal string—the first byte in the string tells how many other bytes the string contains. For example, the string for the trademark symbol (™) looks like this:
    
    
    x03xE2x84xA2

The first byte (x03) indicates that Unicode UTF-8 takes 3 bytes to represent the trademark symbol, and those bytes follow (xE2x84xA2). Pascal strings are not null-terminated.

Here's an example showing you how to decode the character tables. This sample prints out a simple chart of the `normal_map`, `shift_map`, `option_map`, and `option_shift_map` characters:
    
    
    #include <interface/InterfaceDefs.h>
    #include <stdio.h>
    #include <string.h>
    #include <stdlib.h>
    
    static void print_key( char *chars, int32 offset )
    {
       int size = chars[offset++];
    
       switch( size ) {
       case 0:
          // Not mapped
          printf( "N/A" );
          break;
    
       case 1:
          // 1-byte UTF-8/ASCII character
          printf( "%c", chars[offset] );
          break;
    
       default:
          // 2-, 3-, or 4-byte UTF-8 character
          {
             char str[5];
             int i = (size <= 4) ? size : 4;
             strncpy( str, &(chars[offset]), i );
             str[i] = '0';
             printf( "%s", str );
          }
          break;
       }
    
       printf( "\t" );
    }
    
    int main( void )
    {
       // Get the current key map.
       key_map *keys;
       char *chars;
       get_key_map( &keys, &chars );
    
       // Print a chart of the normal, shift, option, and option+shift
       // keys.
       printf( "Key #tNormaltShifttOptiontOption+Shiftn" );
       for( int idx = 0; idx < 128; idx++ ) {
          printf( " %3dt", idx );
          print_key( chars, keys->normal_map[idx] );
          print_key( chars, keys->shift_map[idx] );
          print_key( chars, keys->option_map[idx] );
          print_key( chars, keys->option_shift_map[idx] );
          printf( "\n" );
       }
    
       // Free our copy of the key map.
       free( chars );
       free( keys );
    
       return EXIT_SUCCESS;
    }

The character map tables are ordered. Values from the first applicable table are used, even if another table might also seem to apply. For example, if Caps Lock is on and a Control key is down (and both Command keys are up), the `control_map` array is used, not `caps_map`. If a Shift key is down and Caps Lock is on, the `caps_shift_map` is used, not `shift_map` or `caps_map`.

Notice that the last eight tables (all except `control_map`) are paired, with a table that names the Shift key (`…_shift_map`) preceding an equivalent table without Shift:

  * `option_caps_shift_map` is paired with `option_caps_map`,

  * `option_shift_map` with `option_map`,

  * `caps_shift_map` with `caps_map`, and

  * `shift_map` with `normal_map`.

These pairings are important for a special rule that applies to keys on the numerical keypad when Num Lock is on:

  * If the Shift key is down, the non-Shift table is used.

  * However, if the Shift key is not down, the Shift table is used.

In other words, Num Lock inverts the Shift and non-Shift tables for keys on the numerical keypad.

Not every key needs to be mapped to a character. If the chars array has a 0-length string for a key, the key is not mapped to a character (given the particular modifier states the table represents). Generally, modifier keys are not mapped to characters, but all other keys are, at least for some tables. Key-down events are not generated for unmapped keys.

##### Dead keys

Next are the tables that map combinations of keys to single characters. The first key in the combination is "dead"—it doesn't produce a key-down event until the user strikes another character key. When the user hits the second key, one of two things will happen: If the second key is one that can be used in combination with the dead key, a single key-down event reports the combination character. If the second key doesn't combine with the dead key, two key-down events occur, one reporting the dead-key character and one reporting the second character.

There are five dead-key tables:

Field| Description  
---|---  
int32 `acute_dead_key` [32]| The table for combining an acute accent (´) with other characters.  
int32 `grave_dead_key` [32]| The table for combining a grave accent (`) with other characters.  
int32 `circumflex_dead_key` [32]| The table for combining a circumflex (^) with other characters.  
int32 `dieresis_dead_key` [32]| The table for combining a dieresis (¨) with other characters.  
int32 `tilde_dead_key` [32]| The table for combining a tilde (~) with other characters  
  
The tables are named after diacritical marks that can be placed on more than one character. However, the name is just a mnemonic; it means nothing. The contents of the table determine what the dead key is and how it combines with other characters. It would be possible, for example, to remap the `tilde_dead_key` table so that it had nothing to do with a tilde.

Each table consists of a series of up to 16 offset pairs—where, as in the case of the character maps, each offset picks a character from the chars character array. The first character in the pair is the one that must be typed immediately after the dead key. The second character is the resulting character, the character that's produced by the combination of the dead key plus the first character in the pair. For example, if the first character is 'o', the second might be '^'—meaning that the combination of a dead key plus the character 'o' produces a circumflexed 'ô'.

The character pairs for the default `grave_dead_key` array look something like this:
    
    
    ' ', ''',
    'A', 'À',
    'E', 'È',
    'I', 'Ì',
    'O', 'Ò',
    'U', 'Ù',
    'a', 'à',
    'e', 'è',
    'i', 'ì''o', 'ò',
    'u', 'ù',
    . . .

By convention, the first offset in each array is to the `B_SPACE` character and the second is to the dead-key character itself. This pair does double duty: It states that the dead key plus a space yields the dead-key character, and it also names the dead key. The system understands what the dead key is from the second offset in the array.

##### Character Tables For Dead Keys

As mentioned above, for a key to be dead, it must be mapped to the character picked by the second offset in a dead-key array. However, it's not typical for every key that's mapped to the character to be dead. Usually, there's a requirement that the user must hold down certain modifier keys (often the Option key). In other words, a key is dead only if selected character-map tables map it to the requisite character.

Five additional fields of the key_map structure specify what those character-map tables are—which modifiers are required for each of the dead keys:

Field| Description  
---|---  
uint32 `acute_tables`| The character tables that cause a key to be dead when they map it to the second character in the acute_dead_key array.  
uint32 `grave_tables`| The character tables that cause a key to be dead when they map it to the second character in the grave_dead_key array.  
uint32 `circumflex_tables`| The character tables that cause a key to be dead when they map it to the second character in the circumflex_dead_key array.  
uint32 `dieresis_tables`| The character tables that cause a key to be dead when they map it to the second character in the dieresis_dead_key array.  
uint32 `tilde_tables`| The character tables that cause a key to be dead when they map it to the second character in the tilde_dead_key array.  
  
Each of these fields contains a mask formed from the following constants:

  * `B_CONTROL_TABLE`
  * `B_CAPS_SHIFT_TABLE`
  * `B_OPTION_CAPS_SHIFT_TABLE`
  * `B_CAPS_TABLE`
  * `B_OPTION_CAPS_TABLE`
  * `B_SHIFT_TABLE`
  * `B_OPTION_SHIFT_TABLE`
  * `B_NORMAL_TABLE`
  * `B_OPTION_TABLE`

The mask designates the character-map tables that permit a key to be dead. For example, if the mask for the `grave_tables` field is,
    
    
    B_OPTION_TABLE | B_OPTION_CAPS_SHIFT_TABLE

a key would be dead whenever either of those tables mapped the key to the character of the second offset in the `grave_dead_key` array ('Q' in the example above). A key mapped to the same character by another table would not be dead.

See also: [`get_key_info()`](TheInputServer_Functions.md#get_key_info "get_key_info\(\)"), [`modifiers()`](TheInputServer_Functions.md#modifiers "modifiers\(\)"), the Keyboard Information appendix, [`set_modifier_key()`](TheInputServer_Functions.md#set_modifier_key "set_modifier_key\(\)")

#### get_keyboard_id()

`status_t get_keyboard_id(uint16* id);`

Declared in: `interface/InterfaceDefs.h`

Obtains the keyboard identifier from the Application Server and device driver and writes it into the variable referred to by `id`. This number reveals what kind of keyboard is currently attached to the computer.

The identifier for the standard 101-key PC keyboard—and for keyboards with a similar set of keys—is 0x83ab.

If unsuccessful for any reason, `get_keyboard_id()` returns `B_ERROR`. If successful, it returns `B_OK`.

#### modifiers()

`uint32 modifiers();`

Declared in: `interface/InterfaceDefs.h`

Returns a mask that has a bit set for each modifier key the user is holding down and for each keyboard lock that's set. The mask can be tested against these constants:

  * `B_SHIFT_KEY`
  * `B_COMMAND_KEY`
  * `B_CAPS_LOCK`
  * `B_CONTROL_KEY`
  * `B_MENU_KEY`
  * `B_SCROLL_LOCK`
  * `B_OPTION_KEY`
  * `B_NUM_LOCK`

No bits are set (the mask is 0) if no locks are on and none of the modifiers keys are down.

If it's important to know which physical key the user is holding down, the one on the right or the one on the left, the mask can be further tested against these constants:

  * `B_LEFT_SHIFT_KEY`
  * `B_RIGHT_SHIFT_KEY`
  * `B_LEFT_CONTROL_KEY`
  * `B_RIGHT_CONTROL_KEY`
  * `B_LEFT_OPTION_KEY`
  * `B_RIGHT_OPTION_KEY`
  * `B_LEFT_COMMAND_KEY`
  * `B_RIGHT_COMMAND_KEY`

By default, the keys closest to the space bar function as Command keys, no matter what their labels on particular keyboards. If a keyboard doesn't have Option keys (for example, a standard 101-key keyboard), the key on the right labeled & quot;Control" functions as the right Option key, and only the left "Control" key is available to function as a Control modifier. However, users can change this configuration with the /bin/keymap application.

####  set_key_repeat_rate(), get_key_repeat_rate(), set_key_repeat_delay(), get_key_repeat_delay()

`status_t set_key_repeat_rate(int32 rate);``status_t get_key_repeat_rate(int32* rate);``status_t set_key_repeat_delay(bigtime_t delay);``status_t get_key_repeat_delay(bigtime_t* delay);`

Declared in: `interface/InterfaceDefs.h`

These functions set and report the timing of repeating keys. When the user presses a character key on the keyboard, it produces an immediate `B_KEY_DOWN` message. If the user continues to hold the key down, it will, after an initial delay, continue to produce messages at regularly spaced intervals—until the user releases the key or presses another key. The delay and the spacing between messages are both preferences the user can set with the Keyboard application.

`set_key_repeat_rate()` sets the number of messages repeating keys produce per second. For a standard PC keyboard, the rate can be as low as 2 and as high as 30; `get_key_repeat_rate()` writes the current setting into the integer that rate refers to.

`set_key_repeat_delay()` sets the length of the initial delay before the key begins repeating. Acceptable values are 250,000, 500,000, 750,000 and 1,000,000 microseconds (.25, .5, .75, and 1.0 second); `get_key_repeat_delay()` writes the current setting into the variable that delay points to.

All four functions return `B_OK` if they successfully communicate with the Application Server, and `B_ERROR` if not. It's possible for the `set…()` functions to communicate with the server but not succeed in setting the rate or delay (for example, if the delay isn't one of the listed four values).

#### set_keyboard_locks()

`void set_keyboard_locks(uint32 modifiers);`

Declared in: `interface/InterfaceDefs.h`

Turns the keyboard locks—Caps Lock, Num Lock, and Scroll Lock—on and off. The keyboard locks that are listed in the modifiers mask passed as an argument are turned on; those not listed are turned off. The mask can be 0 (to turn off all locks) or it can contain any combination of the following constants:

  * `B_CAPS_LOCK`
  * `B_NUM_LOCK`
  * `B_SCROLL_LOCK`

See also: [`get_key_map()`](TheInputServer_Functions.md#get_key_map "get_key_map\(\)"), [`modifiers()`](TheInputServer_Functions.md#modifiers "modifiers\(\)")

#### set_modifier_key()

`void set_modifier_key(uint32 modifier,  
uint32 key);`

Declared in: `interface/InterfaceDefs.h`

Maps a modifier role to a particular key on the keyboard, where key is a key identifier and modifier is one of the these constants:

  * `B_CAPS_LOCK`
  * `B_LEFT_SHIFT_KEY`
  * `B_RIGHT_SHIFT_KEY`
  * `B_NUM_LOCK`
  * `B_LEFT_CONTROL_KEY`
  * `B_RIGHT_CONTROL_KEY`
  * `B_SCROLL_LOCK`
  * `B_LEFT_OPTION_KEY`
  * `B_RIGHT_OPTION_KEY`
  * `B_MENU_KEY`
  * `B_LEFT_COMMAND_KEY`
  * `B_RIGHT_COMMAND_KEY`

The key in question serves as the named modifier key, unmapping any key that previously played that role. The change remains in effect until the default key map is restored. In general, the user's preferences for modifier keys—expressed in the Keymap application—should be respected.

Modifier keys can also be mapped by calling [`get_key_map()`](TheInputServer_Functions.md#get_key_map "get_key_map\(\)") and altering the [key_map](TheInterfaceKit_DefinedTypes.md#key_map "key_map") structure directly. This function is merely a convenient alternative for accomplishing the same thing. (It's currently not possible to alter the key map; [`get_key_map()`](TheInputServer_Functions.md#get_key_map "get_key_map\(\)") looks at a copy.)

#### instantiate_input_device()

Note

![Note](./images/admonitions/Info_32.png)

TODO: This function does not appear in the BeBook

#### instantiate_input_filter()

Note

![Note](./images/admonitions/Info_32.png)

TODO: This function does not appear in the BeBook

<!-- Classes And Methods - The Support Kit - Functions and Macros -->

## Functions and Macros  
  
Library: `libbe.so`

This section lists the Support Kit's general-purpose functions and macros.

* * *

###  atomic_add(), atomic_and(), atomic_or()

Declared In: `support/SupportDefs.h`

`int32 atomic_add(vint32* atomicVariable,  
int32 addValue);``int32 atomic_and(vint32* atomicVariable,  
int32 andValue);``int32 atomic_or(vint32* atomicVariable,  
int32 orValue);`

These functions perform the named operations (addition, bitwise AND, or bitwise OR) on the 32-bit value found in `atomicVariable`, thus:
    
    
    *atomicVariable += addValue
    *atomicVariable &= andValue
    *atomicVariable |= orValue

Each function returns the previous value of the vint32 variable that `atomicVariable` points to (in other words, they each return the value that was in *`atomicVariable` before the operation was performed).

These functions are guaranteed to be atomic: if two threads attempt to access the same atomic variable at the same time (through these functions), one of the two threads will be made to wait until the other thread has completed the operation and updated the `atomicVariable` value.

* * *

###  beep(), system_beep(), add_system_beep_event()

Declared In: `support/Beep.h`

`status_t beep();``status_t system_beep(char* eventName,  
uint32 flags = 0);`

`beep()` produces the basic system beep. This function engages the Media Server, but doesn't wait for the sound to play. If it can't contact the server to play the beep, it returns `B_ERROR`. If it can make contact but can't get a satisfactory reply back from the server, it returns `B_BAD_REPLY`. Otherwise, it returns `B_OK`.

`system_beep()` plays the system beep configured for the event specified by the given `eventName`.

Applications can add new system beep events by calling `add_system_beep_event()`, passing the name to assign the event; the `flags` argument is currently unused and should be zero. Once this has been done, the user can use the Sounds preference application to configure a sound for the event.

See also: [`play_sound()`](TheMediaKit_Functions.md#play_sound "play_sound\(\)") in the Media Kit

* * *

###  class_name(), is_instance_of(), is_kind_of(), cast_as()

Declared In: `support/ClassInfo.h`

`char* class_name(object);``bool is_instance_of(object,  
class);``bool is_kind_of(object,  
class);``class* cast_as(object,  
class);`

These macros deliver information about an object's type, including the name of its class and its standing in the class hierarchy. In each case, the object argument is a pointer to an object; it can be an object of any type (it doesn't have to descend from any particular class). The class argument is a class name—not a string such as "`BApplication`", but the type name itself (literally `BApplication`).

`class_name()` returns a pointer to the name of object's class.

`is_instance_of()` returns `true` if object is an instance of class, and `false` otherwise.

`is_kind_of()` returns `true` if object is an instance of class or an instance of any class that inherits from class, and `false` if not.

`cast_as()` returns a pointer to object cast as a pointer to an object of class, but only if object is a kind of class. If not, object cannot be safely cast as a pointer to class, so `cast_as()` returns `NULL`.

For example, given this slice of the inheritance hierarchy from the Interface Kit, and code like this that creates an instance of the `BPictureButton` class,
    
    
    BButton* anObject = new BPictureButton(...);

the first three macros would work as follows:

  * The `class_name()` macro would return a pointer to the string "BPictureButton":
        
        const char *s = class_name(anObject);

  * The `is_instance_of()` macro would return `true` only if the `class` passed to it is [`BPictureButton`](BPictureButton.md "BPictureButton"). In the following example, it would return `false`, and the message would not be printed. Even though [`BPictureButton`](BPictureButton.md "BPictureButton") inherits from [`BView`](BView.md "BView"), the object is an instance of the [`BPictureButton`](BPictureButton.md "BPictureButton") class, not [`BView`](BView.md "BView"):
        
        if ( is_instance_of(anObject, BView) )
           printf("The object is an instance of BView.\n");

  * The `is_kind_of()` macro would return `true` if `class` is [`BPictureButton`](BPictureButton.md "BPictureButton") or any class that [`BPictureButton`](BPictureButton.md "BPictureButton") inherits from. In the following example, it would return true and the message would be printed. A [`BPictureButton`](BPictureButton.md "BPictureButton") is a kind of [`BView`](BView.md "BView"):
        
        if ( is_kind_of(anObject, BView) )
           printf("The object is a kind of BView.\n");

Note that class names are not passed as strings, but `class_name()` returns the name as a string.

The `cast_as()` macro is most useful when you want to treat a generic object as an instance of a more specific class. Suppose, for example, that the [`BPictureButton`](BPictureButton.md "BPictureButton") mentioned above becomes the focus view for a window and you retrieve it by calling the [`BWindow`](BWindow.md "BWindow")'s [`CurrentFocus()`](BWindow.md#BWindow_CurrentFocus "CurrentFocus\(\)") function:
    
    
    BView* focus = myWindow->CurrentFocus();

Since the focus view might be any type of view, [`CurrentFocus()`](BWindow.md#BWindow_CurrentFocus "CurrentFocus\(\)") returns a pointer to an object of the base [`BView`](BView.md "BView") class. Unless you know otherwise, you cannot treat the object as anything more specific than a [`BView`](BView.md "BView") instance. However, you can ask the object if it's a kind of [`BPictureButton`](BPictureButton.md "BPictureButton") and, if it is, cast it to the [`BPictureButton`](BPictureButton.md "BPictureButton") type:
    
    
    if ( is_kind_of(focus, BPictureButton) ) {
       BPictureButton* picbutton = (BPictureButton *)focus);
       if ( picbutton->Behavior() == B_TWO_STATE_BUTTON )
          . . .
    }

The `cast_as()` macro does the same thing, but more efficiently. It casts the object to the target class if it is safe to do so—if the object is an instance of a class that inherits from the target class or an instance of the target class itself—and returns `NULL` if not.
    
    
    BPictureButton* picbutton = cast_as(focus, BPictureButton);
    if ( picbutton ) {
       if ( picbutton->Behavior() == B_TWO_STATE_BUTTON )
          . . .
    }

`cast_as()` is often used in place of the cast operator to assure code safety even where an expected result is anticipated and there's no need for an intermediate variable (like `focus`):
    
    
    BPictureButton* picbutton =
        cast_as(myWindow->CurrentFocus(), BPictureButton);
    if ( picbutton ) {
          . . .
    }

The `cast_as()` and `is_kind_of()` macros work alike; they're both based on the C++ dynamic_cast operator and they reflect its behavior. To describe that behavior more precisely, let's adopt the following shorthand terms for an object's type:

  * The real type of an object is its type on construction. For example, if you construct an instance of the BButton class, as shown above, [`BButton`](BButton.md "BButton") is its real type.

  * The declared type of an object is the class label it currently bears. For example, `CurrentFocus()` returns an object whose declared class is [`BView`](BView.md "BView").

Either of these types can be compared to a target type, the type you want to cast the object to or test it against. The target type is the class argument passed to the macros.

In the best of all possible worlds, you'd want to ignore the declared type of an object and compare only the real type to the target type. However, the dynamic_cast operator—and by extension `cast_as()` and `is_kind_of()`—considers the real type only if it has to. It first compares the object's declared type to the target type. It assumes that the declared type is accurate (that the object is truly the kind of object it's represented to be) and it summarily handles the obvious cases: If the target type is the same as the declared type or if it's a class that the declared type inherits from, the operation will succeed. Consequently, `cast_as()` will cast the object to the target type and `is_kind_of()` will return true, regardless of the object's real type. In other words, if the target class is above or at the same level as the declared class in the inheritance hierarchy, the real class is ignored.

However, if the declared type doesn't match or derive from the target type, dynamic_cast and the macros look at the real type: If the target class is identical to the real type, or if it's a class that the real type derives from, the operation succeeds. If not, it fails.

Therefore, the `is_kind_of()` and `cast_as()` macros will produce reliable results as long as objects are not arbitrarily cast to types that may not be accurate. For example, you should not cast an object to a target type and then attempt to use `is_kind_of()` to determine if the cast was correct. This code is unreliable:
    
    
    BPictureButton* picbutton =
        (BPictureButton*)myWindow->CurrentFocus();
    
    if ( is_kind_of(picbutton, BPictureButton) ) {
          . . .
    }

In this example, `is_kind_of()` will always return `true`, no matter what the class of the current focus view. The general rule is that the declared type of an object must always be accurate; an object should be typed only to its own class or to a class that it inherits from. The macros cannot rescue you from an inaccurate cast.

* * *

###  convert_to_utf8(), convert_from_utf8()

Declared in: `support/UTF8.h`

`status_t convert_to_utf8(uint32 sourceEncoding,  
const char* source,  
int32* sourceLength,  
char* destination,  
int32* destinationLength,  
int32* state,  
char substitute = B_SUBSTITUTE);``status_t convert_from_utf8(uint32 destinationEncoding,  
const char* source,  
int32* sourceLength,  
char* destination,  
int32* destinationLength,  
int32* state,  
char substitute = B_SUBSTITUTE);`

These functions convert text to and from the Unicode™ UTF-8 encoding that's standard for the Be operating system and is assumed in most contexts. UTF-8 is described under "[Character Encoding](TheInterfaceKit_Character_Encoding.md "Character Encoding")" section of The Interface Kit chapter.

`convert_to_utf8()` permits you to take text that's encoded according to another standard and convert it to UTF-8 for the BeOS. `convert_from_utf8()` lets you convert text from UTF-8 to other encodings for other venues (for example, to the encodings commonly used for displaying text on the World Wide Web).

The first argument passed to these functions names the other encoding&#8212;the source encoding for `convert_to_utf8()` and the destination encoding for `convert_from_utf8()`. It can be any of the following constants:

`B_ISO1_CONVERSION`| `B_MAC_ROMAN_CONVERSION`  
---|---  
`B_ISO2_CONVERSION`| `B_SJIS_CONVERSION`  
`B_ISO3_CONVERSION`| `B_EUC_CONVERSION`  
`B_ISO4_CONVERSION`| `B_JIS_CONVERSION`  
`B_ISO5_CONVERSION`| `B_MS_WINDOWS_CONVERSION`  
`B_ISO6_CONVERSION`| `B_UNICODE_CONVERSION`  
`B_ISO7_CONVERSION`| `B_KOI8R_CONVERSION`  
`B_ISO8_CONVERSION`| `B_MS_WINDOWS_1251_CONVERSION`  
`B_ISO9_CONVERSION`| `B_MS_DOS_866_CONVERSION`  
`B_ISO10_CONVERSION`| ` `  
  
Most of these constants designate encoding schemes that are supported by the [`BFont`](BFont.md "BFont") class in the Interface Kit and its [`SetEncoding()`](BFont.md#BFont_SetEncoding "SetEncoding\(\) , Encoding\(\)") function. They parallel the constants that are passed to that function. For example, `B_ISO1_CONVERSION` (for these functions) and `B_ISO_8859_1` (for [`SetEncoding()`](BFont.md#BFont_SetEncoding "SetEncoding\(\) , Encoding\(\)")) both designate the extended ASCII encoding defined in part one of ISO 8859 (Latin 1). Similarly, `B_ISO2_CONVERSION` matches `B_ISO_8859_2`, `B_ISO3_CONVERSION` matches `B_ISO_8859_3`, and so on. `B_MAC_ROMAN_CONVERSION` matches `B_MACINTOSH_ROMAN`. (`B_ISO10_CONVERSION` is not implemented for this release.)

`B_SJIS_CONVERSION` stands for the Shift-JIS (Japanese Industrial Standard) encoding of Japanese and `B_EUC_CONVERSION` stands for the EUC (Extended UNIX Code) encoding of Japanese in packed format.

Both functions convert up to `sourceLength` bytes of text from the source buffer. They write up to `destinationLength` bytes of converted text into the destination buffer. The amount of text that they actually convert is therefore constrained both by the amount of source text (`sourceLength`) and the capacity of the output buffer (`destinationLength`). Neither function stops at a null terminator ('0') when reading the input buffer nor adds one to the text in the output buffer; they depend only on `sourceLength` and `destinationLength` for guidance.

When finished, these functions modify the variable that `sourceLength` refers to so that it reports the number of bytes of source text actually converted. They also modify the variable that `destinationLength` refers to so that it reports the number of bytes actually written to the destination buffer. Neither function will stop in the middle of a multibyte source character; they're guaranteed to convert only full characters.

The `state` argument serves as a cookie that lets you use multiple calls to these functions to convert a batch of text (if, for example, the text is stored in multiple buffers). Pass 0 for the first buffer, and pass the value returned in state to the next call to continue processing the same text; this will cause the conversion to continue where it left off. For example:
    
    
    /* buffer is a pointer to a small source buffer */
    /* destBuffer is a pointer to a small destination buffer */
    /* destBufferLen is the size of the destination buffer in bytes */
    int32 state = 0;
    int32 bufferLen;
    
    while ((bufferLen = GetNextBuffer(file, buffer))) {
       convert_to_utf8(buffer, &amp;bufferLen, destBuffer, &amp;destBufferLen,
           &amp;state);
       /* do stuff with the buffer in destBuffer, which has the converted
          text */
    }
    

In this example, text is fetched using a call to a function called `GetNextBuffer()`, whose implementation depends on the application's needs. Each buffer is converted, and the converted text is stored in destBuffer, which can then be used by the application.

If either function encounters a character in the source that the destination format doesn't allow, it puts the character specified by the substitute argument (or a question mark ('?') if substitute isn't specified) in its place in the output text. This is much more likely to occur when converting from UTF-8 than when converting to it, since Unicode represents a very large number of characters.

If successful in converting at least one source character, both functions return `B_OK`. If unsuccessful, for example, if they don't recognize the source or destination encoding, they return `B_ERROR`. If there's an error, you should not trust any of the output arguments.

Note

![Note](./images/admonitions/Info_32.png)

These functions are found in the `libtextencoding.so` library. If you use them, be sure to include this library file.

See also: [`BFont::SetEncoding()`](BFont.md#BFont_SetEncoding "SetEncoding\(\) , Encoding\(\)"), "[Character Encoding](TheInterfaceKit_Character_Encoding.md "Character Encoding")" section of the The Interface Kit chapter

* * *

### find_instantiation_func()

Declared in: `support/Archivable.h`

`instantiation_func find_instantiation_func(const char* className);` `instantiation_func find_instantiation_func([`BMessage`](BMessage.md "BMessage")* archive);`

Returns a pointer to the [`Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)") function that can create instances of the `className` class, or `NULL` if the function can't be found. If passed a [`BMessage`](BMessage.md "BMessage") archive, `find_instantiation_func()` gets the name of the class from a `B_STRING_TYPE` field called "class" in the [`BMessage`](BMessage.md "BMessage").

The instantiation_func type is defined as follows:
    
    
    BArchivable* (*instantiation_func) (BMessage *)

In other words, the function has the same syntax as the [`Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)") function declared in the [`BArchivable`](BArchivable.md "BArchivable") class and replicated in derived classes (with class-specific return values).

The function that's returned can be called like any C function; you don't need the class name or another object of the class. For example:
    
    
    instantiation_func func;
    if ( func = find_instantiation_func(arhiveMessage) ) {
       BArchivable* object = func(archiveMessage);
    }

`instantiate_object()` will do this work for you.

See also: [`Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)")

* * *

### instantiate_object()

Declared in: `support/Archivable.h`

`[`BArchivable`](BArchivable.md "BArchivable")* instantiate_object([`BMessage`](BMessage.md "BMessage")* archive);`

Creates and returns a new instance of an archived object, or returns `NULL` if the object can't be constructed. The object is created by calling the [`Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)") function of…

  1. …the class that was last added to the "class" field (an array of class names) of the archive [`BMessage`](BMessage.md "BMessage"). This will be the "most derived" class in the array.

  2. If the named class isn't recognized by the app (or doesn't define [`Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)")), the function tries to load the add-on that's identified (by signature) in archive's `add_on` field. This add-on should contain the code for the class.

  3. If the add-on method doesn't work, `instantiate_object()` tries the next class in the "class" array, and so works its way up the class hierarchy.

  4. If the appropriate class is never found, the function returns `NULL`.

When successful, `instantiate_object()` returns the object that `Instantiate()` created, but typed to the base [`BArchivable`](BArchivable.md "BArchivable") class. The `cast_as()` macro can type it to a the proper class.
    
    
    BArchivable* base = instantiate_object(archive);
    if ( base ) {
       TheClass* object = cast_as(base, TheClass);
       if ( object ) {
          . . .
       }
    }

* * *

###  swap_data(), is_type_swapped()

Declared in: `support/ByteSwap.h`

`status_t swap_data(type_code type,  
void* data,  
size_t length,  
swap_action action);``bool is_type_swapped(type_code type);`

`swap_data` is a general byte swapping function, converting data of type type and converting its endianness according to action (defined in "[ Byte Swapping Constants](TheSupportKit_Constants.md#Constants_Byte_Swapping "Byte Swapping Constants")"). The length field can be used to specify an array of items to be converted. For example, you can convert an array of [`BMessenger`](BMessenger.md "BMessenger")s from little endian to the host endianness:
    
    
    BMessenger messengers[10];
    ...
    swap_data(B_MESSENGER_TYPE, messengers, 10*sizeof(BMessenger)
        B_SWAP_LENDIAN_TO_HOST);

The function can swap most data types with type constants defined in `support/TypeConstants.h`. It returns `B_OK` on success and `B_BAD_VALUE` on failure.

`is_type_swapped()` takes a type code and determines whether or not it has been swapped. This only works for types defined in `support/TypeConstants.h`. It returns `true` if the type code is in the host's native endianness and `false` otherwise.

See also: "[ Byte Swapping Constants](TheSupportKit_Constants.md#Constants_Byte_Swapping "Byte Swapping Constants")"

* * *

###  min(), max(), min_c(), max_c()

Declared in: `support/SupportDefs.h`

`min(a,  
b);``min_c(a,  
b);``max(a,  
b);``max_c(a,  
b);`

These macros compare two integers or floating-point numbers. `min()` and `min_c()` return the lesser of the two (or b if they're equal); `max()` and `max_c()` return the greater of the two (or a if they're equal). `min()` and `max()` can only be used in straight C programs. `min_c()` and `max_c()` are defined for C and C++.

* * *

### validate_instantiation()

`bool validate_instantiation(BMessage* archive,  
const char* className);`

Declared in: `support/Archivable.h`

Returns `true` if the `archive` [`BMessage`](BMessage.md "BMessage") contains data for an object belonging to the `className` class, and `false` if not. The determination is made by looking for the class name in a "class" array in the archive. If the class name appears anywhere in the array, this function returns `true`. If not, it returns `false`.

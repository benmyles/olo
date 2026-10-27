<!-- System Overview - The Storage Kit - BMimeType -->

## BMimeType

The `BMimeType` class provides three services:

  * It can parse a MIME string. It can tell you whether the string is valid, what it's supertype component is, and whether it has a subtype component. (The MIME string format is described in "Valid MIME Strings."

  * It gives you access to the File Type database. Given a MIME type, it can look in the database and retrieve that type's icon(s), "preferred handler" application, the filename extensions that correspond to it, and so on.

  * It can regard a MIME string as an application signature, and so get and set the executable file, the file types, and the document icons that correspond to that signature.

All three services operate on MIME strings. In other words, they answer questions such as "Does this string have a supertype?", "Is this string installed in the database?", and so on. You can get the MIME strings from anywhere: from a file's file type attribute, from and application's signature, from the header of an e-mail message, you can even make them up.

* * *

### Valid MIME Strings

A valid MIME string takes the form…
    
    
    supertype/[subtype]
    

…where supertype is one of the seven "media" strings:

  * text

  * application

  * image

  * audio

  * video

  * multipart

  * message

…and (the optional) subtype can be just about anything… Except it can't include spaces or any of these forbidden characters:
    
    
    * / < > @ , ; : " ( ) [ ] ? =
    

When you initialize a `BMimeType` object (through the constructor or [`SetTo()`](BMimeType.md#BMimeType_SetTo "SetTo\(\), Unset\(\)") function), you have to tell it what MIME string you want it to represent:

  * The string can be supertype-only, or it can be supertype/subtype.

  * Currently, the supertype is not restricted to the seven types listed above, but you're probably making a mistake if you make up a new, unrecognized supertype.

  * Neither the supertype nor the subtype can include any of the forbidden characters.

  * The entire string must be no longer than `B_MIME_TYPE_LENGTH` characters long. (That's about 240 characters. More than enough.)

You can check the validity of a MIME string without constructing a `BMimeType` object by calling the static [`IsValid()`](BMimeType.md#BMimeType_IsValid "IsValid\(\), IsSupertypeOnly\(\)") function:
    
    
    BMimeType::IsValid("text/qwerty");

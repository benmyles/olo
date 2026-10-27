<!-- Classes And Methods - The Application Kit - BCursor -->

## BCursor

| Derived From:| –  
---|---  
Mix-in Classes:| [`BArchivable`](BArchivable.md "BArchivable")  
Declared In:| `app/Cursor.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BCursor_Overview.md "BCursor")

  * [Constructor and Destructor](BCursor.md#BCursor_ConstructorDestructor)
  * [Static Functions](BCursor.md#BCursor_StaticFunctions)
  * [ Constants](BCursor.md#TheApplicationKit_Constants)

  
  
* * *

### Constructor and Destructor

#### BCursor()

`BCursor(const void* cursorData);` `BCursor(BMessage* archive);`

Initializes the new cursor object. If you specify a non-`NULL` value for `cursorData`, the cursor is initialized with the specified cursor data.

If you specify a `NULL` value for `cursorData`, the cursor is useless; since this class doesn't currently provide a means of setting the cursor data once the object is instantiated, you're out of luck, so why bother?

`BCursor` doesn't currently implement archiving, so you shouldn't use the second form.

#### ~BCursor()

`virtual ~BCursor();`

Releases any resources used by the cursor.

* * *

### Static Functions

#### Instantiate()

`static BArchivable* Instantiate(BMessage* archive);`

Not currently implemented; always returns `NULL`.

See also: [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)")

* * *

###  Constants

Declared in: `app/AppDefs.h`
    
    
    const unsigned char B_HAND_CURSOR[]
    const unsigned char B_I_BEAM_CURSOR[]

These constants contain all the data needed to set the cursor to the default hand image or to the standard I-beam image for text selection.
    
    
    const BCursor* B_CURSOR_SYSTEM_DEFAULT
    const BCursor* B_CURSOR_I_BEAM

The second two constants are [`BCursor`](BCursor.md "BCursor") objects and produce the same cursors the first two constants.

See also: [`BApplication::SetCursor()`](BApplication.md#BApplication_SetCursor "SetCursor\(\), HideCursor\(\), ShowCursor\(\), ObscureCursor\(\), IsCursorHidden\(\)")

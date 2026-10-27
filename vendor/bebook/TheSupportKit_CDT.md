<!-- Classes And Methods - The Support Kit - Defined Types -->

## Defined Types

* * *

### bigtime_t

Declared in: `support/SupportDefs.h`
    
    
    typedef int64 bigtime_t

This type records the time in microseconds as a 64-bit integer. Typically, a bigtime_t variable measures the system time, the number of microseconds since 12:00:00 AM January 1, 1970, UTC (Coordinated Universal Time).

See Also: [`system_time()`](TheKernelKit_Time.md#system_time "system_time\(\)")

* * *

### bool

Declared in: `support/SupportDefs.h`
    
    
    typedef unsigned char bool

The C++ language defines bool as its basic boolean type. The BeOS extends the definition to C code, so you can use the same type in both languages. The `true` and `false` constants (listed above) are defined as boolean values.

* * *

### Function Pointers

Declared in: `support/SupportDefs.h`
    
    
    typedef int (*B_PFI)()
    typedef long (*B_PFL)()
    typedef void (*B_PFV)()

These types define pointers to functions that return int, long, and void values respectively.

* * *

### instantiation_func

Declared in: `support/Archivable.h`
    
    
    typedef BArchivable* (*instantiation_func) (BMessage*)

This type is a pointer to a function that can instantiate an object from a `BMessage` archive and return a pointer to the new object. The `Instantiate()` member function of [`BArchivable`](BArchivable.md "BArchivable") is such a function.

See also: [`find_instantiation_func()`](TheSupportKit_Functions.md#find_instantiation_func "find_instantiation_func\(\)")

* * *

###  Integer Types

Declared in: `support/SupportDefs.h`
    
    
    typedef unsigned char uchar
    typedef signed char int8
    typedef unsigned char uint8
    typedef volatile signed char vint8
    typedef volatile unsigned char vuint8
    typedef short int16
    typedef unsigned short uint16
    typedef volatile short vint16
    typedef volatile unsigned short vuint16
    typedef long int32
    typedef unsigned long uint32
    typedef volatile long vint32
    typedef volatile unsigned long vuint32
    typedef long long int64
    typedef unsigned long long uint64
    typedef volatile long long vint64
    typedef volatile unsigned long long vuint64

These type names are defined as convenient shorthands for standard integers of various sizes. They're used in place of int, short, and long throughout the BeOS API.

The number at the end of the type name indicates the size of the integer. For example, a uint32 is an unsigned 32-bit value. The type is guaranteed to be defined to its stated size for all platforms (thus their exact definitions may vary from platform to platform).

Using these types will make the code you write more portable. They'll help avoid problems as the operating system and your application move to other platforms.

* * *

### status_t

Declared in: `support/SupportDefs.h`
    
    
    typedef int32 status_t

This type indicates an error code; it's used mainly for function return values.

See also: "[Error Codes](TheSupportKit_Error_Codes.md "Error Codes")"

* * *

### type_code

Declared in: `support/SupportDefs.h`
    
    
    typedef uint32 type_code

This type is used for the integer codes that indicate a particular data type. The codes—such as `B_UINT32_TYPE` and `B_MIME_TYPE`—mark the type of data added to a message or stored as a resource and also appear in other contexts.

See also: "[ Type Codes](TheSupportKit_Constants.md#Constants_Type_Codes "Type Codes")", the [`BMessage`](BMessage.md "BMessage") class in the Application Kit, the [`BResources`](BResources.md "BResources") class in the Storage Kit

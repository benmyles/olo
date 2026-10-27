<!-- Classes And Methods - The Media Kit - BTimeCode -->

## BTimeCode  
  
| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `media/TimeCode.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BTimeCode_Overview.md "BTimeCode")

  * [Constructor and Destructor](BTimeCode.md#BTimeCode_ConstructorDestructor)
  * [Member Functions](BTimeCode.md#BTimeCode_MemberFunctions)
  * [Operators](BTimeCode.md#BTimeCode_Operators)
  * [Global C Functions](BTimeCode.md#BTimeCode_Functions)
  * [Constants](BTimeCode.md#BTimeCode_Constants)
  * [Defined Types](BTimeCode.md#BTimeCode_DefinedTypes)

  
  
* * *

### Constructor and Destructor

#### BTimeCode()

`BTimeCode();` `BTimeCode(bigtime_t us,  
timecode_type type = B_TIMECODE_DEFAULT);` `BTimeCode(int hours,  
int minutes,  
int seconds,  
int frames,  
timecode_type type = B_TIMECODE_DEFAULT);` `BTimeCode(const BTimeCode& clone);`

The constructor prepares the `BTimeCode` object for use. If you use the first form of the constructor, without arguments, you'll have to call an appropriate function to set the `BTimeCode`'s time information before using it for translation purposes. This can be done by calling one or more of [`SetData()`](BTimeCode.md#BTimeCode_SetData), [`SetType()`](BTimeCode.md#BTimeCode_SetType), [`SetMicroseconds()`](BTimeCode.md#BTimeCode_SetMicroseconds), or [`SetLinearFrames()`](BTimeCode.md#BTimeCode_SetLinearFrames).

The second form of the constructor accepts as input a time in microseconds, `us`, and the timecode `type`.

The third form accepts as input a time in `hours`, `minutes`, `seconds`, and `frames`, as well as the timecode `type`.

The fourth form of the constructor duplicates an existing `BTimeCode` object.

#### ~BTimeCode()

`~BTimeCode();`

A typical destructor

* * *

### Member Functions

####  GetData(), SetData()

`void GetData(int* outHours,  
int* outMinutes,  
int* outSeconds,  
int* outFrames,  
timecode_type* outType);``void SetData(int hours,  
int minutes,  
int seconds,  
int frames);`

`GetData()` returns the timecode's value in `hours`, `minutes`, `seconds`, and `frames`, and also returns the timecode type, if you specify a valid pointer for `outType`. `SetData()` lets you set the timecode's value.

#### GetString()

`void GetString(char* str) const;`

Fills `str`, which must be at least 24 bytes long, with a string indicating the current time in hours, minutes, seconds, and frames. The string is formatted in a manner appropriate to the timecode type. A typical example would be "01:24:09.18", which is 1 hour, 24 minutes, 9 seconds, and 18 frames.

####  Hours(), Minutes(), Seconds(), Frames()

`void Hours() const;``void Minutes() const;``void Seconds() const;``void Frames() const;`

These functions return the time's hours, minutes, seconds, and frames portions.

####  LinearFrames(), SetLinearFrames()

`int32 LinearFrames() const;``void SetLinearFrames(int32 linearFrames);`

`LinearFrames()` returns the `BTimeCode` object's time in linear frames. `SetLinearFrames()` lets you change the time, specifying the new time in linear frames.

####  Microseconds(), SetMicroseconds()

`bigtime_t LinearFrames() const;``void SetMicroseconds(bigtime_t us);`

`Microseconds()` returns the `BTimeCode` object's time, in microseconds. `SetMicroseconds()` lets you change the time, specifying the new time in microseconds.

####  Type(), SetType()

`timecode_type Type() const;``status_t SetType(timecode_type type);`

`Type()` returns the `BTimeCode` object's timecode type. `SetType()` lets you change the timecode type.

Return Code| Description  
---|---  
`B_OK`| The type was returned safely.  
`B_ERROR`| The timecode type specified is invalid.  
  
* * *

### Operators

#### = (assignment)

`BTimeCode& operator =(const BTimeCode& clone);`

Makes the current `BTimeCode` identical to the `BTimeCode` object specified

#### == (equality)

`BTimeCode& operator ==(const BTimeCode& other);`

Determines whether or not the two `BTimeCode` objects are equal (their times are the same, regardless of their timecode types).

#### < (less than)

`BTimeCode& operator <(const BTimeCode& other);`

Indicates whether or not one `BTimeCode`'s time, in microseconds, is less than the other's.

#### += (expression plus)

`BTimeCode& operator +=(const BTimeCode& other);`

Adds the time of the `BTimeCode` object other to the current `BTimeCode`'s time.

#### -= (expression minus)

`BTimeCode& operator -=(const BTimeCode& other);`

Subtracts the time of the `BTimeCode` object other from the current `BTimeCode`'s time.

#### \+ (plus)

`BTimeCode& operator +(const BTimeCode& other);`

Adds two `BTimeCode` values together, returning a new `BTimeCode`.

#### \- (minus)

`BTimeCode& operator <(const BTimeCode& other);`

Subtracts two `BTimeCode` values, returning a new one.

* * *

### Global C Functions

#### count_timecodes()

`status_t count_timecodes();`

Returns the number of recognized time code types.

####  frames_to_timecode(), timecode_to_frames()

`status_t frames_to_timecode(int32 linearFrames,  
int* hours,  
int* minutes,  
int* seconds,  
int* frames,  
const timecode_info code = NULL);``status_t timecode_to_frames(int hours,  
int minutes,  
int seconds,  
int frames,  
const timecode_info code = NULL);`

`frames_to_timecode()` converts the frame offset `linearFrames` into `hours`, `minutes`, `seconds`, and `frames`.

`timecode_to_frames()` converts the time from `hours`, `minutes`, `seconds`, and `frames` into a linear frame offset, storing the result in `linearFrames`.

The timecode_info structure code is used to determine how the conversion should be made, if you specify it. Otherwise `B_TIMECODE_DEFAULT` is assumed.

Currently these functions always return `B_OK`, but you should still check for errors because you'd hate it if your app broke in the future, wouldn't you?

#### get_timecode_description()

`status_t get_timecode_description(timecode_type type,  
timecode_info* outTimeCode);`

Fills out the timecode_info structure specified by `outTimeCode` with information describing the specified timecode type.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_ERROR`| The specified timecode type isn't valid.  
  
####  us_to_timecode(), timecode_to_us()

`status_t us_to_timecode(bigtime_t micros,  
int* hours,  
int* minutes,  
int* seconds,  
int* frames,  
const timecode_info code = NULL);``status_t timecode_to_us(int hours,  
int minutes,  
int seconds,  
int frames,  
bigtime_t* micros,  
const timecode_info code = NULL);`

`us_to_timecode()` converts the time `micros`, which is specified in microseconds, into `hours`, `minutes`, `seconds`, and `frames`.

`timecode_to_us()` converts the time from `hours`, `minutes`, `seconds`, and `frames` into microseconds, storing the result in `micros`.

The timecode_info structure code is used to determine how the conversion should be made, if you specify it. Otherwise `B_TIMECODE_DEFAULT` is assumed.

Currently these functions always return `B_OK`, but you should still check for errors because you'd hate it if your app broke in the future, wouldn't you?

* * *

### Constants

#### timecode_type

Declared in: `media/TimeCode.h`

Constant| Description  
---|---  
`B_TIMECODE_DEFAULT`| The default time code  
`B_TIMECODE_100`| 100 frames per second.  
`B_TIMECODE_75`| CD audio  
`B_TIMECODE_30`| MIDI  
`B_TIMECODE_30_DROP_2`| NTSC  
`B_TIMECODE_30_DROP_4`| Brazil  
`B_TIMECODE_25`| PAL  
`B_TIMECODE_24`| Film  
`B_TIMECODE_18`| Super8  
  
Constants identifying the various timecode types supported by `BTimeCode`.

* * *

### Defined Types

#### timecode_info

Declared in: `media/TimeCode.h`
    
    
    struct timecode_info {
        timecode_type type;
        int drop_frames;
        int every_nth;
        int except_nth;
        int fps_div;
        char name[32];
        char format[32];
        char _reserved_[64];
    };

The timecode_info structure describes the attributes of a timecode type. You probably should just use the `BTimeCode` class, or the global C functions, though. It just makes your life easier.

Field| Description  
---|---  
`type`| Indicates the timecode type described by the structure.  
`drop_frames`| Indicates how many frames this timecode drops every `every_nth` minute, `except_nth` minute.  
`fps_div`| Indicates the nominal frame rate of the format.  
`name`| Is a printable name that can be used in constructing user interfaces.  
`format`| Is a format to be used in calling `sprintf()`; it's used by [`GetString()`](BTimeCode.md#BTimeCode_GetString "GetString\(\)").

<!-- Classes And Methods - The Network Kit - BNetDebug -->

## BNetDebug

| Derived From:| None  
---|---  
Mix-in Classes:| None  
Declared In:| `net/NetDebug.h`  
Library:| `libnetapi.so`  
Allocation:| Constructor only  
[Class Overview](BNetDebug_Overview.md "BNetDebug")

  * [Static Functions](BNetDebug.md#BNetDebug_StaticFunctions)

  
  
* * *

### Static Functions

#### Dump()

`static void Dump(const char* data,  
size_t size,  
const char* title) const;`

Dumps `size` bytes of raw `data` to `stderr`. The dump is prefaced by the given `title`. The text is only output if debug output is currently enabled.

####  Enable(), IsEnabled()

`static void Enable(bool enable);``static bool IsEnabled();`

`Enable()` enables debug output if `enable` is `true`, otherwise it disables debug output.

`IsEnabled()` reports the current state of debug output.

#### Print()

`static void Print(const char* message);`

Prints the specified `message` to `stderr`, if debug output is currently enabled.

<!-- System Overview - The Network Kit - BNetDebug -->

## BNetDebug

[`BNetDebug`](BNetDebug.md "BNetDebug") provides a few functions that let you turn on and off debug output, and dump raw data to stderr in a clean format.

All the member functions are static; instead of creating a [`BNetDebug`](BNetDebug.md "BNetDebug") object, call them like this:
    
    
    BNetDebug::Print("Starting server...");

Debug output is off by default; call [`BNetDebug::Enable()`](BNetDebug.md#BNetDebug_Enable "Enable\(\), IsEnabled\(\)") to enable it.

<!-- System Overview - The Support Kit - Introduction -->

## Introduction

The Support Kit contains classes and utilities that any application can take advantage of. Among other things, it includes:

  * The [`BArchivable`](BArchivable.md "BArchivable") protocol for objects that can archive themselves and be instantiated from their archives.

  * The [`BBlockCache`](BBlockCache.md "BBlockCache") class provides a simple mechanism for caching fixed-length blocks of memory.

  * The [`BList`](BList.md "BList") class, a container for keeping track of a number of data items, typically object pointers.

  * The [`BLocker`](BLocker.md "BLocker") and [`BAutolock`](BAutolock.md "BAutolock") classes for implementing locking mechanisms.

  * The [`BDataIO`](BDataIO.md "BDataIO") and [`BPositionIO`](BPositionIO.md "BPositionIO") protocols for objects that can be read and written, and the [`BMemoryIO`](BMemoryIO.md "BMemoryIO") and [`BMallocIO`](BMallocIO.md "BMallocIO") classes that implement the protocols.

  * The [`BFlattenable`](BFlattenable.md "BFlattenable") protocol for objects that can write themselves to a flattened representation.

  * The [`BMessageRunner`](BMessageRunner.md "BMessageRunner") class sends a message over and over.

  * The [`BString`](BString.md "BString") class is an interface to smart string allocation, storage, and manipulation. Call me weak, but I can't live without it.

  * The [`BStopWatch`](BStopWatch.md "BStopWatch") class times your functions. Use it when you're developing your app.

  * Common defined types and constants, such as int32 and int64.

  * The error codes for (theoretically) all the software kits. (Some error code may have snuck into other kits.)

  * Type codes for identifying data types in messages, attributes, and resources.

  * Utility functions such as [`atomic_add()`](TheSupportKit_Functions.md#atomic_add "atomic_add\(\), atomic_and\(\), atomic_or\(\)"), [`swap_data()`](TheSupportKit_Functions.md#swap_data "swap_data\(\), is_type_swapped\(\)"), and [`convert_to_utf8()`](TheSupportKit_Functions.md#convert_to_utf8 "convert_to_utf8\(\), convert_from_utf8\(\)").

  * [Thread Local Storage](TheSupportKit_TLS.md "Thread Local Storage") functions that let you define global variables whose values depend on the thread from which they're accessed. These functions are handy when you're porting code that was designed for a single-threaded system.

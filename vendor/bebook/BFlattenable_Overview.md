<!-- System Overview - The Support Kit - BFlattenable -->

## BFlattenable

The [`BFlattenable`](BFlattenable.md "BFlattenable") class declares a protocol for objects that can be flattened—written to an untyped buffer of bytes—and unflattened from the buffer. By implementing this protocol, a class gives others the ability to manipulate its objects in a flexible manner. Currently, only the [`BMessage`](BMessage.md "BMessage") class, through its [`AddFlat()`](BMessage.md#BMessage_AddFlat) and [`FindFlat()`](BMessage.md#BMessage_FindFlat) functions, declares an API that can deal with [`BFlattenable`](BFlattenable.md "BFlattenable") objects. The [`BPath`](BPath.md "BPath") class in the Storage Kit derives from [`BFlattenable`](BFlattenable.md "BFlattenable").

As this class merely declares a protocol for other classes to implement, it doesn't include a constructor or destructor.

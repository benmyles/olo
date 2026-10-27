<!-- Classes And Methods - The Translation Kit - BTranslationUtils -->

## BTranslationUtils

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `translation/TranslationUtils.h`  
Library:| `libtranslation.so`  
Allocation:| –  
[Class Overview](BTranslationUtils_Overview.md "BTranslationUtils")

  * [Constructor and Destructor](BTranslationUtils.md#BTranslationUtils_ConstructorDestructor)
  * [Static Functions](BTranslationUtils.md#BTranslationUtils_StaticFunctions)

  
  
* * *

### Constructor and Destructor

The `BTranslationUtils` class only contains static functions and so there is no constructor or destructor.

* * *

### Static Functions

#### GetBitmap()

`static [`BBitmap`](BBitmap.md "BBitmap") GetBitmap(const char* name,  
[`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") name = NULL);` `static [`BBitmap`](BBitmap.md "BBitmap") GetBitmap(entry_ref ref,  
[`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") name = NULL);` `static [`BBitmap`](BBitmap.md "BBitmap") GetBitmap(uint32 type,  
int32 id,  
[`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") name = NULL);` `static [`BBitmap`](BBitmap.md "BBitmap") GetBitmap(uint32 type,  
const char* name,  
[`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") name = NULL);` `static [`BBitmap`](BBitmap.md "BBitmap") GetBitmap([`BPositionIO`](BPositionIO.md "BPositionIO") stream = NULL,  
[`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") name = NULL);`

The first version of the function returns the image held in the file `name` if it exists. Otherwise, it returns the application resource of type `B_TRANSLATOR_BITMAP` named `name`. The second version does the same thing, but uses an entry_ref, `ref`, to locate the file.

The third and fourth versions of the function search for the specified resource and return the bitmap contained therein.

The final version returns the bitmap found in `stream`. This form of the function is particularly useful when used with [`BMemoryIO`](BMemoryIO.md "BMemoryIO").

In all cases, `use` specifies the `BTranslatorRoster` to use in translating the image. If `use` is `NULL`, the default translator (as returned by [`BTranslatorRoster::Default()`](BTranslatorRoster.md#BTranslatorRoster_Default "Default\(\)")) is used.

If the file or stream doesn't contain a suitable bitmap, the functions return `NULL`.

#### GetBitmapFile()

`static [`BBitmap`](BBitmap.md "BBitmap") GetBitmapFile(const char* name,  
[`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") name = NULL);`

Returns the image held in the file `name`. If the file doesn't contain a suitable image, the function returns `NULL`. `use` specifies the `BTranslatorRoster` used to translate the image. If `use` is `NULL`, the default translator (as returned by [`BTranslatorRoster::Default()`](BTranslatorRoster.md#BTranslatorRoster_Default "Default\(\)")) is used.

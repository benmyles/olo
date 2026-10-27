<!-- Classes And Methods - The Translation Kit - BTranslator -->

## BTranslator

| Derived From:| –  
---|---  
Mix-in Classes:| [`BArchivable`](BArchivable.md "BArchivable")  
Declared In:| `translation/Translator.h`  
Library:| `libtranslation.so`  
Allocation:| –  
[Class Overview](BTranslator_Overview.md "BTranslator")

  * [Constructor and Destructor](BTranslator.md#BTranslator_ConstructorDestructor)
  * [Member Functions](BTranslator.md#BTranslator_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BTranslator()

`BTranslator();`

The constructor must create and return a new instance of your `BTranslator` subclass. Note that the constructor doesn't [`Acquire()`](BTranslator.md#BTranslator_Acquire "Acquire\(\), Release\(\), ReferenceCount\(\)") the object it returns.

#### ~BTranslator()

##### protected

`~BTranslator();`

Note that the destructor is protected; you can only delete a `BTranslator` object from within the implementation of the subclass. From outside the class, you call [`Release()`](BTranslator.md#BTranslator_Release).

* * *

### Member Functions

####  Acquire(), Release(), ReferenceCount()

`BTranslator* Acquire();``BTranslator* Release();``int32 ReferenceCount();`

Warning

![Warning](./images/admonitions/Stop_32.png)

`ReferenceCount()` Is For Debugging use only!

`Acquire()` and `Release()` increment and decrement the object's reference count. The count starts at 1 (i.e .the object is implicitly acquired when it's created); if the count falls to 0, the object is deleted.

When you add a `BTranslator` to a [`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster"), the `BTranslator` is automatically `Acquire()`'d. When the [`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") is deleted, its `BTranslator`s are `Release()`'d. Thus, when you instantiate a `BTranslator` and add it to a [`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster"), you and the Roster maintain joint ownership of the object. To give up ownership (such that the [`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") will destroy the object when the Roster itself is destroyed), call `Release()` after adding the `BTranslator` to the Roster.

`Acquire()` and `Release()` both return a pointer to the `BTranslator` that was just acquired or released. If `Release()` caused the object to be deleted, it retruns `NULL`.

`ReferenceCount()`, which returns the current reference count value, is meant for debugging purposes only. It is not thread-safe! Don't predicate your code on the value it returns.

#### GetConfigurationMessage()

##### Optional

`virtual status_t GetConfigurationMessage([`BMessage`](BMessage.md "BMessage")* ioExtension);`

Hook function that asks the object to write its current state into the [`BMessage`](BMessage.md "BMessage")* argument. See [`GetConfigMessage()`](TranslatorAddOns.md#GetConfigMessage "GetConfigMessage\(\)") [Translator Add-ons] for details.

#### Identify()

##### Required

`virtual status_t Identify([`BPositionIO`](BPositionIO.md "BPositionIO")* inSource,  
const translation_format* inFormat,  
[`BMessage`](BMessage.md "BMessage")* ioExtension,  
translator_info* outInfo,  
uint32* outType) = 0;`

Hook function called by the Translator Roster to ask the `BTranslator` if it knows how to convert `inSource` into the type described by `outType`. See [`Identify()`](TranslatorAddOns.md#Identify "Identify\(\)") [Translator Add-ons] for details.

####  InputFormats(), OutputFormats()

##### Optional

`virtual const translation_format* InputFormats(int32* count) const;``virtual const translation_format* OutputFormats(int32* count) const;`

These functions should be implemented to return arrays of translation_format structures that describe the formats that this object supports. If the functions aren't implemented, the object's [`Identify()`](BTranslator.md#BTranslator_Identify "Identify\(\)") function will be called each time an application requests a translation. Both functions should set `count` to the number of elements in the format array.

Important

![Important](./images/admonitions/Warning_32.png)

Unlike the analogous translator add-on format arrays, the arrays returned by these functions don't have to be terminated by an empty translation_format structure.

#### MakeConfigurationView()

##### Optional

`virtual status_t MakeConfigurationView([`BMessage`](BMessage.md "BMessage")* ioExtension,  
[`BView`](BView.md "BView")** outView,  
[`BRect`](BRect.md "BRect")* outExtent);`

Hook function that lets the `BTranslator` supply a configuration view. See [`MakeConfig()`](TranslatorAddOns.md#MakeConfig "MakeConfig\(\)") [Translator Add-ons] for details.

#### Translate()

##### Required

`virtual status_t MakeConfigurationView([`BPositionIO`](BPositionIO.md "BPositionIO")* inSource,  
const translator_info* inInfo,  
[`BMessage`](BMessage.md "BMessage")* ioExtension,  
uint32* outType,  
[`BPositionIO`](BPositionIO.md "BPositionIO")* outDestination) = 0;`

Hook function that asks the `BTranslator` to translate data from `inSource` to format `outType`, writing the output to `outDestination`. See [`Translate()`](BTranslator.md#BTranslator_Translate "Translate\(\)") [Translator Add-ons] for details.

####  TranslatorInfo(), TranslatorName(), TranslatorVersion()

##### Required

`virtual const char * TranslatorInfo() const = 0;``virtual const char * TranslatorName() const = 0;``virtual int32 TranslatorVersion() const = 0;`

`TranslatorInfo()` returns a pointer to the translator's long name, e.g. "aiff translator by the Pie Man (pie@the.man)".

`TranslatorName()` returns a pointer to the translator's short name, e.g. "aiff translator". The short name should be appropriate for display in a menu.

`TranslatorVersion()` gives an "MM.mm" version number for the translator. For example, a `TranslatorVersion()` of 314 is interpreted as version 3.14.

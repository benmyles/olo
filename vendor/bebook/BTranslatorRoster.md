<!-- Classes And Methods - The Translation Kit - BTranslatorRoster -->

## BTranslatorRoster

| Derived From:| –  
---|---  
Mix-in Classes:| [`BArchivable`](BArchivable.md "BArchivable")  
Declared In:| `translation/TranslatorRoster.h`  
Library:| `libtranslation.so`  
Allocation:| –  
[Class Overview](BTranslatorRoster_Overview.md "BTranslatorRoster")

  * [Constructor and Destructor](BTranslatorRoster.md#BTranslatorRoster_ConstructorDestructor)
  * [Member Functions](BTranslatorRoster.md#BTranslatorRoster_MemberFunctions)
  * [Static Functions](BTranslatorRoster.md#BTranslatorRoster_StaticFunctions)
  * [Archived Fields](BTranslatorRoster.md#BTranslatorRoster_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BTranslatorRoster()

`BTranslatorRoster();``BTranslatorRoster([`BMessage`](BMessage.md "BMessage")* model);`

Creates a new container for translators. The no-argument constructor creates an empty container. The [`BMessage`](BMessage.md "BMessage") constructor loads all the translators from the directories specified in the `be:translator_path` named fields in `model`.

If you just want to load the translators from the standard directories, you can instead use the static member [`Default()`](BTranslatorRoster.md#BTranslatorRoster_Default "Default\(\)") described below.

See also: [`AddTranslators()`](BTranslatorRoster.md#BTranslatorRoster_AddTranslators "AddTranslators\(\)")

#### ~BTranslatorRoster()

`~BTranslatorRoster();`

Unloads any translators that were loaded and frees all memory allocated by the `BTranslatorRoster`.

* * *

### Member Functions

#### AddTranslators()

`virtual status_t AddTranslators(const char* load_path = NULL);`

Loads all the translators located in the colon-deliminated list of files and directories found in `load_path`. All specified paths must be absolute. If `load_path` is `NULL`, it loads the translators from the locations specified in the `TRANSLATORS` environment variable. If the environment variable is not defined, then it loads all the files in the default directories `/boot/home/config/add-ons/Translators`, `/boot/home/config/add-ons/Datatypes`, and `/system/add-ons/Translators`.

Return Code| Description  
---|---  
`B_OK`.| Translators added successfully.  
`B_BAD_VALUE`.|  Error parsing `load_path`.  
Anything else.|  Error loading add-ons.  
  
See also: [`Default()`](BTranslatorRoster.md#BTranslatorRoster_Default "Default\(\)") static function

#### Archive()

`virtual status_t Archive([`BMessage`](BMessage.md "BMessage")* into,  
bool* deep = true) const;`

Archives the `BTranslatorRoster` by recording its loaded add-ons in the [`BMessage`](BMessage.md "BMessage") into.

See also: [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)"), [`Instantiate()`](BTranslatorRoster.md#BTranslatorRoster_Instantiate "Instantiate\(\)") static function

#### GetAllTranslators()

`virtual status_t GetAllTranslators(translator_id** outList,  
int32* outCount) const;`

Returns, in `outList`, an array of all the translators loaded by the `BTranslationRoster`. The number of elements in the array is placed in `outCount`. The application assumes responsibility for deallocating the array.

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_NOT_INITIALIZED`.| Internal Translation Kit error.  
`B_BAD_VALUE`.| `outList` or `outCount` is `NULL`.  
  
See also: [`Identify()`](BTranslatorRoster.md#BTranslatorRoster_Identify "Identify\(\)"), [`GetTranslators()`](BTranslatorRoster.md#BTranslatorRoster_GetTranslators "GetTranslators\(\)"), [`GetAllTranslators()`](BTranslatorRoster.md#BTranslatorRoster_GetAllTranslators "GetAllTranslators\(\)")

#### GetConfigurationMessage()

`virtual status_t GetConfigurationMessage(translator_id forTranslator,  
[`BMessage`](BMessage.md "BMessage")* ioExtension);`

Saves the current configuration information for translator `forTranslator` in `ioExtension`. This information may be flattened, unflattened, and passed to [`Translate()`](BTranslatorRoster.md#BTranslatorRoster_Translate "Translate\(\)") to configure it.

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_NO_TRANSLATOR`.| `forTranslator` not a valid translator_id.  
`B_NOT_INITIALIZED`.| Internal Translation Kit error.  
`B_BAD_VALUE`.| `ioExtension` is `NULL`.  
Anything else.| Error passed on from add-on.  
  
See also: [`MakeConfigurationView()`](BTranslatorRoster.md#BTranslatorRoster_MakeConfigurationView "MakeConfigurationView\(\)"), [`Translate()`](BTranslatorRoster.md#BTranslatorRoster_Translate "Translate\(\)")

####  GetInputFormats(), GetOutputFormats()

`virtual status_t GetInputFormats(translator_id forTranslator,  
const translation_format** outFormats,  
int32* outNumFormats);``virtual status_t GetOutputFormats(translator_id forTranslator,  
const translation_format** outFormats,  
int32* outNumFormats);`

Returns an array of the published accepted input or output formats for translator forTranslator. outNumFormats is filled with the number of elements in the array.

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_NO_TRANSLATOR`.| `forTranslator` not a valid translator_id.  
`B_NOT_INITIALIZED`.| Internal Translation Kit error.  
`B_BAD_VALUE`.| `outFormats` or `outNumFormats` is `NULL`.  
  
See also: [`GetTranslatorInfo()`](BTranslatorRoster.md#BTranslatorRoster_GetTranslatorInfo "GetTranslatorInfo\(\)")

#### GetTranslatorInfo()

`virtual status_t GetTranslatorInfo(translator_id forTranslator,  
const char** outName,  
const char** outInfo,  
int32* outNumFormats);`

Returns public information about translator `forTranslator`. Sets `outName` with a short description of the translator, `outInfo` with a longer description, and `outVersion` with the translator's version number.

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_NO_TRANSLATOR`.| `forTranslator` not a valid translator_id.  
`B_NOT_INITIALIZED`.| Internal Translation Kit error.  
  
See also: [`GetInputFormats()`](BTranslatorRoster.md#BTranslatorRoster_GetInputFormats "GetInputFormats\(\), GetOutputFormats\(\)"), [`GetOutputFormats()`](BTranslatorRoster.md#BTranslatorRoster_GetOutputFormats)

#### GetTranslators()

`virtual status_t GetTranslators([`BPositionIO`](BPositionIO.md "BPositionIO")* inSource,  
[`BMessage`](BMessage.md "BMessage")* ioExtension,  
translator_info** outInfo,  
int32* outNumInfo,  
uint32 inHintType = 0,  
const char** inHintMIME = NULL,  
uint32 inWantType = 0);`

Identifies the media in `inSource`, returning an array of valid formats and translators equipped to handle them in `outInfo`. `outNumInfo` holds the number of elements in the array. If `inHintType` or `inHintMIME` is specified, only those translators that can accept data of the specified type are searched. If `inWantType` is specified, only those translators that can output data of that type are searched. `ioExtension` offers an opportunity for the application to specify additional configuration information to the add-ons. The application assumes responsibility for deallocating the array.

Return Code| Description  
---|---  
`B_OK`.| Identification of `inSource` was successful.  
`B_NO_TRANSLATOR`.| No suitable translators found.  
`B_NOT_INITIALIZED`.| Internal Translation Kit error.  
`B_BAD_VALUE`.| `inSource`, `outInfo`, or `outNumInfo` is `NULL`.  
Anything else.| Error operating on `inSource`.  
  
See also: [`Identify()`](BTranslatorRoster.md#BTranslatorRoster_Identify "Identify\(\)"), [`GetAllTranslators()`](BTranslatorRoster.md#BTranslatorRoster_GetAllTranslators "GetAllTranslators\(\)")

#### Identify()

`virtual status_t Identify([`BPositionIO`](BPositionIO.md "BPositionIO")* inSource,  
[`BMessage`](BMessage.md "BMessage")* ioExtension,  
translator_info* outInfo,  
uint32 inHintType = 0,  
const char** inHintMIME = NULL,  
uint32 inWantType = 0);`

Identifies the media in `inSource`, returning a best guess of the format and the translator best equipped to handle it in `outInfo`. If `inHintType` or `inHintMIME` is specified, only those translators that can accept data of the specified type are searched. If `inWantType` is specified, only those translators that can output data of that type are searched. `ioExtension` offers an opportunity for the application to specify additional configuration information to the add-ons.

If more than one translator can identify `inSource`, then the one with the highest quality*capability is returned.

Return Code| Description  
---|---  
`B_OK`.| Identification of `inSource` was successful.  
`B_NO_TRANSLATOR`.| No suitable translator found.  
`B_NOT_INITIALIZED`.| Internal Translation Kit error.  
`B_BAD_VALUE`.| `inSource` or `outInfo` is `NULL`.  
Anything else.| Error operating on `inSource`.  
  
See also: [`GetTranslators()`](BTranslatorRoster.md#BTranslatorRoster_GetTranslators "GetTranslators\(\)"), [`GetAllTranslators()`](BTranslatorRoster.md#BTranslatorRoster_GetAllTranslators "GetAllTranslators\(\)")

#### MakeConfigurationView()

`virtual status_t MakeConfigurationView(translator_id forTranslator,  
[`BMessage`](BMessage.md "BMessage")* ioExtension,  
[`BView`](BView.md "BView")** outView,  
[`BRect`](BRect.md "BRect")* outExtent);`

Returns, in `outView`, a [`BView`](BView.md "BView") containing controls to configure translator `forTranslator`. It is the application's responsibility to attach the [`BView`](BView.md "BView") to a [`BWindow`](BWindow.md "BWindow"). The initial size of the [`BView`](BView.md "BView") is given in `outExtent` but may be resized by the application.

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_NO_TRANSLATOR`.| `forTranslator` not a valid translator_id.  
`B_NOT_INITIALIZED`.| Internal Translation Kit error.  
`B_BAD_VALUE`.| `outView` or `outExtent` is `NULL`.  
Anything else.| Error passed on from add-on.  
  
See also: [`GetConfigurationMessage()`](BTranslatorRoster.md#BTranslatorRoster_GetConfigurationMessage "GetConfigurationMessage\(\)"), [`Translate()`](BTranslatorRoster.md#BTranslatorRoster_Translate "Translate\(\)")

#### Translate()

`virtual status_t Translate([`BPositionIO`](BPositionIO.md "BPositionIO")* inSource,  
const translator_info* inInfo,  
[`BMessage`](BMessage.md "BMessage")* ioExtension,  
[`BPositionIO`](BPositionIO.md "BPositionIO")* outDestination,  
uint32 inWantOutType,  
uint32 inHintType = 0,  
const char* inHintMIME = NULL);` `virtual status_t Translate(translator_id inTranslator,  
[`BPositionIO`](BPositionIO.md "BPositionIO")* inSource,  
[`BMessage`](BMessage.md "BMessage")* ioExtension,  
[`BPositionIO`](BPositionIO.md "BPositionIO")* outDestination,  
uint32 inWantOutType);`

These two functions carry out data conversion, converting the data in `inSource` to type `inWantoutType` and placing the resulting output in `outDestination`. `inInfo` should always contain either the output of an [`Identify()`](BTranslatorRoster.md#BTranslatorRoster_Identify "Identify\(\)") call or `NULL`. The translation uses the translator identified by `inInfo`->`infoTranslator` or `inTranslator` as appropriate. If `inInfo` is `NULL`, `Translate()` will call first [`Identify()`](BTranslatorRoster.md#BTranslatorRoster_Identify "Identify\(\)") to discover the format of the input stream. `ioExtension`, if it is not `NULL`, provides a communication path between the translator and the application. `inHintType` and `inHintMIME`, if provided, are passed as hints to the translator.

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_NO_TRANSLATOR`.| No suitable translators found.  
`B_NOT_INITIALIZED`.| Internal Translation Kit error.  
`B_BAD_VALUE`.| `inSource` or `outSource` is `NULL`.  
Anything else.| Error passed on from add-on.  
  
* * *

### Static Functions

#### Default()

`BTranslatorRoster* Default();`

This returns a `BTranslatorRoster` loaded with the default set of translators, loaded from the colon-deliminated list of files and directories found in the environment variable `TRANSLATORS`. If no such variable exists, the translators are loaded from the default locations `/boot/home/config/add-ons/Translators`, `/boot/home/config/add-ons/Datatypes`, and `/system/add-ons/Translators`.

The instance of `BTranslatorRoster` returned by this function is global to the application and should not be deleted. Although you can add translators to the returned `BTranslatorRoster`, this is not recommended. It's better to use a new instance of the class.

See also: [`AddTranslators()`](BTranslatorRoster.md#BTranslatorRoster_AddTranslators "AddTranslators\(\)")

#### Instantiate()

`BTranslatorRoster* Instantiate([`BMessage`](BMessage.md "BMessage")* from);`

Returns a new `BTranslatorRoster` object, allocated by new and created with the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive. However, if the archive doesn't contain data for a `BTranslatorRoster` object, `Instantiate()` returns `NULL`.

See also: [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"), [`Archive()`](BTranslatorRoster.md#BTranslatorRoster_Archive "Archive\(\)")

#### Version()

`const char* Version(int32* outCurVersion,  
int32* outMinVersion,  
int32* inAppVersion = B_TRANSLATION_CURRENT_VERSION);`

Sets `outCurVersion` to the Translation Kit protocol version number and `outMinVersion` to the minimum protocol version number supported. Returns a human-readable string containing version information. Currently, `inAppVersion` must be `B_TRANSLATION_CURRENT_VERSION`.

* * *

### Archived Fields

The [`Archive()`](BTranslatorRoster.md#BTranslatorRoster_Archive "Archive\(\)") function adds the following field to its [`BMessage`](BMessage.md "BMessage") argument:

Field| Type code| Description  
---|---|---  
`be:translator_path` (array)| `B_STRING_TYPE`| The location of the Translation Kit add-on (one field per add-on).

<!-- Classes And Methods - The Translation Kit - Translator Add-Ons -->

## Translator Add-Ons

Declared In:| `translation/TranslatorAddOn.h`  
---|---  
Library:| `libtranslation.so`  
  
This section describes the functions and data that a translator add-on must (or can) supply. The translator add-ons that you create and install are loaded into a [`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") object through its [`Default()`](BTranslatorRoster.md#BTranslatorRoster_Default "Default\(\)") or [`AddTranslators()`](BTranslatorRoster.md#BTranslatorRoster_AddTranslators "AddTranslators\(\)") function.

You compile your add-on as a shared library, link against `libtranslation.so`, and install the add-on object file in the Translators subdirectory of `B_USER_ADDONS_DIRECTORY`.

The table below briefly lists the translator add-on symbols:

Symbol| Required?| Description  
---|---|---  
`translatorInfo`| yes| Long description of translator  
`translatorName`| yes| Short name of translator  
`translatorVersion`| yes| Translator version number  
`Identify()`| yes| Identify data in a BPositionIO  
`Translate()`| yes| Translate data from one format to another  
`inputFormats`| no| List of supported input formats  
`outputFormats`| no| List of supported output formats  
`GetConfigMessage()`| no| Save current configuration in a BMessage  
`MakeConfig()`| no| Create a view for user-configuration of translator  
  
* * *

### Translator Add-on Functions

#### GetConfigMessage()

##### Optional

`status_t GetConfigMessage([`BMessage`](BMessage.md "BMessage")* ioExtension);`

This function stores the current configuration in the `ioExtension` in such a manner that it may be flattened, unflattened, and then passed to [`Translate()`](TranslatorAddOns.md#Translate "Translate\(\)") as an `ioExtension`.

#### Identify()

##### Required

`status_t Identify([`BPositionIO`](BPositionIO.md "BPositionIO")* inSource,  
const translation_format* inFormat,  
[`BMessage`](BMessage.md "BMessage")* ioExtension,  
translator_info* outInfo,  
uint32 outType);`

If the translator understands how to convert the data contained in `inSource` to media type `outType`, it fills `outInfo` with details about the input format and return `B_OK`. If it doesn't know how to translate the data, it returns `B_NO_TRANSLATOR`.

The `quality` and `capability` fields in `outInfo` are used by the Translation Kit in selecting the best suited translator for a given translation, so it is best to be conservative in choosing these values.

If the media format doesn't have a type code in `support/TypeConstants.h` (as will most likely be the case), choose a reasonable value. For example, the Targa handler included with the BeOS distribution uses the type code 'TGA '.

The translator need not fill in the `translator` field in `outInfo`; [`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") fills in this value for you.

Remember that `inSource` may be arriving from any source, including a live network feed. It's therefore best to steer clear of calls such as [`BPositionIO::SetSize()`](BPositionIO.md#BPositionIO_SetSize "SetSize\(\)") which force all the data out of the stream. Similarly, it is good practice to read only as much of the stream as you need.

`inFormat`, if it is not `NULL`, is provided as a hint to the format of the data in `inSource`. Since it is only a hint, the data may very well be in some other format.

`ioExtension`, if it is not `NULL`, contains additional information for the add-on. It is described at length in the section in [BTranslatorRoster Overview](BTranslatorRoster_Overview.md "BTranslatorRoster") titled "[Configuration](BTranslatorRoster_Overview.md#BTranslatorRoster_Overview_Configuration "Configuration")".

`outType` may be zero, in which case any output format is acceptable.

#### MakeConfig()

##### Optional

`status_t MakeConfig([`BMessage`](BMessage.md "BMessage")* ioExtension,  
[`BView`](BView.md "BView")** outView,  
[`BRect`](BRect.md "BRect")* outExtent);`

This function creates a new [`BView`](BView.md "BView") through which the user configures the add-on. For example, it could be used to control the degree of image compression used or the video frame rate. The bounds of the view are returned in outExtent, although it can be resized at will by an external source. Changes to the configuration take effect immediately, although translations should be carried out with the same parameters throughout.

#### Translate()

##### Required

`status_t Translate([`BPositionIO`](BPositionIO.md "BPositionIO")* inSource,  
const translator_info* inInfo,  
[`BMessage`](BMessage.md "BMessage")* ioExtension,  
uint32 outType,  
[`BPositionIO`](BPositionIO.md "BPositionIO")* outDestination);`

The translator translates data from `inSource` to format `outType`, writing the output to `outDestination`. `outType` may be zero, in which case it is assumed to be the default format type for the media group. As in [`Identify()`](TranslatorAddOns.md#Identify "Identify\(\)"), `inInfo` serves as a hint to the format of the data in `inSource`. `ioExtension` fills its usual role as a container of configuration information. The function returns `B_OK` if it's able to convert the data successfully. If it's unable to do so, it returns either `B_NO_TRANSLATOR` or an error value as appropriate.

* * *

### Translator Add-on Data

####  inputFormats, outputFormats

##### Optional
    
    
    translation_format inputFormats[];
    translation_format outputFormats[];

These arrays tell [`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") which formats the add-on supports. If they are not exported by the translator, the add-on's [`Identify()`](TranslatorAddOns.md#Identify "Identify\(\)") will be called each time an application requests a translation. Each array ends in an empty translation_format structure, so a typical `inputFormats` would look like:
    
    
    translation_format inputFormats[] = {
    { 'TGA ', B_TRANSLATOR_BITMAP, 0.6, 0.5, "image/targa",
    "Targa bitmap format" },
    { B_TRANSLATOR_BITMAP, B_TRANSLATOR_BITMAP, 0.4, 0.6, "image/x-be-bitmap",
    "Be Bitmap format" },
    { 0, 0, 0, 0, 0, 0 }
    };

and similarly for `outputFormats`.

####  translatorInfo, translatorName, translatorVersion

##### Required
    
    
    char translatorInfo[];
    char translatorName[];
    int32 translatorVersion;

Field| Description  
---|---  
`translatorInfo`| Returns a pointer to the translator's long name, e.g. "aiff translator by the Pie Man (pie@the.man)".  
`translatorName`| Returns a pointer to the translator's short name, e.g. "aiff translator". The short name should be appropriate for display in a menu.  
`translatorVersion`| Gives an "MM.mm" version number for the translator. For example, a `translatorVersion` of 314 is interpreted as version 3.14.

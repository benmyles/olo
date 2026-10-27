<!-- System Overview - The Translation Kit - BTranslatorRoster -->

## BTranslatorRoster  
  
[`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") is the main mechanism through which applications interact with the Translation Kit. An application using the Translation Kit doesn't need to worry about explicitly loading or calling add-ons; [`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") transparently handles all the niggling details. The class provides four categories of service: initialization, information, translation, and configuration.

* * *

### Initialization

You can create an empty [`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") (one which has no translators loaded) by instantiating it in the usual fashion:
    
    
    BTranslatorRoster *roster = new BTranslatorRoster;

You can then load translators to the newly-created [`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") with the [`AddTranslators()`](BTranslatorRoster.md#BTranslatorRoster_AddTranslators "AddTranslators\(\)") method (all paths must be absolute):
    
    
    // load all translators in a given directory
    roster->AddTranslators("/boot/home/config/add-ons/viewer/");
    
    // load a specific translator
    roster->AddTranslators("/system/add-ons/Translators/photos");

More commonly, you want a [`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") with the default translators (those in `/boot/home/config/add-ons/Translators`, `/boot/home/config/add-ons/Datatypes`, and `/system/add-ons/Translators`). The static member [`Default()`](BTranslatorRoster.md#BTranslatorRoster_Default "Default\(\)") returns just such a beast:
    
    
    BTranslatorRoster *roster = BTranslatorRoster::Default();

However, not al [`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster")s are created equal: the object returned by [`Default()`](BTranslatorRoster.md#BTranslatorRoster_Default "Default\(\)") is global to the application and controlled by [`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster"). As a result, you should never delete it. Additionally, if you want to load additional translators, you're better off creating a new instance of [`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") rather than using the default one:
    
    
    BTranslatorRoster *roster = new BTranslatorRoster;
    roster->AddTranslators(NULL); // load default translators
    roster->AddTranslators(...); // load additional translators

* * *

### Information

Applications typically ask the following questions of the Translation Kit:

  1. Which translators are installed?

  2. Which translations can a particular translator carry out?

  3. Which translator is best suited for handling my conversion?

An application can determine the installed translators by calling [`GetAllTranslators()`](BTranslatorRoster.md#BTranslatorRoster_GetAllTranslators "GetAllTranslators\(\)"). [`GetAllTranslators()`](BTranslatorRoster.md#BTranslatorRoster_GetAllTranslators "GetAllTranslators\(\)") returns an array of translator_id values for the installed translators. A translator_id is an application-wide value assigned by [`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") identifying a specific translator. The following snippet prints out the names of the default translators:
    
    
    BTranslatorRoster *roster = BTranslatorRoster::Default();
    int32 num_translators, i;
    translator_id *translators;
    const char *translator_name, *translator_info;
    int32 translator_version;
    
    roster->GetAllTranslators(&translators, &num_translators);
    for (i=0;i<num_translators;i++) {
       roster->GetTranslatorInfo(translators[i], &translator_name,
          &translator_info, &translator_version);
       printf("%s: %s (%.2f)n", translator_name, translator_info,
          translator_version/100.);
    }
    
    delete [] translators; // clean up our droppings

The translator_id is very valuable; it can be used to query [`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") for specific information about a translator's capabilities. This information comes in two nearly identical flavors: translation_format and translator_info. They are defined in `translation/TranslationDefs.h`:
    
    
    struct translation_format {
        uint32 type;
        uint32 group;
        float quality;
        float capability;
        char MIME[251];
        char name[251];
    }
    
    struct translator_info {
        uint32 type;
        translator_id translator;
        uint32 group;
        float quality;
        float capability;
        char name[251];
        char MIME[251];
    }

The common fields:

Field| Description  
---|---  
`group`| Defines the type of media the format represents, i.e. bitmap image, sound, or video. Constants for common media types can be found in `translation/TranslatorFormats.h`.  
`type`| Type constant defining the specific data format. For example, the type constant for tiff bitmaps differs from the constant for jpeg bitmaps.  
`quality`| Ability of the format to represent data of its media group. This value ranges from a low of 0.0 (utter inability) to a high of 1.0 (encodes all relevant information).  
`capability`| Ability of translator to decode the format. As with the quality, the value ranges from 0.0 (unable to decode) to 1.0 (can decode all variants and extensions).  
`MIME`| MIME type of the format. This is a more reliable indicator of the data format than the type field for those formats that have standard MIME names.  
`name`| Human-readable C string describing the format. May include information about the translator as well.  
  
translator_info defines an additional field:

Field| Description  
---|---  
`translator`| translator_id for the translator associated with the structure.  
  
To find the media formats supported by a particular translator, call [`GetInputFormats()`](BTranslatorRoster.md#BTranslatorRoster_GetInputFormats "GetInputFormats\(\), GetOutputFormats\(\)") or [`GetOutputFormats()`](BTranslatorRoster.md#BTranslatorRoster_GetOutputFormats) as appropriate. These methods return a list of the supported input or output formats in an array of translation_format.

Many times, however, you don't really care about individual translators; you just want the translator best suited for handling your media stream. In these cases, you can just call [`Identify()`](BTranslatorRoster.md#BTranslatorRoster_Identify "Identify\(\)") on the [`BPositionIO`](BPositionIO.md "BPositionIO"). [`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") will then return, in a translator_info, the translator most suited for carrying out the translation. If you are instead interested in finding all the translators capable of handling the data in your stream, use [`GetTranslators()`](BTranslatorRoster.md#BTranslatorRoster_GetTranslators "GetTranslators\(\)") instead.

Note that some translators do not publish their input and output formats. In these cases, [`GetInputFormats()`](BTranslatorRoster.md#BTranslatorRoster_GetInputFormats "GetInputFormats\(\), GetOutputFormats\(\)") and [`GetOutputFormats()`](BTranslatorRoster.md#BTranslatorRoster_GetOutputFormats) return an empty list of formats. The only way to tell if such a translator supports a particular input or output format is to pass it to [`Identify()`](BTranslatorRoster.md#BTranslatorRoster_Identify "Identify\(\)").

* * *

### Translation

Although the function of [`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") is to provide translation services, carrying out the translation is simple. All it requires are the input and output [`BPositionIO`](BPositionIO.md "BPositionIO") streams and the type constant for the desired output. In the simplest case, if you know the type constant you want to convert the data into, you can let the [`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") decide which translator to use:
    
    
    BPositionIO *in = ..., *out = ...;
    BTranslatorRoster *roster = BTranslatorRoster::Default();
    uint32 desired_format_constant = ...;
    
    roster->Translate(in, NULL, NULL, out, desired_format_constant);

Sometimes, however, you'd like the services of a specific translator. In these cases, you can use the alternate form of [`Translate()`](BTranslatorRoster.md#BTranslatorRoster_Translate "Translate\(\)"):
    
    
    BPositionIO *in = ..., *out = ...;
    BTranslatorRoster *roster = BTranslatorRoster::Default();
    uint32 desired_format_constant = ...;
    translator_id desired_translator_id = ...;
    
    roster->Translate(desired_translator_id, in, NULL, NULL, out,
       desired_format_constant);

Note

![Note](./images/admonitions/Info_32.png)

The Translation Kit won't chain translators for you. If you want to translate from GIF to PNG, you either need a translator that can convert GIF to PNG, or you need to perform two translations: one from GIF to an interchange format, then another from that format to PNG.

* * *

### Configuration

[`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") provides two mechanisms for configuring the behavior of translators: `ioExtension` and [`MakeConfigurationView()`](BTranslatorRoster.md#BTranslatorRoster_MakeConfigurationView "MakeConfigurationView\(\)").

`ioExtension` is a [`BMessage`](BMessage.md "BMessage") that can be passed to most [`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") members. It is used by the application to communicate format-specific information to the translator. For example, it could be used to ask a video translator to only translate the first 15 frames of the movie. The `ioExtension` field is also used by the translator to communicate information back to the application. The translator may use it, for example, to tell the application that it is returning a greyscale image.

A translator need not support any particular extension and there is no way for an application to tell if a translator supports any extensions.

A set of standard `ioExtension` messages can be found in `translation/TranslatorFormats.h` and are explained below:

Name| Type| Direction| Description  
---|---|---|---  
`B_TRANSLATOR_EXT_HEADER_ONLY`| bool| app to translator| Only output header if `true`  
`B_TRANSLATOR_EXT_DATA_ONLY`| bool| app to translator| Only output data if `true`  
`B_TRANSLATOR_EXT_COMMENT`| string| n/a| Text comment about data.  
`B_TRANSLATOR_EXT_TIME`| bigtime_t| app to translator| If one exists, it specifies a single instant in time. If two exist, they specify a range in time. Time is measured in microseconds.  
`B_TRANSLATOR_EXT_FRAME`| uint32| app to translator| Same as `kTimeExtension`, except the unit of time is frames.  
`B_TRANSLATOR_EXT_BITMAP_RECT`| `BRect`| app to translator| Specifies subsection of a bitmap.  
`B_TRANSLATOR_EXT_BITMAP_COLOR_SPACE`| uint32| both| Specifies desired/actual color space of bitmap.  
`B_TRANSLATOR_EXT_SOUND_CHANNEL`| uint32| app to translator| Only output specified channel of sound.  
`B_TRANSLATOR_EXT_SOUND_MONO`| bool| app to translator| Mix all audio channels to a single mono channel if `true`.  
`B_TRANSLATOR_EXT_SOUND_MARKER`| uint32| both| Specifies markers in sound data. Units are in frames. "1" specifies marker between first sample of last channel and second sample of first channel.  
`B_TRANSLATOR_EXT_SOUND_LOOP`| uint32| both| If there is one value, loop from the end of the sample to the loop point. If there are two values, loop from the second value to the first. If there are more than two values, then there is a release loop.  
  
[`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") contains two methods to facilitate configuration of translators. The first method, [`MakeConfigurationView()`](BTranslatorRoster.md#BTranslatorRoster_MakeConfigurationView "MakeConfigurationView\(\)"), instructs the translator to create a [`BView`](BView.md "BView") in which configuration options may be changed. It is the responsibility of the application calling [`MakeConfigurationView()`](BTranslatorRoster.md#BTranslatorRoster_MakeConfigurationView "MakeConfigurationView\(\)") to attach the view to a [`BWindow`](BWindow.md "BWindow"). The second method, [`GetConfigurationMessage()`](BTranslatorRoster.md#BTranslatorRoster_GetConfigurationMessage "GetConfigurationMessage\(\)"), fills in a [`BMessage`](BMessage.md "BMessage") with the current settings for a specified translator. This [`BMessage`](BMessage.md "BMessage") can then be passed to [`Translate()`](BTranslatorRoster.md#BTranslatorRoster_Translate "Translate\(\)") to request a translation with the settings contained within.

* * *

### Example

The task of translating data often boils down to finding the right type constant. The following function will print the format type constants associated with a MIME string given a [`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster"):
    
    
    void find_constant(BTranslatorRoster *roster, const char *mime)
    {
       translator_id *translators;
       int32 num_translators;
    
       roster->GetAllTranslators(&translators, &num_translators);
    
       for (int32 i=0;i<num_translators;i++) {
          const translation_format *fmts;
          int32 num_fmts;
    
          roster->GetOutputFormats(translators[i], &fmts, &num_fmts);
    
          for (int32 j=0;j<num_fmts;j++) {
             if (!strcasecmp(fmts[j].MIME, mime))
                printf("match: %s type %8.8x (%4.4s)n",
                   fmts[j].name, fmts[j].type, &fmts[j].type);
          }
       }
    }

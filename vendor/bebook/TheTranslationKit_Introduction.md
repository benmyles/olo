<!-- System Overview - The Translation Kit - Introduction -->

## Introduction

The Translation Kit provides a framework for converting data streams between different media formats. A word processor, for example, could use the Translation Kit to import and export documents in a variety of formats, including HTML, PostScript, and plain ASCII, while working in its own native format.

The translation itself is performed by translator add-ons (or, less globally, [`BTranslator`](BTranslator.md "BTranslator") objects). You can create your own translator add-ons and [`BTranslator`](BTranslator.md "BTranslator") subclasses, or you can use the translators that are supplied by Be and third parties.

The Translation Kit operates on [`BPositionIO`](BPositionIO.md "BPositionIO") objects and their descendents. Since [`BFile`](BFile.md "BFile") derives from [`BPositionIO`](BPositionIO.md "BPositionIO"), the Translation Kit is naturally suited for importing and exporting files. However, it's far more general; the Translation Kit could be used, for example, to translate input directly to or from a network connection, provided a suitable [`BPositionIO`](BPositionIO.md "BPositionIO") subclass was available. More importantly, [`BMemoryIO`](BMemoryIO.md "BMemoryIO") and [`BMallocIO`](BMallocIO.md "BMallocIO") can be used to load data directly into or out of memory.

Applications primarily interact with the Translation Kit through the [`BTranslatorRoster`](BTranslatorRoster.md "BTranslatorRoster") class. This class encapsulates the functionality required to load and unload translators, discover their capabilities, configure the translators, and execute the translations.

Applications may also use the [`BTranslationUtils`](BTranslationUtils.md "BTranslationUtils") class, a collection of static utility functions designed to simplify access to the Translation Kit. Currently the class only defines members for loading bitmap images from files, resources, and general [`BPositionIO`](BPositionIO.md "BPositionIO") data streams.

* * *

### Media Formats

The Translation Kit identifies a particular media format by a media group and format type. Media groups identify the class of media represented by a format. Definitions for the common media groups can be found in `translation/TranslatorFormats.h`:

Media Group| Constant  
---|---  
Bitmap| `B_TRANSLATOR_BITMAP`  
`BPicture` data| `B_TRANSLATOR_PICTURE`  
Text| `B_TRANSLATOR_TEXT`  
Sound| `B_TRANSLATOR_SOUND`  
Standard MIDI| `B_TRANSLATOR_MIDI`  
Streaming media| `B_TRANSLATOR_MEDIA`  
nothing| `B_TRANSLATOR_NONE`  
  
A format type is a type constant identifying the specific data format, i.e. tiff, aiff, mpeg. The standard type constants can be found in `support/TypeConstants.h`.

It is often convenient to define a baseline format to which all translators of a given media group adhere. An application that understands the baseline format could then use all the translators for that group. This also simplifies the job of the translator, which minimally needs only to translate between the baseline and the new formats. Well-behaved translators will always be able to translate to and from the default media format (if one exists for its media group).

The Translation Kit defines a default media format for each existing media group. They are explained at length in the section "[Default Media Formats](TheTranslationKit_DefaultMediaFormats.md "Default Media Formats")". The format type constant for each default format is the same as the media group constant; for example, the format type for the default bitmap format is `B_TRANSLATOR_BITMAP`.

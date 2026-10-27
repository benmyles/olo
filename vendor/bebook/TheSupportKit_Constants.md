<!-- Classes And Methods - The Support Kit - Constants -->

## Constants

This section lists the constants defined by the Support Kit and used throughout the entire Be operating system. Not included here are constants used as status_t values (error codes). They're listed in "[Error Codes](TheSupportKit_Error_Codes.md "Error Codes")".

* * *

### Boolean Constants

Declared in: `support/SupportDefs.h`

Constant| Description  
---|---  
`false`| 0  
`true`| 1  
  
These constants are defined as values for the bool type (described in the next section). The BeOS defines them for C code only. Because they match the boolean symbols that are part of the C++ language, they let you use the same bool type and true and false values when programming in both languages.

See also: [bool](TheSupportKit_CDT.md#bool "bool")

* * *

###  Byte Swapping Constants

Declared in: `support/ByteOrder.h`

Constant| Description  
---|---  
`B_SWAP_HOST_TO_LENDIAN`| Convert from host byte order to little endian  
`B_SWAP_HOST_TO_BENDIAN`| Convert from host byte order to big endian  
`B_SWAP_LENDIAN_TO_HOST`| Convert from little endian to host byte order  
`B_SWAP_BENDIAN_TO_HOST`| Convert from big endian to host byte order  
`B_SWAP_ALWAYS`| Always convert endianness  
  
These constants define the action to be taken by the [`swap_data()`](TheSupportKit_Functions.md#swap_data "swap_data\(\), is_type_swapped\(\)") general byte swapping function.

* * *

###  Encoding Conversion Constants

Declared in: `support/UTF8.h`

`B_ISO1_CONVERSION`| `B_MAC_ROMAN_CONVERSION`  
---|---  
`B_ISO2_CONVERSION`| `B_SJIS_CONVERSION`  
`B_ISO3_CONVERSION`| `B_EUC_CONVERSION`  
`B_ISO4_CONVERSION`| `B_JIS_CONVERSION`  
`B_ISO5_CONVERSION`| `B_MS_WINDOWS_CONVERSION`  
`B_ISO6_CONVERSION`| `B_UNICODE_CONVERSION`  
`B_ISO7_CONVERSION`| `B_KOI8R_CONVERSION`  
`B_ISO8_CONVERSION`| `B_MS_WINDOWS_1251_CONVERSION`  
`B_ISO9_CONVERSION`| `B_MS_DOS_866_CONVERSION`  
`B_ISO10_CONVERSION`| ` `  
  
These constants identify character encodings to the [`convert_to_utf8()`](TheSupportKit_Functions.md#convert_to_utf8 "convert_to_utf8\(\), convert_from_utf8\(\)") and [`convert_from_utf8()`](TheSupportKit_Functions.md#convert_from_utf8) functions, which convert text to and from the standard UTF8 character encoding assumed by the BeOS. The identify the source encoding for a conversion to UTF-8 and the destination encoding for a conversion from UTF-8.

See also: The [`BFont`](BFont.md "BFont") class in the Interface Kit

* * *

### Empty String

Declared in: `support/SupportDefs.h`
    
    
    const char* B_EMPTY_STRING

This constant provides a global pointer to an empty string ("").

* * *

###  URL MIME Types

Declared in: `support/TypeConstants.h`

Constant| Description  
---|---  
`B_URL_HTTP`| An HTTP (HyperText Transport Protocol) link  
`B_URL_HTTPS`| An HTTPS (HyperText Transport Protocol Secure) link  
`B_URL_FTP`| An FTP (File Transfer Protocol) link  
`B_URL_GOPHER`| A Gopher link  
`B_URL_MAILTO`| A mailto (email) link  
`B_URL_NEWS`| A news (usenet) link  
`B_URL_NNTP`| An NNTP (network time protocol) link  
`B_URL_TELNET`| A telnet link  
`B_URL_RLOGIN`| An rlogin link  
`B_URL_TN3270`| A TN3270 link  
`B_URL_WAIS`| A WAIS (Wide Area Information Services) link  
`B_URL_FILE`| A local file link  
  
These are MIME type strings for various URL types. An application can register itself as a handler for one of these types. Once that's done, clicking a link of that type in NetPositive will cause that application to be launched, with a `B_ARGV_RECEIVED` message with the full URL as the second argument.

* * *

###  NULL and NIL

Declared in: `support/SupportDefs.h`

Constant| Value  
---|---  
`NIL`| 0  
`NULL`| 0  
  
These constants represent "empty" values. They're synonyms that can be used interchangeably.

* * *

###  Type Codes

Declared in: `support/TypeConstants.h`

Constant| Description  
---|---  
`B_CHAR_TYPE`| A single character  
`B_INT8_TYPE`| An 8-bit integer  
`B_INT16_TYPE`| A 16-bit integer  
`B_INT32_TYPE`| A 32-bit integer  
`B_INT64_TYPE`| A 64-bit integer  
`B_UINT8_TYPE`| An unsigned 8-bit integer  
`B_UINT16_TYPE`| An unsigned 16-bit integer  
`B_UINT32_TYPE`| An unsigned 32-bit integer  
`B_UINT64_TYPE`| An unsigned 64-bit integer  
`B_FLOAT_TYPE`| A float  
`B_DOUBLE_TYPE`| A double  
`B_BOOL_TYPE`| A boolean value (the bool type)  
`B_OFF_T_TYPE`| An off_t value  
`B_SIZE_T_TYPE`| A size_t value  
`B_SSIZE_T_TYPE`| An ssize_t value  
`B_POINTER_TYPE`| A pointer of some kind (including void*)  
`B_OBJECT_TYPE`| An object pointer (such as BMessage*)  
`B_MESSAGE_TYPE`| A [`BMessage`](BMessage.md "BMessage") object (not BMessage*)  
`B_MESSENGER_TYPE`| A [`BMessenger`](BMessenger.md "BMessenger") object  
`B_POINT_TYPE`| A [`BPoint`](BPoint.md "BPoint") object  
`B_RECT_TYPE`| A [`BRect`](BRect.md "BRect") object  
`B_PATH_TYPE`| A [`BPath`](BPath.md "BPath") object  
`B_REF_TYPE`| An [entry_ref](BEntry.md#entry_ref "entry_ref") structure  
`B_RGB_COLOR_TYPE`| An [rgb_color](TheInterfaceKit_DefinedTypes.md#rgb_color "rgb_color") structure  
`B_PATTERN_TYPE`| A [pattern](TheInterfaceKit_DefinedTypes.md#pattern "pattern") structure  
`B_STRING_TYPE`| A null-terminated character string  
`B_MONOCHROME_1_BIT_TYPE`| Raw data for a monochrome bitmap (1 bit/pixel)  
`B_GRAYSCALE_8_BIT_TYPE`| Raw data for a grayscale bitmap (8 bits per pixel)  
`B_COLOR_8_BIT_TYPE`| Raw bitmap data in the [`B_COLOR_8_BIT`](TheInterfaceKit_GeneralConstants.md#color_space "color_space") color space  
`B_RGB_32_BIT_TYPE`| Raw bitmap data in the [`B_RGB_32_BIT`](TheInterfaceKit_GeneralConstants.md#color_space "color_space") color space  
`B_TIME_TYPE`| 64-bit microsecond time_t data  
`B_MEDIA_PARAMETER_TYPE`| A Media Kit parameter.  
`B_MEDIA_PARAMETER_WEB_TYPE`| A Media Kit parameter web.  
`B_MEDIA_PARAMETER_GROUP_TYPE`| A Media Kit parameter group.  
`B_RAW_TYPE`| Raw, untyped data—a stream of bytes  
`B_MIME_TYPE`| The type is specified by a MIME string.  
`B_ANY_TYPE`| The type can be any type.  
  
These constants describe the types of data held by [`BMessage`](BMessage.md "BMessage") objects (the Application Kit) and as resources and file system attributes (the Storage Kit). `B_ANY_TYPE` refers to all types; it indicates that the exact type doesn't matter. `B_MIME_TYPE` indicates that the name of the data in the BMessage is a MIME string that specifies its true data type. The other constants refer only to a particular type. The type_code defined type marks where these constants are used in the API.

Applications can define their own type codes for data types not found on this list. All the codes the BeOS defines have values formed by concatenating four uppercase letters into a multicharacter constant. For example, `B_MESSENGER_TYPE` is 'MSNG' and `B_SIZE_T_TYPE` is 'SIZT'.

See also: [type_code](TheSupportKit_CDT.md#type_code "type_code")

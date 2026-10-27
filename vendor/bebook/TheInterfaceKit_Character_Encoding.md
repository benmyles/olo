<!-- System Overview - The Interface Kit - Character Encoding -->

## Character Encoding

The BeOS encodes characters using the UTF-8 transformation of Unicode character values. Unicode is a standard encoding scheme for all the major scripts of the world—including, among others, extended Latin, Cyrillic, Greek, Devanagiri, Telugu, Hebrew, Arabic, Tibetan, and the various character sets used by Chinese, Japanese, and Korean. It assigns a unique and unambiguous 16-bit value to each character, making it possible for characters from various languages to co-exist in the same document. Unicode makes it simpler to write language-aware software (though it doesn't solve all the problems). It also makes a wide variety of symbols available to an application, even if it's not concerned with covering more than one language.

Unicode's one disadvantage is that all characters have a width of 16 bits. Although 16 bits are necessary for a universal encoding system and a fixed width for all characters is important for the standard, there are many contexts in which byte-sized characters would be easier to work with and take up less memory (besides being more familiar and backwards compatible with existing code). UTF-8 is designed to address this problem.

* * *

### UTF-8

UTF-8 stands for "UCS Transformation Format, 8-bit form" (and UCS stands for "Universal Multiple-Octet Character Set," another name for Unicode). UTF-8 transforms 16-bit Unicode values into a variable number of 8-bit units. It takes advantage of the fact that for values equal to or less than 0x007f, the Unicode character set matches the 7-bit ASCII character set—in other words, Unicode adopts the ASCII standard, but encodes each character in 16 bits. UTF-8 strips ASCII values back to 8 bits and uses two or three bytes to encode Unicode values over 0x007f.

The high bit of each UTF-8 byte indicates the role it plays in the encoding:

  * If the high bit is 0, the byte stands alone and encodes an ASCII value.

  * If the high bit is 1, the byte is part of a multiple-byte character representation.

In addition, the first byte of a multibyte character indicates how many bytes are in the encoding: The number of high bits that are set to 1 (before a bit is 0) is the number of bytes it takes to represent the character. Therefore, the first byte of a multibyte character will always have at least two high bits set. The other bytes in a multibyte encoding have just one high bit set.

To illustrate, a character encoded in one UTF-8 byte will look like this (where a '1' or a '0' indicates a control bit specified by the standard and an 'x' is a bit that contributes to the character value):
    
    
    0xxxxxxx
    

A character encoded in two bytes has the following arrangement of bits:
    
    
    110xxxxx 10xxxxxx
    

And a character encoded in three bytes is laid out as follows:
    
    
    1110xxxx 10xxxxxx 10xxxxxx
    

Note that any 16-bit value can be encoded in three UTF-8 bytes. However, UTF-8 discards leading zeroes and always uses the fewest possible number of bytes—so it can encode Unicode values less than 0x0080 in a single byte and values less than 0x0800 in two bytes.

In addition to the codings illustrated above, UTF-8 takes four bytes to translate a Unicode surrogate pair—two conjoined 16-bit values that together encode a character that's not part of the standard. Surrogates are extremely rare.

* * *

### ASCII Compatibility

The UTF-8 encoding scheme has several advantages:

  * The single byte that encodes an ASCII value can't be confused with a byte that's part of a multiple-byte encoding. You can test a UTF-8 byte for an ASCII value without considering surrounding bytes; if there's a match, you can be sure the byte is the ASCII character. UTF-8 is fully compatible with ASCII.

  * The first (or only) byte of a character can't be confused with a byte inside a multibyte sequence. It's simple to find where a character begins. For example, this macro will do it:
        
        #define BEGINS_CHAR(byte) ((byte & 0xc0) != 0x80)

  * The string functions in the standard C library—for example, strcat() and strlen()—can operate on a UTF-8 string.

  * However, it's important to remember that strlen() measures the string in bytes, not characters. Some Interface Kit functions, like GetEscapements() in the BFont class, ask for a character count; strlen() can't provide the answer. Instead, you need to do something like this to count the characters in a string:
        
        int32 count = 0;
        while ( *p != '0' ) {
           if ( BEGINS_CHAR(*p) )
              count++;
           p++;
        }

  * UTF-8 preserves the numerical ordering of Unicode character values. String comparison functions—such as `strcasecmp()`—will put UTF-8 strings in the correct order.

  * However, you should be careful when using the string comparison functions to order a set of UTF-8 strings. Unicode tries for a universal encoding and orders characters in a way that's generically correct, but it may not be correct for specific characters in specific languages. (Because it follows ASCII, UTF-8 is correct for English.)

  * For European languages, UTF-8 generally yields more compact data representations than would Unicode. Most of the characters in a string can be encoded in a single byte. In many other cases, UTF-8 is no less compact than Unicode.

* * *

### UTF-8 and the BeOS

The BeOS assumes UTF-8 encoding in most cases. For example, a `B_KEY_DOWN` message reports the character that's mapped to the key the user pressed as a UTF-8 value. That value is then passed as a string to [`KeyDown()`](BView.md#BView_KeyDown "KeyDown\(\)") along with the byte count:
    
    
    virtual void KeyDown(const char* bytes, int32 numBytes);

You can expect the bytes string to always contain at least one byte. And, of course, you can test it for an ASCII value without caring whether it's UTF-8:
    
    
    if ( bytes[0] == B_TAB )
       . . .

Similarly, objects that display text in the user interface—such as window titles and button labels—expect to be passed UTF-8 encoded strings, and hand you a UTF-8 string if you ask for the title or label. These objects display text using a system font—either the system plain font (`be_plain_font`) or the bold font (`be_bold_font`). The BFont class allows other character encodings, which you may need to use in limited circumstances from time to time, but the system fonts are constrained to UTF-8 (`B_UNICODE_UTF8` encoding). The FontPanel preferences application doesn't permit users to change the encoding of a system font.

Unicode and UTF-8 are documented in The Unicode Standard, Version 2.0, published by Addison-Wesley. See that book for complete information on Unicode and for a description of how UTF-8 encodes surrogate pairs.

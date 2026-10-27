<!-- Classes And Methods - The Support Kit - BString -->

## BString

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `support/String.h`  
Library:| `libbe.so`  
Allocation:| Constructor or on the stack  
[Class Overview](BString_Overview.md "BString")

  * [Constructor and Destructor](BString.md#BString_ConstructorDestructor)
  * [Member Functions](BString.md#BString_MemberFunctions)
  * [Operators](BString.md#BString_Operators)

  
  
* * *

### Constructor and Destructor

#### BString()

`BString();` `BString(const char* string);` `BString(const char* string,  
int32 maxLength);` `BString(const BString& string);`

Creates a new `BString` object that allocates enough storage to accommodate `string` (or `string`->[`String()`](BString.md#BString_String "String\(\), ByteAt\(\)"), or up to `maxLength` bytes), and then copies the string into the storage. Without an argument, the new `BString` is empty. You can also set a `BString`'s data by using [`SetTo()`](BString.md#BString_SetTo "SetTo\(\), Adopt\(\)"), [`Adopt()`](BString.md#BString_Adopt), or the [`= operator`](BString.md#BString_Adopt).

#### ~BString()

`~BString();`

Frees the object's allocated memory, and destroys the object.

* * *

### Member Functions

####  Append(), Prepend(), Insert()

`inline BString& Append(const BString& source);` `BString& Append(const BString& source,  
int32 charCount);` `inline BString& Append(const char* source);` `BString& Append(const char* source,  
int32 charCount);` `BString& Append(char c,  
int32 charCount);`

`BString& Prepend(const BString& source);` `BString& Prepend(const BString& source,  
int32 charCount);` `BString& Prepend(const char* source);` `BString& Prepend(const char* source,  
int32 charCount);` `BString& Prepend(char c,  
int32 charCount);`

`BString& Insert(const BString& source,  
int32 insertAt);` `BString& Insert(const BString& source,  
int32 charCount,  
int32 insertAt);` `BString& Insert(const BString& source,  
int32 sourceOffset,  
int32 charCount,  
int32 insertAt);` `BString& Insert(const char* source,  
int32 insertAt);` `BString& Insert(const char* source,  
int32 charCount,  
int32 insertAt);` `BString& Insert(const char* source,  
int32 sourceOffset,  
int32 charCount,  
int32 insertAt);` `BString& Insert(char c,  
int32 charCount,  
int32 insertAt);`

These functions add characters to the end (append), beginning (prepend), or middle (insert) of the `BString`'s string. In each case, the `BString` automatically reallocates to accommodate the new data. All of these functions return *this.

`Append()` copies `charCount` characters from `source` and adds them to the end of this `BString`. If `charCount` isn't specified, the entire string is copied; this is the same as the [`+= operator`](BString.md#BString_operator_addition_assignment "+= \(append\)"). The single character version of `Append()` adds `count` copies of the character `c` to the end of the object.

`Prepend()` does the same as `Append()`, except it adds the characters to the beginning of this `BString`, shifting the existing data "to the right" to make room.

`Insert()` adds the designated characters at `insertAt` (zero-based) in this `BString`. The `BString`'s existing data (at location `insertAt` and higher) is shifted right to make room for the new data. The `sourceOffset` argument is an offset (zero-based) into the source string.

####  CharacterEscape(), CharacterDeescape()

`BString& CharacterEscape(const char* original,  
const char* setOfCharsToEscape,  
char escapeWithChar);` `BString& CharacterEscape(const char* setOfCharsToEscape,  
char escapeWithChar);`

`BString& CharacterDeescape(const char* original,  
char escapeWithChar);` `BString& CharacterDeescape(char escapeWithChar);`

`CharacterEscape` scans a string for occurrences of the individual characters in the string `setOfCharsToEscape` and inserts into the string the byte indicated by `escapeWithChar` before each such occurrence. The first form copies the original string into the `BString`; the second form operates on the existing `BString`.

`CharacterDeescape()` undoes this operation by scanning the string and removing all occurrences of the byte `escapeWithChar`. The first form copies the `original` string into the `BString`, while the second form operates on the existing `BString`.

####  Compare() , ICompare()

`int Compare(const BString& string) const;` `int Compare(const BString& string,  
int32 range) const;` `int Compare(const char* string) const;` `int Compare(const char* string,  
int32 range) const;` `int Compare(const BString& astring,  
const BString& bstring) global;` `int Compare(const BString* astring,  
const BString* bstring) global;`

`int ICompare(const BString& string) const;` `int ICompare(const BString& string,  
int32 range) const;` `int ICompare(const char* string) const;` `int ICompare(const char* string,  
int32 range) const;` `int ICompare(const BString& astring,  
const BString& bstring) global;` `int ICompare(const BString* astring,  
const BString* bstring) global;`

These functions compare this `BString` with the argument string (`Compare()` is case-sensitive, `ICompare()` is case-insensitive); they return 0 if the two strings are the same, 1 if this `BString` is "greater than" the argument, and -1 if the argument is greater than this `BString`. Two strings are compared by comparing the ASCII or UTF-8 values of their respective characters. A longer string is greater than a shorter (but otherwise similar) string—"abcdef" is greater than "abcde".

Given a `range` argument, the functions only compare the first `range` characters.

The global functions return 0 if the arguments are equal, 1 if `astring` is greater than `bstring`, and -1 if `bstring` is greater than `astring`.

You can also compare two strings through the comparison operators ==, !=, <, <=, >, and >=.

####  CopyInto(), MoveInto()

`BString& CopyInto(BString& destination,  
int32 sourceOffset,  
int32 charCount) const;` `void CopyInto(char* destination,  
int32 sourceOffset,  
int32 charCount) const;`

`BString& MoveInto(BString& destination,  
int32 sourceOffset,  
int32 charCount);` `void MoveInto(char* destination,  
int32 sourceOffset,  
int32 charCount);`

`CopyInto()` copies a substring from this `BString` into `destination`. `MoveTo()` does the same, but it removes the original substring (from this `BString`). If `destination` is a `BString`, storage for the substring is automatically allocated; if the `destination` is a char*, the caller is responsible for allocating sufficient storage.

The substring comprises `charCount` characters starting at character `sourceOffset` (zero-based). After the substring is removed, the remaining characters move (to the left) to fill in the gap (`MoveTo()` only). For example:
    
    
    BString source, destination;
    source.SetTo("abcdefg");
    source.MoveInto(&destination, 2, 3);
    
    /* source.String() == "abfg" */
    /* destination.String() == "cde" */

The functions return destination (or void).

####  CountChars(), Length()

`int32 CountChars() const;``inline int32 Length() const;`

`CountChars()` returns the length of the object's string measured in characters; `Length()` returns the length measured in bytes.

####  FindFirst(), IFindFirst(), FindLast(), IFindLast()

`int32 FindFirst(const BString& string) const;` `int32 FindFirst(const BString& string,  
int32 offset) const;` `int32 FindFirst(const char* string) const;` `int32 FindFirst(const char* string,  
int32 offset) const;` `int32 FindFirst(char c) const;` `int32 FindFirst(char c,  
int32 offset) const;`

`int32 FindLast(const BString& string) const;` `int32 FindLast(const BString& string,  
int32 offset) const;` `int32 FindLast(const char* string) const;` `int32 FindLast(const char* string,  
int32 offset) const;` `int32 FindLast(char c) const;` `int32 FindLast(char c,  
int32 offset) const;`

`int32 IFindFirst(const BString& string) const;` `int32 IFindFirst(const BString& string,  
int32 offset) const;` `int32 IFindFirst(const char* string) const;` `int32 IFindFirst(const char* string,  
int32 offset) const;`

`int32 IFindLast(const BString& string) const;` `int32 IFindLast(const BString& string,  
int32 offset) const;` `int32 IFindLast(const char* string) const;` `int32 IFindLast(const char* string,  
int32 offset) const;`

These functions return the index of the first or last occurrence (within this `BString`) of a substring or character. `FindFirst()` and `FindLast()` are case-sensitive; `IFindFirst()` and `IFindLast()` are case-insensitive. The functions return `B_ERROR` if the character isn't found.

The `offset` versions only look in the portion of this `BString` that starts at character `offset` (for `[I]FindFirst()`), or that ends at character `offset` (for `[I]FindLast()`). For example, in this example…
    
    
    BString astring("AbcAbcAbc");
    astring.FindLast("Abc", 7);

…the `FindLast()` call returns 3, the index of the last complete instance of "Abc" that occurs before character 7.

####  LockBuffer(), UnlockBuffer()

`char* LockBuffer(int32 maxLength);``BString& UnlockBuffer(int32 length = -1);`

`LockBuffer()` returns a pointer to the object's string; you're allowed to manipulate this pointer directly. The `maxLength` argument lets you ask the object to allocate some extra space at the end of the string before passing you the pointer. If `maxLength` is less than the string's current length, the argument is ignored (pass 0 if you want to lock the buffer but don't need to pre-allocate extra space).

`UnlockBuffer()` tells the object that you're done manipulating the string pointer. `length` is the string's new length; if you pass -1, the `BString` gets the length by calling `strlen()` on the string. The functions returns *this.

Every `LockBuffer()` must be balanced by a subsequent `UnlockBuffer()`. In between the two calls, you may not call any `BString` functions. Many of the `BString` functions assert an error if they're called while the buffer is locked.

See "Direct Data Access" for an example.

####  Remove(), RemoveFirst(), RemoveLast(), RemoveAll() , RemoveSet()

`BString& Remove(int32 startingAt,  
int32 charCount);`

`BString& RemoveFirst(const BString& string);` `BString& RemoveFirst(const char* string);`

`BString& RemoveLast(const BString& string);` `BString& RemoveLast(const char* string);`

`BString& RemoveAll(const BString& string);` `BString& RemoveAll(const char* string);`

`BString& RemoveSet(const char* charSet);`

These functions remove characters from the `BString` and reallocate the object's storage so it fits the new (smaller) data. The functions return *this.

`Remove()` removes `charCount` characters, starting with character `startingAt` (zero-based).

`RemoveFirst()`, `RemoveLast()`, and `RemoveAll()` remove, respectively, the first, last and every occurrence of string within the object.

`RemoveSet()` removes all occurrences of every character in the `charSet` string. For example:
    
    
    BString string("ab abc abcd");
    string.RemoveSet("db");
    /* 'string' now contains "a ac ac" */

####  Replace(), ReplaceFirst(), ReplaceLast(), ReplaceSet(), IReplace() , IReplaceFirst(), IReplaceLast(), IReplaceAll()

`BString& Replace(const char* old,  
const char* new,  
int32 count,  
int32 offset = 0);` `BString& Replace(char old,  
char new,  
int32 count,  
int32 offset = 0);`

`BString& ReplaceFirst(const char* old,  
const char* new);` `BString& ReplaceFirst(char old,  
char new);`

`BString& ReplaceLast(const char* old,  
const char* new);` `BString& ReplaceLast(char old,  
char new);`

`BString& ReplaceAll(const char* old,  
const char* new,  
int32 offset = 0);` `BString& ReplaceAll(char old,  
char new,  
int32 offset = 0);`

`BString& ReplaceSet(const char* oldSet,  
const char* new);` `BString& ReplaceSet(const char* oldSet,  
char new);`

`BString& IReplace(const char* old,  
const char* new,  
int32 count,  
int32 offset = 0);` `BString& IReplace(char old,  
char new,  
int32 count,  
int32 offset = 0);`

`BString& IReplaceFirst(const char* old,  
const char* new);` `BString& IReplaceFirst(char old,  
char new);`

`BString& IReplaceLast(const char* old,  
const char* new);` `BString& IReplaceLast(char old,  
char new);`

`BString& IReplaceAll(const char* old,  
const char* new,  
int32 offset = 0);` `BString& IReplaceAll(char old,  
char new,  
int32 offset = 0);`

`BString& IReplaceSet(const char* oldSet,  
const char* new);` `BString& IReplaceSet(const char* oldSet,  
char new);`

These functions find occurrences of `old` within the `BString`, and replace them with `new`. In the `Replace…()` functions, the search for `old` is case-insensitive; in the `IReplace…()` functions, the search is case-insensitive. The functions return *this.

`[I]Replace()` replaces the first `count` occurrences that start on or after the character at `offset`.

`[I]ReplaceFirst()` and `[I]ReplaceLast()` replace only the first and last occurrence (respectively).

`[I]ReplaceAll()` replaces all occurrence that start on or after the character at `offset`.

`[I]ReplaceSet()` is slightly different from the others: It replaces each occurrence of any character in `oldSet` with the character or the entire string given by new. For example:
    
    
    BString astring("a-b-c");
    astring.ReplaceSet("abc", "ABC");
    /* astring is now "ABC-ABC-ABC" */

####  SetTo(), Adopt()

`inline BString& SetTo(const char* source);` `BString& SetTo(const char* source,  
int32 charCount);` `BString& SetTo(const BString& source);` `BString& SetTo(const BString& source,  
int32 charCount);` `BString& SetTo(char c,  
int32 charCount);`

`BString& Adopt(const BString& source);` `BString& Adopt(const BString& source,  
int32 charCount);`

These functions initialize a `BString`'s string data. Storage for the data is automatically allocated by the functions. The object's current data is wholly replaced by the new data. The functions return *this.

`SetTo()` copies `charCount` characters from `source` into this object. If `charCount` isn't given, the entire string is copied; this form is equivalent to the = operator. The single character version of `SetTo()` sets the object's data to a `charCount`-length string that consists entirely of the character `c`.

`Adopt()` moves `charCount` characters from `source` into this object, and then clears `source`'s data (pointer). Note that source will be empty after you call `Adopt()`, even if `charCount` is less than source's full length.

####  String(), ByteAt()

`inline const char* String() const;``inline char ByteAt(int32 index) const;`

`String()` returns a pointer to the object's string, guaranteed to be `NULL` terminated. You may not modify or free the pointer. If the `BString` is deleted, the pointer becomes invalid.

`ByteAt()` returns the `index`'th character in the string. If `index` is out of bounds, the function returns 0. Except for the boundary check, this function is the same as the [] operator.

####  ToLower(), ToUpper(), Capitalize(), CapitalizeEachWord()

`BString& ToLower();``BString& ToUpper();``BString& Capitalize();``BString& CapitalizeEachWord();`

`ToLower()` converts every character in the string to lower case; `ToUpper()` converts them all to upper case. Non-alphabetic characters aren't affected.

`Capitalize()` converts the first alphabetic character in the word to upper case, and the rest to lower case. `CapitalizeEachWord()` converts the first alphabetic character in each "word" to upper case, and the rest to lower case. A word is a group of alphabetic characters delimited by non-alphabetic characters.

The functions return *this.

Warning

![Warning](./images/admonitions/Stop_32.png)

`Capitalize()` and `CapitalizeEachWord()` are broken in Release 4.0.

#### Truncate()

`BString& Truncate(int32 charCount,  
bool lazy = true);`

`Truncate()` shrinks the object's string so that it's `charCount` characters long. This function will not lengthen the string: If `charCount` is equal to or greater than the string's current character count, the function does nothing.

If `lazy` is `false`, the string is immediately reallocated to trim it to the new size (and the excess is immediately freed). If it's `true`, the string is set to the new size, but the excess isn't freed until the next storage manipulating function is called. It's slightly more efficient to be lazy; otherwise, the two forms of the function are identical.

The function return *this.

* * *

### Operators

#### = (assignment)

`BString& operator=(const BString& string);` `BString& operator=(const char* string);` `BString& operator=(const char character);`

Sets the contents of the `BString` to `string` or `character`, and returns *this->[`LockBuffer()`](BString.md#BString_LockBuffer "LockBuffer\(\), UnlockBuffer\(\)").

#### += (append)

`inline BString& operator+=(const BString& string);` `BString& operator+=(const char* string);` `BString& operator+=(const char character);`

Appends `string` or `character` to this `BString`, and returns *this.

#### << (formatted append)

`BString& operator<<(const BString& string);` `BString& operator<<(const char* string);` `BString& operator<<(char c);` `BString& operator<<(uint32 val);` `BString& operator<<(int32 val);` `BString& operator<<(uint64 val);` `BString& operator<<(int64 val);` `BString& operator<<(float val);`

Converts the right operand to a string (if necessary), and appends it to the left operand. Floating point values are always written with two decimal digits:
    
    
    BString astring("The result is: "), bstring(" bps");
    float result = 12.5;
    astring << result << bstring;

Here, `astring` becomes "The result is: 12.50 bps".

Multiple append operations are evaluated from left to right, so that only the leftmost operand is modified. For example:
    
    
    BString astring("a"), bstring("b"), cstring("c");
    astring << bstring << cstring;

Here, `bstring` is appended to `astring`, and then `cstring` is appended to the result; thus, `astring` is "abc", and `bstring` is still "b".

#### [] (indexing)

`char operator[](int32 index) const;` `inline char& operator[](int32 index);`

Returns the character at `index` in the string. No boundary checking is done—it's up to the caller to ensure that `index` is in bounds.

####  ==, !=, <, >, <=, >= (comparison)

`inline bool operator==(const BString& string) const;` `bool operator==(const char* string) const;` `global inline bool operator==(const char* string,  
const BString& string) const;`

`inline bool operator!=(const BString& string) const;` `inline bool operator!=(const char* string) const;`

`global inline bool operator!=(const char* string,  
const BString& string) const;`

`inline bool operator<(const BString& string) const;` `bool operator<(const char* string) const;` `global inline bool operator<(const char* string,  
const BString& string) const;`

`inline bool operator>(const BString& string) const;` `bool operator>(const char* string) const;` `global inline bool operator>(const char* string,  
const BString& string) const;` `inline bool operator>=(const BString& string) const;` `bool operator>=(const char* string) const;` `global inline bool operator>=(const char* string,  
const BString& string) const;`

Case-sensitive comparison of two strings. Two strings are compared by comparing the ASCII or UTF-8 values of their respective characters. A longer string is greater than a shorter (but otherwise similar) string—"abcdef" is greater than "abcde".

The global versions of these operators are provided so you don't have to worry about the order of the operands; for example:
    
    
    if (astring == "Okay")
    /* ...is equivalent to... */
    if ("Okay" == astring)

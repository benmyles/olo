<!-- System Overview - The Support Kit - BString -->

## BString

The [`BString`](BString.md "BString") is a string allocation and manipulation class. The object takes care of allocating and freeing memory for you, and provides a number of character search, comparison, and manipulation functions in a variety of flavors: [`FindFirst()`](BString.md#BString_FindFirst "FindFirst\(\), IFindFirst\(\), FindLast\(\), IFindLast\(\)"), [`FindLast()`](BString.md#BString_FindLast), [`Prepend()`](BString.md#BString_Prepend), [`Append()`](BString.md#BString_Append "Append\(\), Prepend\(\), Insert\(\)"), [`Insert()`](BString.md#BString_Insert), [`Remove()`](BString.md#BString_Remove "Remove\(\), RemoveFirst\(\), RemoveLast\(\), RemoveAll\(\) , RemoveSet\(\)"), [`RemoveSet()`](BString.md#BString_RemoveSet), and so on. The class also defines a number of operators that map to these functions; for example, the [+=](BString.md#BString_operator_addition_assignment "+= \(append\)") operator is the same as the [`Append()`](BString.md#BString_Append "Append\(\), Prepend\(\), Insert\(\)") function. The operators make [`BString`](BString.md "BString") objects particularly easy to work with.

This document describes some global C functions and operators that act like [`BString`](BString.md "BString") functions. The global [`Compare()`](BString.md#BString_Compare "Compare\(\) , ICompare\(\)") and [`ICompare()`](BString.md#BString_ICompare) functions let you compare two [`BString`](BString.md "BString")s that are passed as arguments; they're meant to be used as hook functions that are called from within a sorting routine (such as `qsort()`). The global operators are defined so you don't have to worry about the order of the operands.

Warning

![Warning](./images/admonitions/Stop_32.png)

[`BString`](BString.md "BString") is not thread safe. You shouldn't try to access the same [`BString`](BString.md "BString") object from two different threads at the same time.

Currently, [`BString`](BString.md "BString") only supports assignment and retrieval of strings that contain single-byte characters. If you want to use a [`BString`](BString.md "BString") to store a string that contains multi-byte (UTF8) characters, you have to flatten the string yourself and adjust the character count arguments accordingly.

* * *

### Assigning and Retrieving String Data

To assign a [`BString`](BString.md "BString")'s string, you pass the string to the object's constructor, to [`SetTo()`](BString.md#BString_SetTo "SetTo\(\), Adopt\(\)"), or you use the [=](BString.md#BString_operator_assignment "= \(assignment\)") or [<<](BString.md#BString_operator_formatted_append "<< \(formatted append\)") operators:
    
    
    BString string("8 Bits");
    
    /* or */
    BString string;
    string.SetTo("8 Bits");
    
    /* or */
    BString string = "8 Bits";
    
    /* or */
    BString string;
    string << (int32)8 << " Bits";

There's also an [`Adopt()`](BString.md#BString_Adopt) function that let's you move data from one [`BString`](BString.md "BString") object to another. In all cases, the [`BString`](BString.md "BString") object takes care of allocating storage for you—the object is guaranteed to be "big enough" to contain the string data.

The [`String()`](BString.md#BString_String "String\(\), ByteAt\(\)") function retrieves the object's string:
    
    
    myButton->SetLabel(string.String());

[`ByteAt()`](BString.md#BString_ByteAt) returns a single character, located by index. You can also get a character by using the [[]](BString.md#BString_operator_indexing "\[\] \(indexing\)") operator; the following returns the string's first character:
    
    
    char c = string[0];

* * *

### Direct Data Access

If you want to access a [`BString`](BString.md "BString")'s data directly, you call [`LockBuffer()`](BString.md#BString_LockBuffer "LockBuffer\(\), UnlockBuffer\(\)"), and then call [`UnlockBuffer()`](BString.md#BString_UnlockBuffer) when you're finished. Between these two calls, you can manipulate the string data, but you mustn't call any other [`BString`](BString.md "BString") function:
    
    
    int32 len = string.Length()+1
    char *stringData = string.LockBuffer(len);
    
    /* While the buffer is locked you can manipulate the string by hand, or
    through standard C functions, but you mustn't call BString functions. */
    
    stringData[0] = 'd';
    strcat(stringData, " button");
    string.UnlockBuffer(len+strlen(" button"));
    
    /* The object can once again receive BString functions. */

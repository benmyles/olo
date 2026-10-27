<!-- System Overview - The Application Kit - BMessage -->

## BMessage

A [`BMessage`](BMessage.md "BMessage") is a bundle of structured information. Every [`BMessage`](BMessage.md "BMessage") contains a command constant and some number of data fields.

  * The command constant is an int32 value that describes, roughly, the purpose of the [`BMessage`](BMessage.md "BMessage"). It's stored as the public `what` data member. You always set and examine the `what` value directly, you don't need to call a function. (As a convenience, you can set the command constant when you create your [`BMessage`](BMessage.md "BMessage") object.)

  * The data fields are name-type-value triplets. A field is be primarily identified by name, but you can look for fields by name, type, or a combination of the two. The type is encoded as a constant (`B_INT32_TYPE`, `B_STRING_TYPE` etc), and is meant to describe the type of value that the field holds. A single field can have only one name and one type, but can contain an array of values. Individual values in a field are accessible by index.

Neither the command constant nor the data fields are mandatory. You can create a [`BMessage`](BMessage.md "BMessage") that has data but no command, or that only has a command. However, creating a [`BMessage`](BMessage.md "BMessage") that has neither is pointless.

* * *

### Preparatory Reading

[`BMessage`](BMessage.md "BMessage")s are used throughout the kits to send data (or notifications) to another threadpossibly in another application. To understand how [`BMessage`](BMessage.md "BMessage")s fit into the messaging system, see "[Messaging](TheApplicationKit_Messaging.md "Messaging")".

The [`BMessage`](BMessage.md "BMessage") class also contributes a number of functions that help define the scripting system. See "[Scripting](TheApplicationKit_Scripting.md "Scripting")" for an introduction to this system.

[`BMessage`](BMessage.md "BMessage")s are also used by a number of classes ([`BClipboard`](BClipboard.md "BClipboard"), [`BArchivable`](BArchivable.md "BArchivable"), and others) for their ability to store data.

* * *

### Types of Functions

The [`BMessage`](BMessage.md "BMessage") class defines five types of functions:

Data field functions.
    

These functions either set or retrieve the value of a data field. See [`AddData()`](BMessage.md#BMessage_AddData "AddData\(\), AddBool\(\), AddInt8\(\), AddInt16\(\), AddInt32\(\), AddInt64\(\), AddFloat\(\), AddDouble\(\), AddString\(\), AddPoint\(\), AddRect\(\), AddRef\(\), AddMessage\(\), AddMessenger\(\), AddPointer\(\), AddFlat\(\)"), [`FindData()`](BMessage.md#BMessage_FindData "FindData\(\), FindBool\(\), FintInt8\(\), FindInt16\(\), FindInt32\(\), FindInt64\(\), FindFloat\(\), FindDouble\(\), FindString\(\), FindPoint\(\), FindRect\(\), FindRef\(\), FindMessage\(\), FindMessenger\(\), FindPointer\(\), FindFlat\(\)"), [`ReplaceData()`](BMessage.md#BMessage_ReplaceData "ReplaceData\(\), ReplaceBool\(\), ReplaceInt8\(\), ReplaceInt16\(\), ReplaceInt32\(\), ReplaceInt64\(\), ReplaceFloat\(\), ReplaceDouble\(\), ReplaceString\(\), ReplacePoint\(\), ReplaceRect\(\), ReplaceRef\(\), ReplaceMessage\(\), ReplaceMessenger\(\), ReplacePointer\(\), ReplaceFlat\(\)"), and [`RemoveName()`](BMessage.md#BMessage_RemoveName "RemoveName\(\), RemoveData\(\)").

Info functions.
    

These functions retrieve information about the state and contents of the [`BMessage`](BMessage.md "BMessage"). See [`IsSystem()`](BMessage.md#BMessage_IsSystem "IsSystem\(\)") and [`GetInfo()`](BMessage.md#BMessage_GetInfo "GetInfo\(\)").

Messaging functions.
    

These functions are part of the messaging system. A smaller set of functions reports on the status of a received message. For example, [`IsSourceWaiting()`](BMessage.md#BMessage_IsSourceWaiting) tells whether the message sender is waiting for a reply, [`WasDropped()`](BMessage.md#BMessage_WasDropped "WasDropped\(\), DropPoint\(\)") says whether it was dragged and dropped, and [`DropPoint()`](BMessage.md#BMessage_DropPoint) says where it was dropped.

Scripting functions.
    

Functions such as [`AddSpecifier()`](BMessage.md#BMessage_AddSpecifier "AddSpecifier\(\)") and [`PopSpecifier()`](BMessage.md#BMessage_PopSpecifier).

Flattening functions.
    

The data in a [`BMessage`](BMessage.md "BMessage") can be flattened into an untyped stream of bytes. See [`Flatten()`](BMessage.md#BMessage_Flatten "Flatten\(\), Unflatten\(\), FlattenedSize\(\)").

* * *

### BMessage Ownership

The documentation for the functions that accept or pass back a [`BMessage`](BMessage.md "BMessage") object should tell you who's responsible for deleting the object. Most functions that accept a [`BMessage`](BMessage.md "BMessage") argument copy the object, leaving the caller with the responsibility for deleting the argument. The exceptions i.e. [`BMessage`](BMessage.md "BMessage")-accepting functions that take over ownership of the object are listed below:

Functions that return a [`BMessage`](BMessage.md "BMessage") to you usually don't give up ownership; in general, you don't delete the [`BMessage`](BMessage.md "BMessage")s that are passed to you. The exceptions, functions that expect the caller to take over ownership of a passed-back [`BMessage`](BMessage.md "BMessage"),are listed below:

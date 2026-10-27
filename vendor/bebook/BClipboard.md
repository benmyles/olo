<!-- Classes And Methods - The Application Kit - BClipboard -->

## BClipboard

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `app/Clipboard.h`  
Library:| `libbe.so`  
Allocation:| Constructor, on the stack, or use the [`be_clipboard`](BClipboard.md#BClipboard_be_clipboard "be_clipboard") global object.  
[Class Overview](BClipboard_Overview.md "BClipboard")

  * [Constructor and Destructor](BClipboard.md#BClipboard_ConstructorDestructor)
  * [Member Functions](BClipboard.md#BClipboard_MemberFunctions)
  * [Global Variables](BClipboard.md#BClipboard_GlobalVariables)

  
  
* * *

### Constructor and Destructor

#### BClipboard()

`BClipboard(const char* name,  
bool discard = false);`

Creates a new `BClipboard` object that refers to the `name` clipboard. The clipboard itself is created if a clipboard of that name doesn't already exist.

The `discard` flag is currently unused.

#### ~BClipboard()

`virtual ~BClipboard();`

Destroys the `BClipboard` object. The clipboard itself and the data it contains are not affected by the object's destruction.

* * *

### Member Functions

####  Clear(), Commit(), Revert()

`status_t Clear();``status_t Commit();``status_t Revert();`

These functions are used when you're writing data to the clipboard. `Clear()` prepares your `BClipboard` for writing. You call `Clear()` just before you add new data to your clipboard message. `Commit()` copies your `BClipboard` data back to the clipboard. See "[Writing to the Clipboard](BClipboard_Overview.md#BClipboard_Overview_Writing_To_The_Clipboard "Writing to the Clipboard")" for an example of these functions.

`Revert()` refreshes the `BClipboard`'s data message by uploading it from the clipboard. The function is provided for the (rare) case where you alter your `BClipboard`'s data message, and then decide to back out of the change. In this case, you should call `Revert()` (rather than `Commit()`). If you don't revert, your `BClipboard`'s message will still contain your unwanted change, even if you unlock and then re-lock the object.

All three functions return `B_ERROR` if the `BClipboard` isn't locked, and `B_OK` otherwise.

#### Data()

`[`BMessage`](BMessage.md "BMessage")* Data() const;`

Returns the [`BMessage`](BMessage.md "BMessage") object that holds the `BClipboard`'s data, or `NULL` if the `BClipboard` isn't locked. You're expected to read and write the [`BMessage`](BMessage.md "BMessage") directly; however, you may not free it or dispatch it like a normal [`BMessage`](BMessage.md "BMessage"). If you change the [`BMessage`](BMessage.md "BMessage") and want to write it back to the clipboard, you have to call `Commit()` after you make the change.

See "[The Clipboard Message](BClipboard_Overview.md#BClipboard_Overview_The_Clipboard_Message "The Clipboard Message")" for more information.

#### DataSource()

`[`BMessenger`](BMessenger.md "BMessenger") DataSource() const;`

Returns a [`BMessenger`](BMessenger.md "BMessenger") that targets the [`BApplication`](BApplication.md "BApplication") object of the application that last committed data to the clipboard. The `BClipboard` needn't be locked.

####  LocalCount(), SystemCount()

`uint32 LocalCount() const;``uint32 SystemCount() const;`

These functions return the clipboard count. `LocalCount()` uses a cached count, while `SystemCount()` asks the Application Server for the more accurate system counter.

####  Lock(), Unlock(), IsLocked()

`bool Lock();``void Unlock();``bool IsLocked();`

`Lock()` uploads data from the clipboard into your `BClipboard` object, and locks the object so no other thread in your application can use it. You must call `Lock()` before reading or writing the `BClipboard`. `Lock()` blocks if the object is already locked. It returns `true` if the lock was acquired, and `false` if the `BClipboard` object was deleted while `Lock()` was blocked.

Warning

![Warning](./images/admonitions/Stop_32.png)

There's no way to tell `Lock()` to time out.

`Unlock()` unlocks the object so other threads in your application can use it.

`IsLocked()` hardly needs to be documented.

#### Name()

`const char* Name() const;`

Returns the name of the clipboard. The object needn't be locked.

####  StartWatching(), StopWatching()

`status_t StartWatching(BMessenger target);``status_t StopWatching(BMessenger target);`

If you want to be alerted when the clipboard changes, call `StartWatching()`, passing a [`BMessenger`](BMessenger.md "BMessenger") to be the target for the notification. When the clipboard changes, a `B_CLIPBOARD_CHANGED` message will be sent to the target.

`StopWatching()` stops monitoring the clipboard for changes.

Return Code| Description  
---|---  
`B_OK`.| No error.  
Other errors.| You get the idea.  
  
* * *

### Global Variables

#### be_clipboard
    
    
    BClipboard* be_clipboard

This variable gives applications access to the system clipboard—the shared repository of data for cut, copy, and paste operations. It's initialized at startup.

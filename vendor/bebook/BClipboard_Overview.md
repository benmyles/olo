<!-- System Overview - The Application Kit - BClipboard -->

## BClipboard

A [`BClipboard`](BClipboard.md "BClipboard") object is an interface to a clipboard, a resource that provides system-wide, temporary data storage. Clipboards are identified by name; if two apps want to refer to the same clipboard, they simply create respective [`BClipboard`](BClipboard.md "BClipboard") objects with the same name:
    
    
    /* App A: This creates a clipboard named "MyClipboard". */
    [BClipboard](BClipboard.md "BClipboard") *appAclipboard = new [BClipboard](BClipboard.md "BClipboard")("MyClipboard");
    
    /* App B: This object refers to the clipboard already created
       by App A. */
    [BClipboard](BClipboard.md "BClipboard") *appBclipboard = new [BClipboard](BClipboard.md "BClipboard")("MyClipboard");

* * *

### The System Clipboard

In practice, you rarely need to construct your own [`BClipboard`](BClipboard.md "BClipboard") object; instead, you use the `BClipboard` that's created for you by your [`BApplication`](BApplication.md "BApplication") object. This object, which you refer to through the global [`be_clipboard`](BClipboard.md#BClipboard_be_clipboard "be_clipboard") variable, accesses the default system clipboard. Data that you write to your [`be_clipboard`](BClipboard.md#BClipboard_be_clipboard "be_clipboard") object can be read from any other app's [`be_clipboard`](BClipboard.md#BClipboard_be_clipboard "be_clipboard") For example, the cut/copy/paste operations defined by [`BTextView`](BTextView.md "BTextView") transfer data through the system clipboard.

Note

![Note](./images/admonitions/Info_32.png)

To access the system clipboard without creating a [`BApplication`](BApplication.md "BApplication") object, construct a [`BClipboard`](BClipboard.md "BClipboard") object with the name "system". The system clipboard is under the control of the user; you should only read or write the system clipboard as a direct result of the user's actions. If you create your own clipboards don't name them "system".

* * *

### The Clipboard Message

To access a clipboard's data, you call functions on a [`BMessage`](BMessage.md "BMessage") that the [`BClipboard`](BClipboard.md "BClipboard") object hands you (through its [`Data()`](BClipboard.md#BClipboard_Data "Data\(\)") function). The [`BMessage`](BMessage.md "BMessage") follows these conventions:

  * The [`what`](BMessage.md#BMessage_what "what") value is unused.

  * The data is stored in a message field. The field should be typed as `B_MIME_TYPE`; the MIME type that describes the data should be used as the name of the field that holds the data (see "[Writing to the Clipboard](BClipboard_Overview.md#BClipboard_Overview_Writing_To_The_Clipboard "Writing to the Clipboard")" for an example).

  * If the [`BMessage`](BMessage.md "BMessage") contains more than one field, each field should present the same data in a different format. For example, the StyledEdit app writes text data in its own format (in order to encode the fonts, colors, etc.) and also writes the data as plain ASCII text (MIME type "text/plain").

* * *

### Writing to the Clipboard

The following annotated example shows how to write to the clipboard.
    
    
    [BMessage](BMessage.md "BMessage")* clip = ([BMessage](BMessage.md "BMessage") *)NULL;
      if (be_clipboard->Lock()![1](./images/callouts/1.png)) {
        be_clipboard->Clear()![2](./images/callouts/2.png);
        if ((clip = be_clipboard->Data()![3](./images/callouts/3.png)) {
           clip->AddData("text/MyFormat", B_MIME_TYPE, myText,
                         myLength)![4](./images/callouts/4.png);
           clip->AddData("text/plain", B_MIME_TYPE, asciiText,
                         asciiLength);
           be_clipboard->Commit()![5](./images/callouts/5.png);
        }
        be_clipboard->Unlock()![6](./images/callouts/6.png);
     }

![1](./images/callouts/1.png) | `Lock()` your [`BClipboard`](BClipboard.md "BClipboard") object. This uploads data from the clipboard into your [`BClipboard`](BClipboard.md "BClipboard")'s local [`BMessage`](BMessage.md "BMessage") object, and prevents other threads in your application from accessing the [`BClipboard`](BClipboard.md "BClipboard")'s data. Note that locking does not lock the underlying clipboard data other applications can change the clipboard while you have your object locked.  
---|---  
![2](./images/callouts/2.png) | Prepare the [`BClipboard`](BClipboard.md "BClipboard") for writing by calling `Clear()`. This erases the data that was uploaded from the clipboard.  
![3](./images/callouts/3.png) | Call [`Data()`](BClipboard.md#BClipboard_Data "Data\(\)") to get a pointer to the [`BClipboard`](BClipboard.md "BClipboard")'s [`BMessage`](BMessage.md "BMessage") object.  
![4](./images/callouts/4.png) | Write the data by invoking [`AddData()`](BMessage.md#BMessage_AddData "AddData\(\), AddBool\(\), AddInt8\(\), AddInt16\(\), AddInt32\(\), AddInt64\(\), AddFloat\(\), AddDouble\(\), AddString\(\), AddPoint\(\), AddRect\(\), AddRef\(\), AddMessage\(\), AddMessenger\(\), AddPointer\(\), AddFlat\(\)") directly on the [`BMessage`](BMessage.md "BMessage"). In the example, we write the data in two different formats.  
![5](./images/callouts/5.png) | Call [`Commit()`](BClipboard.md#BClipboard_Commit) to copy your [`BMessage`](BMessage.md "BMessage") back to the clipboard. As soon as you call [`Commit()`](BClipboard.md#BClipboard_Commit), the data that you added is visible to other clipboard clients.  
![6](./images/callouts/6.png) | [`Unlock()`](BClipboard.md#BClipboard_Unlock) balances the [`Lock()`](BClipboard.md#BClipboard_Lock "Lock\(\), Unlock\(\), IsLocked\(\)"). The [`BClipboard`](BClipboard.md "BClipboard") object can now be accessed by other threads in your application.  
  
If you decide that you don't want to commit your changes, you should call [`Revert()`](BClipboard.md#BClipboard_Revert) before you unlock.

* * *

### Reading from the Clipboard

Here we show how to read a simple string from the clipboard.
    
    
    const char *text;
    int32 textLen;
    BMessage *clip = (BMessage *)NULL;
     if (be_clipboard->Lock()![1](./images/callouts/1.png)) {
       if ((clip = be_clipboard->Data())
          clip->FindData("text/plain", B_MIME_TYPE,
              (const void **)text, textlen)![2](./images/callouts/2.png);
    
       be_clipboard-Unlock()![3](./images/callouts/3.png);
    }

![1](./images/callouts/1.png) | As in writing, we bracket the operation with [`Lock()`](BClipboard.md#BClipboard_Lock "Lock\(\), Unlock\(\), IsLocked\(\)") and [`Unlock()`](BClipboard.md#BClipboard_Unlock). Keep in mind that [`Lock()`](BClipboard.md#BClipboard_Lock "Lock\(\), Unlock\(\), IsLocked\(\)") uploads data from the clipboard into our object. Any changes that are made to the clipboard (by some other application) after [`Lock()`](BClipboard.md#BClipboard_Lock "Lock\(\), Unlock\(\), IsLocked\(\)") is called won't be seen here.  
---|---  
![2](./images/callouts/2.png) | In this example, we only look for one hard-coded format. In a real application, you may have a list of formats that you can look for.  
![3](./images/callouts/3.png) | It isn't necessary to examine the clipboard data before you unlock it. The [`FindData()`](BMessage.md#BMessage_FindData "FindData\(\), FindBool\(\), FintInt8\(\), FindInt16\(\), FindInt32\(\), FindInt64\(\), FindFloat\(\), FindDouble\(\), FindString\(\), FindPoint\(\), FindRect\(\), FindRef\(\), FindMessage\(\), FindMessenger\(\), FindPointer\(\), FindFlat\(\)") call could just as well have been performed after the [`Unlock()`](BClipboard.md#BClipboard_Unlock) call.  
  
* * *

### Persistence

Inter-boot persistence:
    

Clipboard data does not persist between boots, the constructor provides a persistence flag but it's currently unused.

Intra-boot persistence:
    

Once you've created a clipboard, that clipboard will exist until you reboot your computer. For example, let's say you design an app that creates a clipboard called "MyClip": You launch the app, write something to "MyClip", and then quit the app. The clipboard and the data that you wrote to it will still exist: If you relaunch your app (or any app that knows about "MyClip"), you can pick up the data by reading from the "MyClip" clipboard.

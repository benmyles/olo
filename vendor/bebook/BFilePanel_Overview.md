<!-- System Overview - The Storage Kit - BFilePanel -->

## BFilePanel

`BFilePanel` knows how to create and display an "Open File" or "Save File" panel, and provides the means for filtering and responding to the user's actions on the panel. The Save Panel looks like this:

![File Save Panel](./images/TheStorageKit/FilePanel1.png)

The Open Panel looks pretty much the same, but without the text view in the lower left corner.

* * *

### Creating and Using a BFilePanel

To create and use a `BFilePanel`, follow these steps:

  1. Construct a `BFilePanel` object in response to the user's request (most likely, a click on an "Open" or "Save"/"Save As" menu item). When you construct the panel, you have to specify its "mode" (Open or Save).

  2. Fine-tune the panel by telling it which directory to display, whether it allows multiple selection, whether it can open a directory, which target it should send notifications to, and so on. (Most of these parameters can also be set in the constructor.)

  3. Invoke `Show()` on the panel, and then wait for the user to confirm a selection (or close the panel).

  4. Receive a message. When the user confirms a selection (or cancels the panel), the panel disappears and a notification message (Open, Save, or Cancel) is sent to the panel's target. The message identifies the confirmed file(s).

  5. Delete the `BFilePanel` object…or don't. When the user closes a file panel, the object is not automatically deleted; you have to do it yourself. But you may not want to. If you don't delete the panel, you can simply call `Show()` the next time you want to display it; the state from the previous invocation (the panel's size and location, the directory it points to) is remembered.

* * *

### Constructing and Fine-tuning the Panel

The `BFilePanel` constructor has about two thousand arguments. They all have default values, and most of the parameters that they control can be set through individual functions. The following sections list and describe the constructor arguments and tell you if there's an analogous function.

#### Panel Mode

Argument| Default| Function  
---|---|---  
file_panel_mode `mode`| `B_OPEN_PANEL`| –  
  
There are two file panel modes: `B_OPEN_PANEL` and `B_SAVE_PANEL`. You've got to make up your mind in the constructor.

#### Target

Argument| Default| Function  
---|---|---  
[`BMessenger`](BMessenger.md "BMessenger")* `target`| `be_app_messenger`| [`SetTarget()`](BFilePanel.md#BFilePanel_SetTarget "SetTarget\(\), Messenger\(\)")  
  
The `target` represents the [`BLooper`](BLooper.md "BLooper")/[`BHandler`](BHandler.md "BHandler") that will receive the Open, Save, and Cancel messages.

#### Panel Directory

Argument| Default| Function  
---|---|---  
entry_ref* `panel_directory`| cwd| SetPanelDirectory()  
  
When a panel is first displayed, it has to show the contents of some directory; this is called the "panel directory." The panel directory defaults to the current working directory.

#### Confirmable Node Flavors

Argument| Default| Function  
---|---|---  
uint32 `node_flavors`| `B_FILE_NODE`| –  
  
Note

![Note](./images/admonitions/Info_32.png)

This parameter applies to Open panels only.

There are three node flavors: `B_FILE_NODE`, `B_DIRECTORY_NODE`, and `B_SYMLINK_NODE`. You combine these constants to declare the flavors that you want the user to be able to confirm. Before describing the flavor settings, keep this in mind…

  * Double-clicking a directory in the file list always enters the directory, regardless of the panel's flavor setting.

If you understand the following, you can save yourself some reading:

  * If your app wants to open files only, then stick with the default (`B_FILE_NODE`); the user will be able to confirm files and symlinks to files. If you want directories as well (for example, a compression app might want to work on files and directories) then add in `B_DIRECTORY_NODE` (symlinks to directories are okay, as well). If you only want directories (unusual, but possible), then leave `B_FILE_NODE` out of it.

If you're not convinced, read on:

  * If the setting includes `B_FILE_NODE` and the user selects and confirms a file or a symlink to a file, the file (or symlink) is delivered to your target. If it doesn't include `B_FILE_NODE` and the user selects a file (or symlink to a file), the Open button is disabled.

  * If the setting includes `B_DIRECTORY_NODE` and the user selects and Opens (i.e. clicks the Open button) a directory or a symlink to a directory, the directory (or symlink) is delivered to your target. If it doesn't include `B_DIRECTORY_NODE` and the user Opens a directory (or symlink to a directory), the directory is entered (the contents of the directory are displayed in the file list).

  * If the setting includes `B_SYMLINK_NODE` and the user confirms a symlink, the symlink is delivered to your target. If it doesn't include `B_SYMLINK_NODE` and the user selects symlink, the panel's response depends on the inclusion of the other two flavors. Note that including `B_SYMLINK_NODE` is an odd thing to do—it only makes sense if it's not combined with either of the other two flavors, and even then it doesn't make much sense.

As implied by the here, when the user confirms a symlink (regardless of the flavor setting), you always receive the symlink itself in the Open message—you don't get the file or directory it points to.

#### Multiple Selection

Argument| Default| Function  
---|---|---  
bool `allow_multiple_selection`| `true`| –  
  
This parameter determines whether the user is allowed to select more than one item at a time. Save panels should set this to `false`.

#### Notification Message

Argument| Default| Function  
---|---|---  
[`BMessage`](BMessage.md "BMessage")* `message`| a default [`BMessage`](BMessage.md "BMessage")| [`SetMessage()`](BFilePanel.md#BFilePanel_SetMessage "SetMessage\(\)")  
  
By default, the format of the message that's sent to your target when the user confirms or cancels is defined by the file panel (the default formats are defined later). You can override the default by specifying your own [`BMessage`](BMessage.md "BMessage"). The [`BMessage`](BMessage.md "BMessage") is copied by the `BFilePanel` object. You can change this message using the [`SetMessage()`](BFilePanel.md#BFilePanel_SetMessage "SetMessage\(\)") function.

#### Ref Filter

Argument| Default| Function  
---|---|---  
[`BRefFilter`](BRefFilter.md "BRefFilter")* `filter`| `NULL`| [`SetRefFilter()`](BFilePanel.md#BFilePanel_SetRefFilter "SetRefFilter\(\), RefFilter\(\)")  
  
When panel directory changes (this includes when the panel is constructed, and when the panel's [`Refresh()`](BFilePanel.md#BFilePanel_Refresh "Refresh\(\)") function is called), or when a new entry is added to the existing directory, the new entries are passed, one-by-one, to the panel's [`BRefFilter`](BRefFilter.md "BRefFilter") object through a [`BRefFilter`](BRefFilter.md "BRefFilter") hook function. In your implementation of the hook function, you can reject individual entries; rejected entries won't be displayed in the file list.

By default, a file panel has no [`BRefFilter`](BRefFilter.md "BRefFilter"). To supply one, you have to subclass [`BRefFilter`](BRefFilter.md "BRefFilter") (in order to implement the hook function) and pass it in.

  * Note that the ref filter isn't asked to "re-review" the entry list when the file panel is Show()'d after being hidden.

#### Is Modal?

Argument| Default| Function  
---|---|---  
bool| `false`| –  
  
A modal file panel can't be closed; to get rid of the panel, the user has to click a button. By default, file panels are not modal.

#### Hide When Done

Argument| Default| Function  
---|---|---  
bool `hide_when_done`| `true`| [`SetHideWhenDone()`](BFilePanel.md#BFilePanel_SetHideWhenDone "SetHideWhenDone\(\), HidesWhenDone\(\)")  
  
By default, a file panel is hidden when the user confirms or Cancels. If you set `hide_when_done` to `false`, the panel remains on the screen. Clicking the panel's close box always hides the panel

* * *

### The Target and the Messages it Sees

When the user confirms a selection or cancels a file panel, a [`BMessage`](BMessage.md "BMessage") is constructed and sent to the target of the `BFilePanel` object. By default, the target is [`be_app_messenger`](BApplication.md#BApplication_be_app_messenger "be_app_messenger"). You can specify a different target (as a [`BMessenger`](BMessenger.md "BMessenger")) through the `BFilePanel` constructor, or through the [`SetTarget()`](BFilePanel.md#BFilePanel_SetTarget "SetTarget\(\), Messenger\(\)") function.

The format of the [`BMessage`](BMessage.md "BMessage") that the target receives depends on whether the user is opening, saving, or canceling.

* * *

### Open Notification

If the target is [`be_app_messenger`](BApplication.md#BApplication_be_app_messenger "be_app_messenger") and the [`what`](BMessage.md#BMessage_what "what") field is `B_REFS_RECEIVED`, the [`BMessage`](BMessage.md "BMessage") shows up in the [`RefsReceived()`](BApplication.md#BApplication_RefsReceived "RefsReceived\(\)") function. Otherwise it's sent to the target's [`MessageReceived()`](BHandler.md#BHandler_MessageReceived "MessageReceived\(\)").

  * By default, the [`what`](BMessage.md#BMessage_what "what") field is `B_REFS_RECEIVED`. You can override the default by supplying your own [`BMessage`](BMessage.md "BMessage") ([`SetMessage()`](BFilePanel.md#BFilePanel_SetMessage "SetMessage\(\)")).

  * The `refs` field (type `B_REF_TYPE`) contains entry_ref structures, one for each entry that the user has confirmed.

  * The message may contain other fields, as copied from the [`BMessage`](BMessage.md "BMessage") you (optionally) supplied. The data in these fields won't be changed.

Keep in mind that the refs that you receive through this message point to the literal entries that the user confirmed. In other words, if the confirmed selection is a symlink to a file, you'll receive a ref for the symlink, not the file (and similarly for a link to a directory). It's up to you to turn the symlink into a file (which is probably what you want).

If you want a [`BEntry`](BEntry.md "BEntry") object, all you have to do is pass `true` as the `traverse` argument to [`BEntry`](BEntry.md "BEntry")'s constructor or [`SetTo()`](BEntry.md#BEntry_SetTo):
    
    
    /* We'll assume that 'ref' was just plucked from an
       open notification. */
    BEntry entry(ref, true);

You don't even have to check to see if the ref is a symlink.

If you want to turn a symlink ref into a ref to the pointed-to file, just add this line:
    
    
    entry.GetRef(&ref);

* * *

### Save Notification

Save notifications are always sent to the target's [`MessageReceived()`](BHandler.md#BHandler_MessageReceived "MessageReceived\(\)") function.

  * By default, the [`what`](BMessage.md#BMessage_what "what") field is `B_SAVE_REQUESTED`. You can override the default by supplying your own [`BMessage`](BMessage.md "BMessage") ([`SetMessage()`](BFilePanel.md#BFilePanel_SetMessage "SetMessage\(\)")).

  * The `directory` field (type `B_REF_TYPE`) contain a single entry_ref structure that points to the directory in which the user has requested the entry be saved (in other words, the ref refers to the panel directory).

  * The `name` field (`B_STRING_TYPE`) is the text the user typed in the Save Panel's text view.

  * The message may contain other fields, as copied from the [`BMessage`](BMessage.md "BMessage") you (optionally) supplied. The data in these fields won't be changed.

Note that if the user confirms a name that collides with an existing file, an alert is automatically displayed. The user can then back out of the confirmation and return to the Save Panel, or clobber the existing file. The save notification is sent after (and only if) the user agrees to clobber the file.

Note

![Note](./images/admonitions/Info_32.png)

The file _isn't_ clobbered by the system; it's up to you (as the receiver of the save notification) to do the dirty work.

* * *

### Cancel Notification

A cancel notification is sent _whenever_ the file panel is hidden. This includes the Cancel button being clicked, the panel being closed, and the panel being hidden after an open or a save (given that the panel is in hide-when-done mode).

Cancel notifications are always sent to the target's [`MessageReceived()`](BHandler.md#BHandler_MessageReceived "MessageReceived\(\)") function.

  * The [`what`](BMessage.md#BMessage_what "what") field is always `B_CANCEL`, even if you supplied your own [`BMessage`](BMessage.md "BMessage").

  * The `old_what` field (`B_UINT32_TYPE`) records the "previous" what value. This is only useful (and dependable) if you supplied your own [`BMessage`](BMessage.md "BMessage"): The [`what`](BMessage.md#BMessage_what "what") from your message is moved to the `old_what` field. If you didn't supply a [`what`](BMessage.md#BMessage_what "what"), you should ignore this field (it could contain garbage).

  * The `source` (`B_POINTER_TYPE`) is a pointer to the `BFilePanel` object that was closed.

  * The message may contain other fields, as copied from the [`BMessage`](BMessage.md "BMessage"): you (optionally) supplied. The data in these fields won't be changed.

Keep in mind that when a file panel is closed—regardless of how it's closed—the `BFilePanel` object is not destroyed. It's merely hidden.

* * *

### Modifying the Look of the File Panel

There are two ways you can modify the look of your `BFilePanel` object.

  * You can do some simple text twiddling by calling the label- and text-setting functions SetButtonLabel() and SetSaveText().

  * If you need to really change the look, you can get a handle on the panel's BWindow and BView objects in order to move them around, add your own, or whatever. You get the window through the Window() function. Finding specific views within the window is described below.

#### Finding Views in the Panel

The views in the panel are (mostly) named, as listed and shown below

  * "MenuBar" is the window's menu bar.

  * "DirMenuField" is the path popup.

  * "TitleView" is the bar that holds the attribute titles.

  * "PoseView" is the scrollable list of files.

  * "VScrollBar" and "HScrollBar" are the vertical and horizontal scroll bars.

  * "CountVw" is the item counter to the left of the horizontal scroll bar.

  * "text view" is where the user types a file name (Save Panel only).

  * "default button" is the Save or Open button.

  * "cancel button" is the Cancel button.

![File Panel Views](./images/TheStorageKit/FilePanel2.png)

The background view doesn't have a name, but it's always the first in the window's list of views:
    
    
    BView* background = filepanel->Window()->ChildAt(0);

The other views can be found by name, reckoning off of the background view. For example, here we get the "PoseView" view (the view that contains the file list):
    
    
    BView* files = background->FindView("PoseView");

* * *

### The C Functions

You can also display Open and Save Panels through the global C functions `run_open_panel()` and `run_save_panel()` (which are declared in `FilePanel.h`). The functions create `BFilePanel` objects using the default constructor settings (modulo the file_panel_mode, of course).

The C functions create a new file panel each time they're called, and delete the panel when the user is finished with it.

<!-- Classes And Methods - The Interface Kit - Message Constants -->

## Message Constants

* * *

### Container Messages

  * [`B_MIME_DATA`](TheInterfaceKit_MessageConstants.md#B_MIME_DATA "B_MIME_DATA")
  * [`B_SIMPLE_DATA`](TheInterfaceKit_MessageConstants.md#B_SIMPLE_DATA "B_SIMPLE_DATA")

A few constants identify messages as data containers. The system currently uses the `B_MIME_DATA` and `B_SIMPLE_DATA` constants to mark the containers it constructs for drag-and-drop operations.

#### B_MIME_DATA

This message constant indicates that all the data in the message is identified by MIME type names. The type code of every data field is `B_MIME_TYPE` and the name of each field is the MIME type string.

As an example, a [`BTextView`](BTextView.md "BTextView") object puts together a `B_MIME_DATA` message for drag-and-drop operations. The message has the text itself in a field named "text/plain"; the [text_run_array](TheInterfaceKit_DefinedTypes.md#text_run_array "text_run_array") structure that describes the character formats of the text is in a field named "application/x-vnd.Be-text_run_array".

#### B_SIMPLE_DATA

This message is a package for a single data element. If there are multiple data fields in the message, they present the same data in various formats.

For example, when the user drags selected files and directories from a Tracker window, the Tracker packages entry_ref references to them in a `B_SIMPLE_DATA` message. The references are in a `refs` array with a type code of [`B_REF_TYPE`](TheSupportKit_Constants.md#B_REF_TYPE). In other words, the message has the same structure as a [`B_REFS_RECEIVED`](TheApplicationKit_MessageConstants.md#B_REFS_RECEIVED "B_REFS_RECEIVED") message, but a different what constant.

* * *

### Selection Messages

  * [`B_CUT`](TheInterfaceKit_MessageConstants.md#B_CUT "B_CUT")
  * [`B_COPY`](TheInterfaceKit_MessageConstants.md#B_COPY "B_COPY")
  * [`B_PASTE`](TheInterfaceKit_MessageConstants.md#B_PASTE "B_PASTE")
  * [`B_SELECT_ALL`](TheInterfaceKit_MessageConstants.md#B_SELECT_ALL "B_SELECT_ALL")

Declared in: `app/AppDefs.h`

These messages are sent to [`BTextView`](BTextView.md "BTextView") and [`BWindow`](BWindow.md "BWindow") classes (which normally passes the message to a [`BTextView`](BTextView.md "BTextView") instance) to cut, copy, paste and select all items respectively.

#### B_CUT

Removes the selected items and moves it to the clipboard.

The Command+X key combination is normally mapped to `B_CUT`

#### B_COPY

Copies the selected items to the clipboard.

The Command+C key combination is normally mapped to `B_COPY`

#### B_PASTE

Pastes items stored in the clipboard into the view.

The Command+V key combination is normally mapped to `B_PASTE`

#### B_SELECT_ALL

Selects all the items in the clipboard into the view.

The Command+A key combination is normally mapped to `B_SELECT_ALL`

* * *

### Status Bar Messages

#### B_UPDATE_STATUS_BAR

Purpose:| Deliverable  
---|---  
Source:| Your app.  
Target:| The [`BStatusBar`](BStatusBar.md "BStatusBar") you're updating.  
Hook:| [`BStatusBar::Update()`](BStatusBar.md#BStatusBar_Update "Update\(\)")  
  
You construct and send this message to a [`BStatusBar`](BStatusBar.md "BStatusBar") object to tell it to (asynchronously) update its progress. To send the message, invoke [`BWindow`](BWindow.md "BWindow")'s [`PostMessage()`](BLooper.md#BLooper_PostMessage "PostMessage\(\)") naming the target [`BStatusBar`](BStatusBar.md "BStatusBar") as the handler:
    
    
    statusBar->Window()->PostMessage(theMessage, statusBar);

Field| Type code| Description  
---|---|---  
`delta`| `B_FLOAT_TYPE`| An increment to the object's current value.  
`text`| `B_STRING_TYPE`| The object's new text (`NULL`-terminated).  
`trailing_text`| `B_STRING_TYPE`| The object's new trailing text (`NULL`-terminated)..  
  
#### B_RESET_STATUS_BAR

Purpose:| Deliverable  
---|---  
Source:| Your app.  
Target:| [`Reset()`](BStatusBar.md#BStatusBar_Reset "Reset\(\)")  
Hook:| The [`BStatusBar`](BStatusBar.md "BStatusBar") you're resetting.  
  
You construct and send this message to a [`BStatusBar`](BStatusBar.md "BStatusBar") object to tell it to (asynchronously) reset itself. The message also lets you reset the object's labels. To send the message, invoke [`BWindow`](BWindow.md "BWindow")'s [`PostMessage()`](BLooper.md#BLooper_PostMessage "PostMessage\(\)") naming the target [`BStatusBar`](BStatusBar.md "BStatusBar") as the handler:
    
    
    statusBar->Window()->PostMessage(theMessage, statusBar);

Field| Type code| Description  
---|---|---  
`label`| `B_STRING_TYPE`| The object's new label (NULL-terminated).  
`trailing_label`| `B_STRING_TYPE`| The object's new trailing label (`NULL`-terminated)..  
  
* * *

### General Constants

#### B_CONTROL_INVOKED

Purpose:| Deliverable  
---|---  
Source:| A [`BControl`](BControl.md "BControl").  
Target:| Selected [`BMessenger`](BMessenger.md "BMessenger").  
  
This message is sent to the targeted messenger when a [`BControl`](BControl.md "BControl")-derived object is invoked.

* * *

### Keyboard Messages

####  B_KEY_DOWN, B_KEY_UP, B_UNMAPPED_KEY_DOWN, B_UNMAPPED_KEY_UP

Purpose:| Deliverable  
---|---  
Source:| The system.  
Target:| The focus view's [`BWindow`](BWindow.md "BWindow").  
Hook:|  [`BView::KeyDown()`](BView.md#BView_KeyDown "KeyDown\(\)") (`B_KEY_DOWN`)  
[`BView::KeyUp()`](BView.md#BView_KeyUp "KeyUp\(\)") (`B_KEY_UP`)  
(The ...`UNMAPPED`... messages don't map to hook functions.)  
  
`B_KEY_DOWN` is sent when the user presses (or holds down) a key that's mapped to a character; `B_KEY_UP` is sent when the user releases the key. `B_UNMAPPED_KEY_DOWN` and `B_UNMAPPED_KEY_UP` are sent if the key isn't mapped to a character. This doesn't include modifier keys, which are reported in the `B_MODIFIERS_CHANGED` message.

Field| Type code| Description  
---|---|---  
`when`| `B_INT64_TYPE`| Event time, in microseconds since 01/01/70  
`key`| `B_INT32_TYPE`| The code for the physical key that was pressed. See <x> for the key map.  
`be:key_repeat` (`B_KEY_DOWN` only)| `B_INT32_TYPE`| The "iteration number" of this key down. When the user holds the key down, successive messages are sent with increasing key repeat values. This field isn't present in the initial event; the first repeat message (i.e., the second key down message) has a key repeat value of 1.  
`modifiers`| `B_INT32_TYPE`| The modifier keys that were in effect at the time of the event. See <x> for a list of values.  
`states`| `B_UINT8_TYPE`| The state of all keys at the time of the event. See <x>.  
`byte` [3] (`B_KEY_DOWN` and `B_KEY_UP` only)| `B_INT8_TYPE`| The UTF8 data that's generated  
`bytes` (`B_KEY_DOWN` and `B_KEY_UP` only)| `B_STRING_TYPE`| The string that's generated. (The string usually contains a single character.)  
`raw_char` (`B_KEY_DOWN` and `B_KEY_UP` only)| `B_INT32_TYPE`| Modifier-independent ASCII code for the character.  
  
#### B_MODIFIERS_CHANGED

Purpose:| Deliverable  
---|---  
Source:| The system.  
Target:| The focus view's window.  
Hook:|   
  
Sent when the user presses or releases a modifier key.

Field| Type code| Description  
---|---|---  
`when`| `B_INT64_TYPE`| Event time, in microseconds since 01/01/70  
`modifiers`| `B_INT32_TYPE`| The current modifier keys.  
`be:old_modifiers`| `B_INT32_TYPE`| The previous modifier keys.  
`states`| `B_UINT8_TYPE`| The state of all keys at the time of the event. See <x>.  
  
* * *

### Mouse Messages

#### B_MOUSE_DOWN

Purpose:| Deliverable  
---|---  
Source:| The system.  
Target:| The [`BWindow`](BWindow.md "BWindow") of the view the mouse is pointing to.  
Hook:| [`BView::MouseDown()`](BView.md#BView_MouseDown "MouseDown\(\)")  
  
Sent when the user presses a mouse button. This message is only sent if no other mouse button is already down.

Field| Type code| Description  
---|---|---  
`when`| `B_INT64_TYPE`| Event time, in microseconds since 01/01/70  
`where`| `B_POINT_TYPE`| Mouse location in the view's coordinate system.  
`modifiers`| `B_INT32_TYPE`| The modifier keys that were in effect at the time of the event.  
`buttons`| `B_INT32_TYPE`| The mouse button that was pressed, one of: `B_PRIMARY_MOUSE_BUTTON`, `B_SECONDARY_MOUSE_BUTTON`, `B_TERTIARY_MOUSE_BUTTON`  
`clicks`| `B_INT32_TYPE`| 1 for a single-click, 2 for double-click, 3 for triple-click, and so on. The counter is reset if the time between clicks exceeds the "Double-click speed" set by the user in the Mouse preferences. Note that the counter is not reset if the mouse moves between clicks.  
  
#### B_MOUSE_MOVED

Purpose:| Deliverable  
---|---  
Source:| The system.  
Target:| The [`BWindow`](BWindow.md "BWindow") of the view the mouse is pointing to.  
Hook:| [`BView::MouseMoved()`](BView.md#BView_MouseMoved "MouseMoved\(\)")  
  
Sent when the user moves the mouse.

Field| Type code| Description  
---|---|---  
`when`| `B_INT64_TYPE`| Event time, in microseconds since 01/01/70  
`where`| `B_POINT_TYPE`| The mouse's new location in window coordinates.  
`buttons`| `B_INT32_TYPE`| The mouse buttons that are down. Zero or more of: `B_PRIMARY_MOUSE_BUTTON`, `B_SECONDARY_MOUSE_BUTTON`, `B_TERTIARY_MOUSE_BUTTON`  
  
#### B_MOUSE_UP

Purpose:| Deliverable  
---|---  
Source:| The system.  
Target:| [`BWindow`](BWindow.md "BWindow") of the view the mouse is pointing to.  
Hook:| [`BView::MouseUp()`](BView.md#BView_MouseUp "MouseUp\(\)")  
  
Sent when the user releases a mouse button. It's only sent if no other mouse button remains down.

Field| Type code| Description  
---|---|---  
`when`| `B_INT64_TYPE`| Event time, in microseconds since 01/01/70  
`where`| `B_POINT_TYPE`| Mouse location in the view's coordinate system.  
`modifiers`| `B_INT32_TYPE`| The modifier keys that were in effect at the time of the event.  
  
#### B_MOUSE_WHEEL_CHANGED

Purpose:| Deliverable  
---|---  
Source:| The system.  
Target:| The [`BWindow`](BWindow.md "BWindow") of the view the mouse is pointing to.  
Hook:|   
  
Sent when the user moves the mouse wheel (on mice that have them).

Field| Type code| Description  
---|---|---  
`when`| `B_INT64_TYPE`| Event time, in microseconds since 01/01/70  
`be:wheel_delta_x`| `B_FLOAT_TYPE`| How much the Y value of the wheel has changed.  
`be:wheel_delta_y`| `B_FLOAT_TYPE`| How much the Y value of the wheel has changed.  
  
The standard mouse driver that comes with BeOS only supports a Y-oriented wheel.

* * *

### B_PRINTER_CHANGED

Purpose:| Deliverable  
---|---  
Source:| The Print Server.  
Target:| Everyone.  
Hook:|   
  
Sent whenever the user changes printers. Applications that support printing should watch for this message, and if they receive it, they should suggest that the user check their page setup the next time they choose to print.

* * *

### B_SCREEN_CHANGED

Purpose:| Deliverable  
---|---  
Source:| The system.  
Target:| Every [`BWindow`](BWindow.md "BWindow") in the screen that changed (even hidden windows).  
Hook:| [`BWindow::ScreenChanged()`](BWindow.md#BWindow_ScreenChanged "ScreenChanged\(\)")  
  
Sent when the screen's dimensions or color space changes—because the user played with the Screen preferences app, for example.

Field| Type code| Description  
---|---|---  
`when`| `B_INT64_TYPE`| Event time, in microseconds since 01/01/70  
`frame`| `B_RECT_TYPE`| The screen's dimensions.  
`mode`| `B_INT32_TYPE`| The screen's color space: `B_CMAP8`, `B_RGB15`, `B_RGB15`, or `BRGB32`.  
  
* * *

### B_VALUE_CHANGED

Purpose:| Deliverable  
---|---  
Source:| The system.  
Target:| The manipulated scrollbar's [`BWindow`](BWindow.md "BWindow").  
Hook:| [`BScrollBar::ValueChanged()`](BScrollBar.md#BScrollBar_ValueChanged "ValueChanged\(\)")  
  
Sent when the user plays with a scrollbar.

Field| Type code| Description  
---|---|---  
`when`| `B_INT64_TYPE`| Event time, in microseconds since 01/01/70  
`value`| `B_INT32_TYPE`| The scrollbar's new value.  
  
* * *

### View Messages

#### B_VIEW_MOVED

Purpose:| Deliverable  
---|---  
Source:| The system.  
Target:| The moved view's [`BWindow`](BWindow.md "BWindow").  
Hook:| [`BView::FrameMoved()`](BView.md#BView_FrameMoved "FrameMoved\(\)")  
  
Sent when a view's origin (left top corner) changes relative to the origin of its parent. The message isn't sent if the view doesn't have the `B_FRAME_EVENTS` flag set.

Field| Type code| Description  
---|---|---  
`when`| `B_INT64_TYPE`| Event time, in microseconds since 01/01/70  
`where`| `B_POINT_TYPE`| The view's new origin in the coordinate system of its parent.  
  
#### B_VIEW_RESIZED

Purpose:| Deliverable  
---|---  
Source:| The system.  
Target:| The resized view's [`BWindow`](BWindow.md "BWindow").  
Hook:| [`BView::FrameResized()`](BView.md#BView_FrameResized "FrameResized\(\)")  
  
Sent when the size of the view's frame changes. The message isn't sent if the view doesn't have the `B_FRAME_EVENTS` flag set.

Field| Type code| Description  
---|---|---  
`when`| `B_INT64_TYPE`| Event time, in microseconds since 01/01/70  
`width`| `B_INT32_TYPE`| The view's new width.  
`height`| `B_INT32_TYPE`| The view's new height.  
`where`| `B_POINT_TYPE`| The view's new origin expressed in the coordinate system of its parent. This field is only included if the view actually moved while being resized, and can always be ignored: If the view did move, you'll hear about it in a separate [`B_VIEW_MOVED`](TheInterfaceKit_MessageConstants.md#B_VIEW_MOVED "B_VIEW_MOVED") [`BMessage`](BMessage.md "BMessage").  
  
* * *

### Window Messages

#### B_WINDOW_ACTIVATED

Purpose:| Deliverable  
---|---  
Source:| The system.  
Target:| The activated/deactivated [`BWindow`](BWindow.md "BWindow").  
Hook:| [`BWindow::WindowActivated()`](BWindow.md#BWindow_WindowActivated "WindowActivated\(\)") and [`BView::WindowActivated()`](BView.md#BView_WindowActivated "WindowActivated\(\)")  
  
Sent just after a window is activated or deactivated. Note that the [`BWindow`](BWindow.md "BWindow") invokes [`WindowActivated()`](BView.md#BView_WindowActivated "WindowActivated\(\)") on each of its views.

Field| Type code| Description  
---|---|---  
`when`| `B_INT64_TYPE`| Event time, in microseconds since 01/01/70  
`active`| `B_BOOL_TYPE`| `true` if the window is now active; `false` if not.  
  
#### B_WINDOW_MOVE_BY

Purpose:| Deliverable.  
---|---  
Source:| Your application.  
Target:| The [`BWindow`](BWindow.md "BWindow") to be moved.  
Hook:|   
  
You can send this message to a window to resize it by the specified deltas.

Field| Type code| Description  
---|---|---  
`data`| `B_POINT_TYPE`| The amount by which to move the window's X and Y coordinates.  
  
#### B_WINDOW_MOVE_TO

Purpose:| Deliverable.  
---|---  
Source:| Your application.  
Target:| The [`BWindow`](BWindow.md "BWindow") to be moved.  
Hook:|   
  
You can send this message to a window to resize it to the specified size.

Field| Type code| Description  
---|---|---  
`data`| `B_POINT_TYPE`| The width and height (in X and Y) to resize the window to.  
  
#### B_WINDOW_MOVED

Purpose:| Deliverable  
---|---  
Source:| The system.  
Target:| [`BWindow`](BWindow.md "BWindow") that moved.  
Hook:| [`BWindow::FrameMoved()`](BWindow.md#BWindow_FrameMoved "FrameMoved\(\), FrameResized\(\)")  
  
Sent when a window's origin (left top corner) changes within the screen coordinate system.

Field| Type code| Description  
---|---|---  
`when`| `B_INT64_TYPE`| Event time, in microseconds since 01/01/70  
`where`| `B_POINT_TYPE`| The window's new origin in screen coordinates.  
  
#### B_WINDOW_RESIZED

Purpose:| Deliverable  
---|---  
Source:| The system.  
Target:| The resized [`BWindow`](BWindow.md "BWindow").  
Hook:| [`BWindow::FrameResized()`](BWindow.md#BWindow_FrameResized)  
  
Sent when the size of the window's frame changes. Note that the `width` and `height` fields measure the window's content area—they don't include the window border or window tab.

Field| Type code| Description  
---|---|---  
`when`| `B_INT64_TYPE`| Event time, in microseconds since 01/01/70  
`width`| `B_INT32_TYPE`| The width of the window's content area.  
`height`| `B_INT32_TYPE`| The height of the window's content area.  
  
#### B_WORKSPACE_ACTIVATED

Purpose:| Deliverable  
---|---  
Source:| The system.  
Target:| Every [`BWindow`](BWindow.md "BWindow") in the activated and deactivated workspaces.  
Hook:| [`BWindow::WorkspaceActivated()`](BWindow.md#BWindow_WorkspaceActivated "WorkspaceActivated\(\)")  
  
Sent when the active workspace changes.

Field| Type code| Description  
---|---|---  
`when`| `B_INT64_TYPE`| Event time, in microseconds since 01/01/70  
`workspace`| `B_INT32_TYPE`| The index of the window's workspace.  
`active`| `B_BOOL_TYPE`| `true` if the workspace is now active; `false` if not.  
  
#### B_WORKSPACES_CHANGED

Purpose:| Deliverable  
---|---  
Source:| The system.  
Target:| The [`BWindow`](BWindow.md "BWindow") whose set of workspaces changed.  
Hook:| [`BWindow::WorkspacesChanged()`](BWindow.md#BWindow_WorkspacesChanged "WorkspacesChanged\(\)")  
  
Sent when there's a change to the set of workspaces in which a window can appear.

Field| Type code| Description  
---|---|---  
`when`| `B_INT64_TYPE`| Event time, in microseconds since 01/01/70  
`old`| `B_INT32_TYPE`| The window's old workspace set, given as a vector of workspace indices.  
`new`| `B_INT32_TYPE`| The window's new workspace set, given as a vector of workspace indices.  
  
#### B_ZOOM

Purpose:| Deliverable  
---|---  
Source:| The system.  
Target:| The [`BWindow`](BWindow.md "BWindow") that was zoomed.  
Hook:| [`BWindow::Zoom()`](BWindow.md#BWindow_Zoom "Zoom\(\)")  
  
Sent when the user clicks a window's zoom button.

The message has just one data field:

Field| Type code| Description  
---|---|---  
`when`| `B_INT64_TYPE`| Event time, in microseconds since 01/01/70  
  
#### B_OPEN_IN_WORKSPACE

Purpose:| Deliverable  
---|---  
Source:| The system.  
Target:| [`BApplication`](BApplication.md "BApplication").  
Hook:|   
  
Sent to an application when it's first launched to tell it to open in a specific workspace. The message will be handled during the construction of the [`BApplication`](BApplication.md "BApplication") object.

Field| Type code| Description  
---|---|---  
`be:workspace`| `B_INT32_TYPE`| Workspace number into which the application should open.  
  
#### B_MINIMIZE

Purpose:| Deliverable  
---|---  
Source:| The system or your app.  
Target:| The [`BWindow`](BWindow.md "BWindow") that's hidden/unhidden.  
Hook:| [`BWindow::Minimize()`](BWindow.md#BWindow_Minimize)  
  
Sent when the user double-clicks a window's title bar (to hide the window), or selects a window from the DeskBar's window list (to unhide the window).

Field| Type code| Description  
---|---|---  
`when`| `B_INT64_TYPE`| Event time, in microseconds since 01/01/70.  
`minimize`| `B_BOOL_TYPE`| `true` if the window is being hidden; `false` if it's being unhidden.  
  
* * *

### BShelf Messages

#### B_ARCHIVED_OBJECT

Purpose:| Deliverable and format  
---|---  
Source:| A dragged replicant, or your app.  
Target:| A (remote) application.  
Hook:| [`BShelf::CanAcceptReplicantMessage()`](BShelf.md#BShelf_CanAcceptReplicantMessage "CanAcceptReplicantMessage\(\), CanAcceptReplicantView\(\)")  
  
##### As a deliverable:

The replicant system uses this message as a deliverable. If you're using [`BDragger`](BDragger.md "BDragger") and [`BShelf`](BShelf.md "BShelf") objects, the message is created and delivered for you. You can also simulate a dragged replicant by archiving a view, setting the archive message's command to `B_ARCHIVED_OBJECT`, and sending the message to a remote application. If the remote application has a [`BShelf`](BShelf.md "BShelf") object, the [`BShelf`](BShelf.md "BShelf") will pick up the message (through a [`BMessageFilter`](BMessageFilter.md "BMessageFilter")) and pass it to the hook function.

To create a simulated replicant message, you call [`Archive()`](BView.md#BView_Archive "Archive\(\)") on the view that you want to replicate, and add (at least) the `add_on` field to the archive message.

See [`BShelf`](BShelf.md "BShelf") and [`BDragger`](BDragger.md "BDragger") for more information about replicants.

##### As a format:

`B_ARCHIVED_OBJECT` should be used as the command constant for all archive messages. When you archive an object, the `class` field is automatically added to the archive message. All other fields must be added by your archiving code. See the [`BArchivable`](BArchivable.md "BArchivable") class for more information about archiving.

Field| Type code| Description  
---|---|---  
`class` [ ]| `B_STRING_TYPE`| An array of class names that gives the class hierarchy of the archived object.  
`add_on`| `B_STRING_TYPE`| The signature of the library or application that knows how to create the archived object.  
`be:add_on_version`| `B_INT32_TYPE`| The version of the add_on.  
`be:load_each_time`| `B_BOOL_TYPE`| `true`: The add_on is loaded each time the object is unarchived. `false`: The add_on is loaded only if it isn't already loaded.  
`be:unload_on_delete`| `B_BOOL_TYPE`| Is the add_on unloaded when the unarchived object is deleted?  
`shelf_type` (replicants only; optional)| `B_STRING_TYPE`| The `type` of shelf that you want to have display the replicant. A shelf's type is its name, as assigned when it's created.

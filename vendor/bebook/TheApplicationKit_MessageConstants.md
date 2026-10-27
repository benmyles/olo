<!-- Classes And Methods - The Application Kit - Message Constants -->

## Message Constants

The following sections describe Application Kit constants which can be either used as a return value from a method or as a `what` data member of a [`BMessage`](BMessage.md "BMessage").

* * *

### Application Messages

These constants represent the system messages that are recognized and given special treatment by [`BApplication`](BApplication.md "BApplication"). Application messages concern the application as a whole, rather than any particular window thread. See the introduction to this chapter and the [`BApplication`](BApplication.md "BApplication") class for details.

#### B_ABOUT_REQUESTED

Purpose:| Deliverable  
---|---  
Source:| The system or your app.  
Target:| App-defined; typically [`be_app`](BApplication.md#BApplication_be_app "be_app").  
Hook:| [`BApplication::AboutRequested()`](BApplication.md#BApplication_AboutRequested "AboutRequested\(\)") if the target is [`be_app`](BApplication.md#BApplication_be_app "be_app").  
  
The message should be assigned to an About... menu item, such that the message is sent when the user clicks the item. Your application is expected to put up an "About This Application" panel when it receives this message.

#### B_APP_ACTIVATED

Purpose:| Deliverable  
---|---  
Source:| The system.  
Target:| [`be_app`](BApplication.md#BApplication_be_app "be_app")  
Hook:| [`BApplication::AppActivated()`](BApplication.md#BApplication_AppActivated "AppActivated\(\)")  
  
Sent when an application becomes active or inactive.

Field| Type code| Description  
---|---|---  
`active`| `B_BOOL_TYPE`| `true` if the app has become active; otherwise `false`.  
  
#### B_ARGV_RECEIVED

Purpose:| Deliverable  
---|---  
Source:| The system.  
Target:| [`be_app`](BApplication.md#BApplication_be_app "be_app")  
Hook:| [`BApplication::ArgvReceived()`](BApplication.md#BApplication_ArgvReceived "ArgvReceived\(\)")  
  
Forwards arguments that (a) the user passes while launching the app from the command line, or (b) are passed to [`BRoster::Launch()`](BRoster.md#BRoster_Launch "Launch\(\)"). Most apps treat command line arguments as filenames that should be opened. If the filename is relative (if it doesn't start with "/"), you should append it to the `cwd` field to reconstruct the full path.

Field| Type code| Description  
---|---|---  
`argc`| `B_INT32_TYPE`| The number of arguments.  
`argv` [`argc`]| `B_STRING_TYPE`| The arguments.  
`cwd`| `B_STRING_TYPE`| The path name of the current working directory.  
  
#### B_PULSE

Purpose:| Deliverable  
---|---  
Source:| The system.  
Target:| [`be_app`](BApplication.md#BApplication_be_app "be_app") or [`BWindow`](BWindow.md "BWindow") object.  
Hook:| [`BApplication::Pulse()`](BApplication.md#BApplication_Pulse "Pulse\(\)") and [`BView::Pulse()`](BView.md#BView_Pulse "Pulse\(\)")  
  
Sent at regular intervals, but with no particular intent. You can implement `Pulse()` to do whatever you want. The message is to the [`BWindow`](BWindow.md "BWindow") only if a [`BView`](BView.md "BView") within the window declares `B_PULSE_NEEDED` in its constructor flags.

#### B_READY_TO_RUN

Purpose:| Deliverable  
---|---  
Source:| The system.  
Target:| [`be_app`](BApplication.md#BApplication_be_app "be_app")  
Hook:| [`BApplication::ReadyToRun()`](BApplication.md#BApplication_ReadyToRun "ReadyToRun\(\)")  
  
Sent when an application has finished configuring itself and is ready to start running.

(No Be-defined fields)

#### B_REFS_RECEIVED

Purpose:| Deliverable  
---|---  
Source:| The system or your app.  
Target:| [`be_app`](BApplication.md#BApplication_be_app "be_app"), or other app-defined target.  
Hook:| [`BApplication::RefsReceived()`](BApplication.md#BApplication_RefsReceived "RefsReceived\(\)")  
  
Automatically sent to be_app when (a) the user double-clicks a file that has a type that's supported by the app, and (b) when the user confirms some files (or directories) in an Open File panel (the target is be_app by default; it can be changed in [`BFilePanel::SetTarget()`](BFilePanel.md#BFilePanel_SetTarget "SetTarget\(\), Messenger\(\)")). You can also create, stuff, and send a [`B_REFS_RECEIVED`](TheApplicationKit_MessageConstants.md#B_REFS_RECEIVED "B_REFS_RECEIVED") message yourself. When it receives this message, an app is expected to open the files that the message refers to.

Field| Type code| Description  
---|---|---  
`refs` [i]| `B_REF_TYPE`| entry_ref items, one for each file or directory.  
  
See also: "[Application Messages](BApplication_Overview.md#BApplication_Overview_Application_Messages "Application Messages")"

* * *

### BLooper Messages

#### B_QUIT_REQUESTED

Purpose:| Deliverable  
---|---  
Source:| The system or your app.  
Target:| [`be_app`](BApplication.md#BApplication_be_app "be_app"), [`BWindow`](BWindow.md "BWindow") closed by the user, or other [`BLooper`](BLooper.md "BLooper") object.  
Hook:| [`BLooper::QuitRequested()`](BLooper.md#BLooper_QuitRequested "QuitRequested\(\)").  
  
Automatically sent (a) to [`be_app`](BApplication.md#BApplication_be_app "be_app") when the user types Command+q, and (b) to a [`BWindow`](BWindow.md "BWindow") when the user clicks the window's close box. Applications can also manufacture and send the message themselves. A looper that receives this message is expected to quit, or at least consider quitting.

Field| Type code| Description  
---|---|---  
`shortcut`| `B_BOOL_TYPE`| `true` if the user typed Command+q.  
  
* * *

### Scripting Messages

The scripting system defines four generic messages that can operate on the specific properties of an object and two meta-messages that query an object about the messages it can handle. See "[Scripting](TheApplicationKit_Scripting.md "Scripting")" in The Application Kit chapter for a full explanation.

#### B_COUNT_PROPERTIES

This message requests the number of properties supported by the receiver. It contains no data, but the reply message should contain one field:

Field| Type code| Description  
---|---|---  
`result`| `B_INT32_TYPE`| The number of properties supported.  
  
#### B_GET_SUPPORTED_SUITES

This message requests the names of all message suites that the receiver supports. It doesn't contain any data, but the message that's sent in reply has one field:

Field| Type code| Description  
---|---|---  
`suites`| `B_STRING_TYPE`| An array of suite names.  
  
A suite is a named set of messages and specifiers. A [`BHandler`](BHandler.md "BHandler") supports the suite if it can respond to the messages and resolve the specifiers.

####  B_SET_PROPERTY, B_GET_PROPERTY, B_CREATE_PROPERTY, B_DELETE_PROPERTY

These messages—as their names state—target a particular property under the control of the target handler. They have the following data fields:

Field| Type code| Description  
---|---|---  
`specifiers`| `B_MESSAGE_TYPE`| An array of one or more [`BMessage`](BMessage.md "BMessage")s that specify the targeted property. See [`AddSpecifier()`](BMessage.md#BMessage_AddSpecifier "AddSpecifier\(\)") in the [`BMessage`](BMessage.md "BMessage") class of the Application Kit for details on the contents of a specifier.  
`data`|  _variable_|  For `B_SET_PROPERTY` messages only, the data that should be set. The data type depends on the targeted property.  
  
* * *

### Reply Messages

The following three messages are sent as replies to other messages.

#### B_MESSAGE_NOT_UNDERSTOOD

This message doesn't contain any data. The system sends it as a reply to a message that the receiving thread's chain of [`BHandler`](BHandler.md "BHandler") does not recognize. See [`MessageReceived()`](BHandler.md#BHandler_MessageReceived "MessageReceived\(\)") and [`ResolveSpecifier()`](BHandler.md#BHandler_ResolveSpecifier "ResolveSpecifier\(\)") in the [`BHandler`](BHandler.md "BHandler") class of the Application Kit.

#### B_NO_REPLY

This message doesn't contain any data. It's sent as a default reply to another message when the original message is about to be deleted. The default reply is sent only if a synchronous reply is expected and none has been sent. See the [`SendReply()`](BMessage.md#BMessage_SendReply "SendReply\(\)") function in the [`BMessage`](BMessage.md "BMessage") class of the Application Kit.

#### B_REPLY

This constant identifies a message as being a reply to a previous message. The data in the reply depends on the circumstances and, particularly, on the original message. For replies to scripting messages, it generally has a `result` field with requested data and an `error` field with an error code reporting the success or failure of the scripted request.

* * *

### General Messages

#### B_CLIPBOARD_CHANGED

Purpose:| Deliverable  
---|---  
Source:| The Application Server.  
Target:| Selected [`BMessenger`](BMessenger.md "BMessenger").  
  
If you've called [`BClipboard::StartWatching()`](BClipboard.md#BClipboard_StartWatching "StartWatching\(\), StopWatching\(\)") to monitor a clipboard for changes, this message is sent to the specified [`BMessenger`](BMessenger.md "BMessenger") when the clipboard changes.

#### B_OBSERVER_NOTICE_CHANGE

Purpose:| Deliverable  
---|---  
Source:| The system.  
Target:| Application-defined target.  
Hook:|   
  
Sent by [`BHandler`](BHandler.md "BHandler")s to all targets that have been registered with the [`BHandler`](BHandler.md "BHandler")'s [`StartWatching()`](BHandler.md#BHandler_StartWatching "StartWatching\(\), StartWatchingAll\(\), StopWatching\(\), StopWatchingAll\(\)") or [`StartWatchingAll()`](BHandler.md#BHandler_StartWatchingAll) function.

Field| Type code| Description  
---|---|---  
`be:observe_change_what`| `B_INT32_TYPE`| The `what` code of the message being broadcast.  
`be:observe_orig_what`| `B_INT32_TYPE`| The original `what` code of the message, before it was altered by the broadcasting mechanism.  
  
The message may have other fields, depending on what the broadcast message is. Messages are sent to targets in response to the [`BHandler::SendNotices()`](BHandler.md#BHandler_SendNotices "SendNotices\(\)") function. The logic is:

  1. Take the message to broadcast and place its what code into the `be:observe_change_orig_what` field.

  2. Add the `be:observe_change_what` field, which is set to the `what` code specified by the call to [`SendNotices()`](BHandler.md#BHandler_SendNotices "SendNotices\(\)").

  3. Set the message's `what` code to `B_OBSERVER_NOTICE_CHANGE`.

The resulting message is then sent to all interested parties.

#### B_SILENT_RELAUNCH

Purpose:| Deliverable  
---|---  
Source:| The system.  
Target:| [`be_app`](BApplication.md#BApplication_be_app "be_app")  
Hook:|   
  
Sent to a single-launch application when it's activated by being launched (for example, if the user double-clicks its icon in Tracker).

(No Be-defined fields)

####  B_SOME_APP_ACTIVATED, B_SOME_APP_LAUNCHED, B_SOME_APP_QUIT

Purpose:| Deliverable  
---|---  
Source:| The Roster Monitor.  
Target:| App-defined.  
Hook:|   
  
Sent as apps are launched, activated, or quit. You get these messages by invoking [`BRoster::StartWatching()`](BRoster.md#BRoster_StartWatching "StartWatching\(\) , StopWatching\(\)") passing a one or more of [`B_REQUEST_ACTIVATED`](BRoster.md#B_REQUEST_ACTIVATED), [`B_REQUEST_LAUNCHED`](BRoster.md#B_REQUEST_LAUNCHED), and [`B_REQUEST_QUIT`](BRoster.md#B_REQUEST_QUIT).

Field| Type code| Description  
---|---|---  
`mime_sig`| `B_STRING_TYPE`| The app signature.  
`team`| `B_INT32_TYPE`| The app's team id.  
`thread`| `B_INT32_TYPE`| The id of the app's main thread.  
`flags`| `B_INT32_TYPE`| The app's app flags (`B_SINGLE_LAUNCH`, `B_BACKGROUND_APP`, etc).  
`ref`| `B_REF_TYPE`| The entry_ref of the app's executable.  
  
#### B_CANCEL

Purpose:| Deliverable  
---|---  
Source:| The Application Kit.  
Target:| Application Server.  
  
Used to cancel an ongoing operation. The Application Kit sends this message to the Application Server to cancel a shutdown if a window refuses to quit.

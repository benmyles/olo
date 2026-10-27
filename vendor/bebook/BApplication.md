<!-- Classes And Methods - The Application Kit - BApplication -->

## BApplication

| Derived From:| [`BLooper`](BLooper.md "BLooper") > [`BHandler`](BHandler.md "BHandler")  
---|---  
Mix-in Classes:| [`BArchivable`](BArchivable.md "BArchivable")  
Declared In:| `app/Application.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BApplication_Overview.md "BApplication")

  * [Constructor and Destructor](BApplication.md#BApplication_ConstructorDestructor)
  * [Hook Functions](BApplication.md#BApplication_HookFunctions)
  * [Member Functions](BApplication.md#BApplicationMemberFunctions)
  * [Static Functions](BApplication.md#BApplication_StaticFunctions)
  * [Global Variables](BApplication.md#BApplication_GlobalVariables)
  * [Archived Fields](BApplication.md#BApplication_ArchivedFields)
  * [Scripting Suites and Properties](BApplication.md#BApplication_ScriptingSuites)

  
  
* * *

### Constructor and Destructor

#### BApplication()

`BApplication(const char* signature);` `BApplication(const char* signature,  
status_t* error);` `BApplication(BMessage* archive);`

The constructor creates a new object, locks it, sets the global variable [`be_app`](BApplication.md#BApplication_be_app "be_app") to point to it, and establishes a connection to the Application Server. From this point on, your application can receive messages, although it won't start processing them until you call [`Run()`](BApplication.md#BApplication_Run "Run\(\), Quit\(\)"). You can also begin creating and displaying [`BWindow`](BWindow.md "BWindow") objects even before you call [`Run()`](BApplication.md#BApplication_Run "Run\(\), Quit\(\)").

The `signature` constructors assign the argument as the app's application signature. The argument is ignored if a signature is already specified in a resource or attribute of the application's executable (serious apps should always set the signature as both an attribute and a resource). The signature is a MIME type string that must have the supertype "application". For more information on application signatures and how to set them, see **TODO**.

If you specify `error`, a pointer to a status_t, any error that occurs while constructing the `BApplication` will be returned in that variable. Alternately, you can call [`InitCheck()`](BApplication.md#BApplication_InitCheck "InitCheck\(\)") to check the results. If an error is returned by the constructor, you shouldn't call [`Run()`](BApplication.md#BApplication_Run "Run\(\), Quit\(\)").

The `archive` constructor is an implementation detail; see the [BArchivable](BArchivable.md "BArchivable") class.

#### ~BApplication()

`virtual ~BApplication();`

Closes and deletes the application's [`BWindow`](BWindow.md "BWindow")s (and the [`BView`](BView.md "BView")s they contain), and severs the application's connection to the Application Server.

Never delete a `BApplication` object while it's running; wait until [`Run()`](BApplication.md#BApplication_Run "Run\(\), Quit\(\)") returns. To stop a `BApplication` (and so cause [`Run()`](BApplication.md#BApplication_Run "Run\(\), Quit\(\)") to return), send it a [`B_QUIT_REQUESTED`](TheApplicationKit_MessageConstants.md#B_QUIT_REQUESTED "B_QUIT_REQUESTED") message:
    
    
    be_app->PostMessage(B_QUIT_REQUESTED);

* * *

### Hook Functions

#### AboutRequested()

`virtual void AboutRequested();`

Hook function that's invoked when the `BApplication` receives a [`B_ABOUT_REQUESTED`](TheApplicationKit_MessageConstants.md#B_ABOUT_REQUESTED "B_ABOUT_REQUESTED") message, undoubtedly because the user clicked an "About…" menu item. You should implement the function to put a window on-screen that provides the user with information about the application (version number, license restrictions, authors' names, etc).

#### AppActivated()

`virtual void AppActivated(bool active);`

Hook function that's invoked when the application receives a [`B_APP_ACTIVATED`](TheApplicationKit_MessageConstants.md#B_APP_ACTIVATED "B_APP_ACTIVATED") message. The message is sent when the application gains or loses active application status. The `active` flag tells you which way the wind blows: `true` means your application is now active; `false` means it isn't.

The user can activate an application by clicking on or unhiding one of its windows; you can activate an application programmatically by calling [`BWindow::Activate()`](BWindow.md#BWindow_Activate "Activate\(\), IsActive\(\)") or [`BRoster::ActivateApp()` ](BRoster.md#BRoster_ActivateApp "ActivateApp\(\)"). (With regard to the latter: This function is called only if the application has an "activatable" window i.e. a non-modal, non-floating window).

During launch, this function is called after [`ReadyToRun()`](BApplication.md#BApplication_ReadyToRun "ReadyToRun\(\)") (provided the application is displaying an activatable window).

#### ArgvReceived()

`virtual void ArgvReceived(int32 argc,  
char** argv);`

Hook function that's invoked when the application receives a [`B_ARGV_RECEIVED`](TheApplicationKit_MessageConstants.md#B_ARGV_RECEIVED "B_ARGV_RECEIVED") message. The message is sent if command line arguments are used in launching the application from the shell, or if `argv`/`argc` values are passed to [`BRoster::Launch()`](BRoster.md#BRoster_Launch "Launch\(\)").

Warning

![Warning](./images/admonitions/Stop_32.png)

This function isn't called if there were no command line arguments, or if [`BRoster::Launch()`](BRoster.md#BRoster_Launch "Launch\(\)") was called without `argv`/`argc` values.

When the application is launched from the shell, `ArgvReceived()`'s arguments are identical to the traditional `main()` arguments: The number of command line arguments is passed as `argc`; the arguments themselves are passed as an array of strings in `argv`. The first `argv` string identifes the executable file; the other strings are the command line arguments proper. For example, this…
    
    
    $ MyApp file1 file2

…produces the `argv` array { "./MyApp", "file1", "file2" }.

[`BRoster::Launch()`](BRoster.md#BRoster_Launch "Launch\(\)") forwards its `argv` and `argc` arguments, but adds the executable name to the front of the `argv` array and increments the `argc` value.

Normally, the [`B_ARGV_RECEIVED`](TheApplicationKit_MessageConstants.md#B_ARGV_RECEIVED "B_ARGV_RECEIVED") message (if sent at all) is sent once, just before [`B_READY_TO_RUN`](TheApplicationKit_MessageConstants.md#B_READY_TO_RUN "B_READY_TO_RUN") is sent. However, if the user tries to re-launch (from the command line and with arguments) an already-running application that's set to [`B_EXCLUSIVE_LAUNCH`](BRoster.md#B_EXCLUSIVE_LAUNCH) or [`B_SINGLE_LAUNCH`](BRoster.md#B_SINGLE_LAUNCH), the re-launch will generate a [`B_ARGV_RECEIVED`](TheApplicationKit_MessageConstants.md#B_ARGV_RECEIVED "B_ARGV_RECEIVED") message that's sent to the already-running image. Thus, for such apps, the [`B_ARGV_RECEIVED`](TheApplicationKit_MessageConstants.md#B_ARGV_RECEIVED "B_ARGV_RECEIVED") message can show up at any time.

#### Pulse()

`virtual void Pulse();`

`Pulse()` is a hook function that's called when the application receives a [`B_PULSE`](TheApplicationKit_MessageConstants.md#B_PULSE "B_PULSE") message. The message is sent at the rate set in [`SetPulseRate()`](BApplication.md#BApplication_SetPulseRate "SetPulseRate\(\)"). The first `Pulse()` message is sent after [`ReadyToRun()`](BApplication.md#BApplication_ReadyToRun "ReadyToRun\(\)") returns.

You can implement `Pulse()` to do whatever you want (the default version does nothing), but don't try to use it for precision timing: The pulse granularity is no better than 100,000 microseconds.

Keep in mind that `Pulse()` executes in the app's message loop thread along with all other message handling functions. Your application won't receive any `Pulse()` invocations while it's waiting for some other handler function (including [`MessageReceived()`](BApplication.md#BApplication_MessageReceived "MessageReceived\(\)")) to finish. In the meantime, [`B_PULSE`](TheApplicationKit_MessageConstants.md#B_PULSE "B_PULSE") messages will be stacking up in the message queue; when the loop becomes "unblocked", you'll see a burst of `Pulse()` invocations.

#### QuitRequested()

`virtual bool QuitRequested();`

Hook function that's invoked when the application receives a [`B_QUIT_REQUESTED`](TheApplicationKit_MessageConstants.md#B_QUIT_REQUESTED "B_QUIT_REQUESTED") message. As described in the [`BLooper`](BLooper.md "BLooper") class (which declares this function), the request to quit is confirmed if `QuitRequested()` returns `true`, and denied if it returns `false`.

In its implementation, `BApplication` sends [`BLooper::QuitRequested()`](BLooper.md#BLooper_QuitRequested "QuitRequested\(\)") to each of its [`BWindow`](BWindow.md "BWindow") objects. If they all agree to quit, the windows are all destroyed (through [`BWindow::Quit()`](BWindow.md#BWindow_Quit "Quit\(\)")) and this `QuitRequested()` returns `true`. But if any [`BWindow`](BWindow.md "BWindow") refuses to quit, that window and all surviving windows are saved, and this `QuitRequested()` returns `false`.

Augment this function as you will, but be sure to call the `BApplication` version in your implementation.

#### ReadyToRun()

`virtual void ReadyToRun();`

Hook function that's called when the application receives a [`B_READY_TO_RUN`](TheApplicationKit_MessageConstants.md#B_READY_TO_RUN "B_READY_TO_RUN") message. The message is sent automatically during the [`Run()`](BApplication.md#BApplication_Run "Run\(\), Quit\(\)") function, and is sent after the initial [`B_REFS_RECEIVED`](TheApplicationKit_MessageConstants.md#B_REFS_RECEIVED "B_REFS_RECEIVED") and [`B_ARGV_RECEIVED`](TheApplicationKit_MessageConstants.md#B_ARGV_RECEIVED "B_ARGV_RECEIVED") messages (if any) have been handled. This is the only application message that every running application is guaranteed to receive.

What you do with `ReadyToRun()` is up to you, if your application hasn't put up a window by the time this function is called, you'll probably want to do it here. The default version of `ReadyToRun()` is empty.

#### RefsReceived()

`virtual void RefsReceived(BMessage* message);`

Hook function that's called when the application receives a [`B_REFS_RECEIVED`](TheApplicationKit_MessageConstants.md#B_REFS_RECEIVED "B_REFS_RECEIVED") message. The message is sent when the user drops a file (or files) on your app's icon, or double clicks a file that's handled by your app. The message can arrive either at launch time, or while your application is already running use [`IsLaunching()`](BApplication.md#BApplication_IsLaunching "IsLaunching\(\)") to tell which.

`message` contains a single field named `be:refs` that contains one or more entry_ref (`B_REF_TYPE`) items one for each file that was dropped or double-clicked. Do with them what you will; the default implementation is empty. Typically, you would use the refs to create [`BEntry`](BEntry.md "BEntry") or [`BFile`](BFile.md "BFile") objects.

* * *

### Member Functions

#### InitCheck()

????

#### Archive()

See [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)")

#### DispatchMessage()

See [`BLooper::DispatchMessage()`](BLooper.md#BLooper_DispatchMessage "DispatchMessage\(\)")

#### GetAppInfo()

`status_t GetAppInfo([app_info](BRoster.md#app_info "app_info")* theInfo) const;`

Returns information about the application. This is a cover for
    
    
    be_roster->GetRunningAppInfo(be_app->Team(), theInfo);

See [`BRoster::GetAppInfo()`](BRoster.md#BRoster_GetAppInfo "GetAppInfo\(\), GetRunningAppInfo\(\), GetActiveAppInfo\(\)") for more information.

#### GetSupportedSuites()

`virtual status_t GetSupportedSuites(BMessage* message);`

Adds the scripting suite "suite/vnd.Be-application" to message. See "[Scripting Suites and Properties](BApplication.md#BApplication_ScriptingSuites "Scripting Suites and Properties")" for the suite's properties. Also see [`BHandler::GetSupportedSuites()`](BHandler.md#BHandler_GetSupportedSuites "GetSupportedSuites\(\)") for more information on how this function works.

#### IsLaunching()

`bool IsLaunching() const;`

Returns `true` if the application is still launching. An application is considered to be in its launching phase until [`ReadyToRun()`](BApplication.md#BApplication_ReadyToRun "ReadyToRun\(\)") returns. Invoked from within [`ReadyToRun()`](BApplication.md#BApplication_ReadyToRun "ReadyToRun\(\)"), `IsLaunching()` returns `true`.

#### MessageReceived()

See [`BHandler::MessageReceived()`](BHandler.md#BHandler_MessageReceived "MessageReceived\(\)")

#### ResolveSpecifier()

See [`BHandler::ResolveSpecifier()`](BHandler.md#BHandler_ResolveSpecifier "ResolveSpecifier\(\)")

####  Run(), Quit()

`virtual thread_id Run();``virtual void Quit();`

These functions, inherited from [`BLooper`](BLooper.md "BLooper"), are different enough from their parent versions to warrant description.

`Run()` doesn't spawn a new thread—it runs the message loop in the thread that it's called from, and doesn't return until the message loop stops.

`Quit()` doesn't kill the looper thread it tells the thread to finish processing the message queue (disallowing new messages) at which point `Run()` will be able to return. After so instructing the thread, `Quit()` returns, it doesn't wait for the message queue to empty.

Also, `Quit()` doesn't delete the `BApplication` object. It's up to you to delete it after `Run()` returns. (However, `Quit()` _does_ delete the object if it's called before the message loop starts i.e. before `Run()` is called.)

####  SetCursor(), HideCursor(), ShowCursor(), ObscureCursor(), IsCursorHidden()

`void SetCursor(const void* cursor);` `void SetCursor(const [`BCursor`](BCursor.md "BCursor")* cursor,  
bool sync = true);`

`void HideCursor();``void ShowCursor();``void ObscureCursor();``bool IsCursorHidden() const;`

`SetCursor()` sets the cursor image that's used when this is the active application.

You can pass one of the Be-defined cursor constants ([`B_HAND_CURSOR`](BCursor.md#B_HAND_CURSOR) and [`B_I_BEAM_CURSOR`](BCursor.md#B_I_BEAM_CURSOR)) or create your own cursor image. The cursor data format is described in the [`BCursor`](BCursor.md "BCursor") [class overview](BCursor_Overview.md "BCursor").

You can also call `SetCursor()` passing a [`BCursor`](BCursor.md "BCursor") object; specifying `sync` as `true` forces the Application Server to immediately resynchronize, thereby ensuring that the cursor change takes place immediately. The default [`BCursor`](BCursor.md "BCursor")s are [`B_CURSOR_SYSTEM_DEFAULT`](BCursor.md#B_CURSOR_SYSTEM_DEFAULT) for the hand cursor and [`B_CURSOR_I_BEAM`](BCursor.md#B_CURSOR_I_BEAM) for the I-beam text editing cursor.

`HideCursor()` removes the cursor from the screen.

`ShowCursor()` restores it.

`ObscureCursor()` hides the cursor until the user moves the mouse.

`IsCursorHidden()` returns `true` if the cursor is hidden (but not obscured), and `false` if not.

The cursor data format is described in the "[Cursor Data Format](BCursor_Overview.md#BCursor_Overview_CursorDataFormat "Cursor Data Format")" section in the [`BCursor` overview](BCursor_Overview.md "BCursor").

#### SetPulseRate()

`void SetPulseRate(bigtime_t rate);`

Sets the period between [`B_PULSE`](TheApplicationKit_MessageConstants.md#B_PULSE "B_PULSE") messages being sent and the [`Pulse()`](BApplication.md#BApplication_Pulse "Pulse\(\)") method being called. If the pulse rate is 0 (the default), the [`B_PULSE`](TheApplicationKit_MessageConstants.md#B_PULSE "B_PULSE") messages aren't sent.

####  WindowAt(), CountWindows()

`BWindow* WindowAt(int32 index) const;``int32 CountWindows() const;`

`WindowAt()` returns the `index`'th [`BWindow`](BWindow.md "BWindow") object in the application's window list. If `index` is out of range, the function returns `NULL`.

`CountWindows()` returns the number of windows in the window list.

The windows list includes all windows explicitly created by the app—whether they're normal, floating, or modal, and whether or not they're actually displayed—but excludes private windows created by Be classes.

The order of windows in the list has no signficance.

Locking the `BApplication` object doesn't lock the window list. If you need coordinated access to the list, you'll have to provide your own locking mechanism that protects these functions and all [`BWindow`](BWindow.md "BWindow") construction and deletion.

* * *

### Static Functions

#### AppResources()

`static [`BResources`](BResources.md "BResources")* AppResources();`

Returns a [`BResources`](BResources.md "BResources") object that's configured from your application's executable file. You may read the data in the [`BResources`](BResources.md "BResources") object, but you're not allowed to write it; see the [`BResources`](BResources.md "BResources") class for details. The [`BResources`](BResources.md "BResources") object belongs to the `BApplication` class and mustn't be freed.

You needn't have a `be_app` object to invoke this function.

#### Instantiate()

See [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)")

* * *

### Global Variables

#### be_app
    
    
    BApplication* be_app;

`be_app` is the global variable that represents your `BApplication` object. You can refer to `be_app` anywhere you need a reference to the `BApplication` object that you created. If you want to call a function that's declared by your `BApplication` subclass, you have to cast `be_app` to your subclass:
    
    
    ((MyApp *)be_app)->MyAppFunction();

#### be_app_messenger
    
    
    [BMessenger](BMessenger.md "BMessenger")* be_app_messenger;

`be_app_messenger` is a global [`BMessenger`](BMessenger.md "BMessenger") that targets your `be_app` object. It's created in the `BApplication` [constructor](BApplication.md#BApplication_Constructor "BApplication\(\)").

* * *

### Archived Fields

Field| Type code| Description  
---|---|---  
`mime_sig`| `B_STRING_TYPE`| Application signature.  
  
* * *

### Scripting Suites and Properties

#### Suite: "suite/vnd.Be-application"

##### "Name"

Command| Specifier| Description  
---|---|---  
`B_GET_PROPERTY`| `B_DIRECT_SPECIFIER`| Gets the name of the application's main thread.  
  
##### "Window"

Command| Specifier| Description  
---|---|---  
`B_COUNT_PROPERTIES`| `B_DIRECT_SPECIFIER`| Returns [`CountWindows()`](BApplication.md#BApplication_CountWindows).  
Not applicable.| `B_NAME_SPECIFIER`, `B_INDEX_SPECIFIER`, `B_REVERSE_INDEX_SPECIFIER`| The message is forwarded to the specified [BWindow](BWindow.md "BWindow").

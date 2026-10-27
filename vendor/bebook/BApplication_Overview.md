<!-- System Overview - The Application Kit - BApplication -->

## BApplication  
  
The [`BApplication`](BApplication.md "BApplication") class defines an object that represents your application, creates a connection to the App Server, and runs your app's main message loop. An app can only create one [`BApplication`](BApplication.md "BApplication") object; the system automatically sets the global [`be_app`](BApplication.md#BApplication_be_app "be_app") object to point to the [`BApplication`](BApplication.md "BApplication") object you create.

A [`BApplication`](BApplication.md "BApplication") object's most pervasive task is to handle messages that are sent to your app, a subject that's described in detail below. But message handling aside, you can also use your [`BApplication`](BApplication.md "BApplication") object to…

Control the cursor.
    

[`BApplication`](BApplication.md "BApplication") defines functions that hide and show the cursor, and set the cursor's image. See [`SetCursor()`](BApplication.md#BApplication_SetCursor "SetCursor\(\), HideCursor\(\), ShowCursor\(\), ObscureCursor\(\), IsCursorHidden\(\)").

Access the window list.
    

You can iterate through the windows that your application has created with [`WindowAt()`](BApplication.md#BApplication_WindowAt "WindowAt\(\), CountWindows\(\)").

Get information about your application.
    

Your app's signature, executable location, and launch flags can be retrieved through [`GetAppInfo()`](BApplication.md#BApplication_GetAppInfo "GetAppInfo\(\)"). Additional information icons, version strings, recognized file types can be retrieved by creating an [`BAppFileInfo`](BAppFileInfo.md "BAppFileInfo") object based on your app's executable file. [`BAppFileInfo`](BAppFileInfo.md "BAppFileInfo") is defined in the [The Storage Kit](TheStorageKit.md "The Storage Kit").

* * *

### be_app and Subclassing BApplication

Because of its importance, the [`BApplication`](BApplication.md "BApplication") object that you create is automatically assigned to the global [`be_app`](BApplication.md#BApplication_be_app "be_app") variable. Anytime you need to refer to your [`BApplication`](BApplication.md "BApplication") object from anywhere in your code you can use [`be_app`](BApplication.md#BApplication_be_app "be_app") instead.

Unless you're creating a very simple application, you should subclass [`BApplication`](BApplication.md "BApplication"). But be aware that the [`be_app`](BApplication.md#BApplication_be_app "be_app") variable is typed as ([`BApplication`](BApplication.md "BApplication") *). You'll have to cast [`be_app`](BApplication.md#BApplication_be_app "be_app") when you call a function that's declared by your subclass:
    
    
    ((MyApp *)be_app)->MyAppFunction();

* * *

### Constructing the Object and Running the Message Loop

As with all [`BLooper`](BLooper.md "BLooper")s, to use a [`BApplication`](BApplication.md "BApplication") you construct the object and then tell it to start its message loop by calling the [`Run()`](BApplication.md#BApplication_Run "Run\(\), Quit\(\)") function. However, unlike other loopers, [`BApplication`](BApplication.md "BApplication")'s [`Run()`](BApplication.md#BApplication_Run "Run\(\), Quit\(\)") doesn't return until the application is told to quit. And after [`Run()`](BApplication.md#BApplication_Run "Run\(\), Quit\(\)") returns, you delete the object it isn't deleted for you.

Typically, you create your [`BApplication`](BApplication.md "BApplication") object in your `main()` function—it's usually the first object you create. The barest outline of a typical `main()` function looks something like this:
    
    
    #include Application.h
    
    main()![1](./images/callouts/1.png)
    {
       ![2](./images/callouts/2.png)new BApplication("application/x-vnd.your-app-sig")![3](./images/callouts/3.png);
       /* Further initialization goes here -- read settings,
          set globals, etc. */
       be_app->Run()![4](./images/callouts/4.png);
       /* Clean up -- write settings, etc. */
       delete be_app;
    }

![1](./images/callouts/1.png) | The `main()` function doesn't declare `argc` and `argv` parameters (used for passing along command line arguments). If the user passes command line arguments to your app, they'll show up in the [`ArgvReceived()`](BApplication.md#BApplication_ArgvReceived "ArgvReceived\(\)") hook function.  
---|---  
![2](./images/callouts/2.png) | Why no pointer assignment? The constructor automatically assigns the object to [`be_app`](BApplication.md#BApplication_be_app "be_app"), so you don't have to assign it yourself.  
![3](./images/callouts/3.png) | The string passed to the constructor sets the application's signature. This is a precautionary measure it's better to add the signature as a resource than to define it here (a resource signature overrides the constructor signature). Use the FileTypes app to set the signature as a resource.  
![4](./images/callouts/4.png) | As explained in the [`BLooper`](BLooper.md "BLooper") class, [`Run()`](BApplication.md#BApplication_Run "Run\(\), Quit\(\)") is almost always called from the same thread in which you construct the [`BApplication`](BApplication.md "BApplication") object. (More accurately, the constructor locks the object, and [`Run()`](BApplication.md#BApplication_Run "Run\(\), Quit\(\)") unlocks it. Since locks are scoped to threads, the easiest thing to do is to construct and [`Run()`](BApplication.md#BApplication_Run "Run\(\), Quit\(\)") in the same thread.)  
  
* * *

### Application Messages

After you tell your [`BApplication`](BApplication.md "BApplication") to run, its message loop begins to receive messages. In general, the messages are handled in the expected fashion: They show up in [`BApplication`](BApplication.md "BApplication")'s [`MessageReceived()`](BApplication.md#BApplication_MessageReceived "MessageReceived\(\)") function (or that of a designated [`BHandler`](BHandler.md "BHandler"); for more on how messages are dispatched to handlers, see ("[From Looper to Handler](TheApplicationKit_Messaging.md#TheApplicationKit_Messaging_FromLooperToHandler "From Looper to Handler")").

But [`BApplication`](BApplication.md "BApplication") also recognizes a set of application messages that it handles by invoking corresponding hook functions. The hook functions are invoked by [`DispatchMessage()`](BApplication.md#BApplication_DispatchMessage "DispatchMessage\(\)") so the application messages never show up in [`MessageReceived()`](BApplication.md#BApplication_MessageReceived "MessageReceived\(\)").

Overriding the hook functions that correspond to the application messages is an important part of the implementation of a [`BApplication`](BApplication.md "BApplication") subclass.

In the table below, the application messages (identified by their command constants) are listed in roughly the order your [`BApplication`](BApplication.md "BApplication") can expect to receive them.

Command Constant| Hook function| Description  
---|---|---  
[`B_ARGV_RECEIVED`](TheApplicationKit_MessageConstants.md#B_ARGV_RECEIVED "B_ARGV_RECEIVED")| [`ArgvReceived()`](BApplication.md#BApplication_ArgvReceived "ArgvReceived\(\)")| Command line arguments are delivered through this message.  
[`B_REFS_RECEIVED`](TheApplicationKit_MessageConstants.md#B_REFS_RECEIVED "B_REFS_RECEIVED")| [`RefsReceived()`](BApplication.md#BApplication_RefsReceived "RefsReceived\(\)")| Files (entry_refs) that are dropped on your app's icon, or that are double-clicked to launch your app are delivered through this message.  
[`B_READY_TO_RUN`](TheApplicationKit_MessageConstants.md#B_READY_TO_RUN "B_READY_TO_RUN")| [`ReadyToRun()`](BApplication.md#BApplication_ReadyToRun "ReadyToRun\(\)")| Invoked from within [`Run()`](BApplication.md#BApplication_Run "Run\(\), Quit\(\)"), the application has finished configuring itself and is ready to go. If you haven't already created and displayed an initial window, you should do so here.  
[`B_APP_ACTIVATED`](TheApplicationKit_MessageConstants.md#B_APP_ACTIVATED "B_APP_ACTIVATED")| [`AppActivated()`](BApplication.md#BApplication_AppActivated "AppActivated\(\)")| The application has just become the active application, or has relinquished that status.  
[`B_PULSE`](TheApplicationKit_MessageConstants.md#B_PULSE "B_PULSE")| [`Pulse()`](BApplication.md#BApplication_Pulse "Pulse\(\)")| If requested, pulse messages are sent at regular intervals by the system.  
[`B_ABOUT_REQUESTED`](TheApplicationKit_MessageConstants.md#B_ABOUT_REQUESTED "B_ABOUT_REQUESTED")| [`AboutRequested()`](BApplication.md#BApplication_AboutRequested "AboutRequested\(\)")| The user wants to see the app's About… box.  
  
The protocols for the application messages are described in the "[Message Constants](TheApplicationKit_MessageConstants.md "Message Constants")" section of this chapter.

For more information on the details of when and why the hook functions are invoked, see the individual function descriptions.

A [`BApplication`](BApplication.md "BApplication") can also receive the [`B_QUIT_REQUESTED`](TheApplicationKit_MessageConstants.md#B_QUIT_REQUESTED "B_QUIT_REQUESTED") looper message. As explained in [`BLooper`](BLooper.md "BLooper"), [`B_QUIT_REQUESTED`](TheApplicationKit_MessageConstants.md#B_QUIT_REQUESTED "B_QUIT_REQUESTED") causes `Quit()` to be called, contingent on the value returned by the `QuitRequested`() hook function. However, [`BApplication`](BApplication.md "BApplication")'s implementation of [`Quit()`](BApplication.md#BApplication_Quit) is different from [`BLooper`](BLooper.md "BLooper")'s version. Don't miss it.

* * *

### Other Topics

Locking.
    

Like a [`BLooper`](BLooper.md "BLooper"), a [`BApplication`](BApplication.md "BApplication") must be locked before calling certain protected functions. The [`BApplication`](BApplication.md "BApplication") locking mechanism is inherited without modification from [`BLooper`](BLooper.md "BLooper").

FileTypes settings.
    

The [`BApplication`](BApplication.md "BApplication") object represents your application at run-time. However, some of the characteristics of your application, whether it can be launched more than once, the file types it can open, its icon are not run-time decisions.

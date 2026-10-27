<!-- Classes And Methods - The Application Kit - BRoster -->

## BRoster

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `app/Roster.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BRoster_Overview.md "BRoster")

  * [Constructor and Destructor](BRoster.md#BRoster_ConstructorDestructor)
  * [Member Functions](BRoster.md#BRoster_MemberFunctions)
  * [Global Variables](BRoster.md#BRoster_GlobalVariables)
  * [Constants](BRoster.md#BRoster_Constants)
  * [Defined Types](BRoster.md#BRoster_DefinedTypes)

  
  
* * *

### Constructor and Destructor

#### BRoster()

`BRoster();`

Sets up the object's connection to the roster service.

When an application constructs its [`BApplication`](BApplication.md "BApplication") object, the system constructs a `BRoster` object and assigns it to the [`be_roster`](BRoster.md#BRoster_be_roster "be_roster") global variable. A `BRoster` is therefore readily available from the time the application is initialized until the time it quits; you don't have to construct one. The constructor is public only to give programs that don't have [`BApplication`](BApplication.md "BApplication") objects access to the roster.

#### ~BRoster()

`~BRoster();`

Does nothing.

* * *

### Member Functions

#### ActivateApp()

`status_t ActivateApp(team_id team) const;`

Activates the `team` application (by bringing one of its windows to the front and making it the active window). This function works only if the target application has a window on-screen. The newly activated application is notified with a `B_APP_ACTIVATED` message.

See also: [`BApplication::AppActivated()`](BApplication.md#BApplication_AppActivated "AppActivated\(\)")

####  AddToRecentDocuments(), GetRecentDocuments()

`void AddToRecentDocuments(const entry_ref* document,  
const char* appSig = NULL) const;`

`void GetRecentDocuments([`BMessage`](BMessage.md "BMessage")* refList,  
int32 maxCount,  
const char* ofType = NULL,  
const char* openedByAppSig = NULL) const;` `void GetRecentDocuments([`BMessage`](BMessage.md "BMessage")* refList,  
int32 maxCount,  
const char* ofTypeList[] = NULL,  
int32 ofTypeListCount,  
const char* openedByAppSig = NULL) const;`

`AddToRecentDocuments()` adds the document file specified by `document` to the list of recent documents. If you wish to record that a specific application used the document, you can specify the signature of that application using the `appSig` argument; otherwise you can specify `NULL`.

`GetRecentDocuments()` returns a list of the most recent documents. The [`BMessage`](BMessage.md "BMessage") `refList` will be filled out with information about the `maxCount` most recently used documents. If you want to obtain a list of documents of a specific type, you can specify a pointer to that MIME type string in the `ofType` argument. Likewise, if you're only interested in files that want to be opened by a specific application, specify that application's signature in `openedByAppSig`; if you don't care, pass `NULL`.

If you want to get a list of files of multiple types, you can specify a pointer to an array of strings in `ofTypeList`, and the number of types in the list in `ofTypeListCount`.

Specifying `NULL` for `ofType` will fetch all files of all types.

The resulting `refList` will have a field, `refs`, containing the entry_refs to the resulting list of files.

####  AddToRecentFolders(), GetRecentFolders()

`void AddToRecentFolders(const entry_ref* folder,  
const char* appSig = NULL) const;``void GetRecentFolders([`BMessage`](BMessage.md "BMessage")* refList,  
int32 maxCount,  
const char* openedByAppSig = NULL) const;`

`AddToRecentFolders()` adds the folder specified by `folder` to the list of recent folders. If you wish to record that a specific application used the folder, you can specify the signature of that application using the `appSig` argument; otherwise you can use `NULL`.

`GetRecentFolders()` returns a list of the most recently-accessed folders. The [`BMessage`](BMessage.md "BMessage") `refList` will be filled out with information about the `maxCount` most recently used folders. If you're only interested in folders that were used by a specific application, specify that application's signature in `openedByAppSig`; if you don't care, pass `NULL`.

The resulting `refList` will have a field, `refs`, containing the entry_refs to the resulting list of folders.

#### Broadcast()

`status_t Broadcast([`BMessage`](BMessage.md "BMessage")* message) const;` `status_t Broadcast([`BMessage`](BMessage.md "BMessage")* message,  
[`BMessenger`](BMessenger.md "BMessenger") reply_to) const;`

Sends the `message` to every running application, except to those applications (`B_ARGV_ONLY`) that don't accept messages. The `message` is sent asynchronously with a timeout of 0. As is the case for other message-sending functions, the caller retains ownership of the message.

This function returns immediately after setting up the broadcast operation. It doesn't wait for the messages to be sent and doesn't report any errors encountered when they are. It returns an error only if it can't start the broadcast operation. If successful in getting the operation started, it returns `B_OK`.

Replies to the broadcasted `message` will be sent via the `reply_to` [`BMessenger`](BMessenger.md "BMessenger"), if specified. If `reply_to` is absent, the replies will be lost.

See also: [`BMessenger::SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)")

#### FindApp()

`status_t FindApp(const char* type,  
entry_ref* app) const;` `status_t FindApp(entry_ref* file,  
entry_ref* app) const;`

Finds the application associated with the MIME data `type` or with the specified `file`, and modifies the `app` entry_ref structure so that it refers to the executable file for that application. If the `type` is an application signature, this function finds the application that has that signature. Otherwise, it finds the preferred application for the `type`. If the `file` is an application executable, `FindApp()` merely copies the file reference to the `app` argument. Otherwise, it finds the preferred application for the filetype.

In other words, this function goes about finding an application in the same way that [`Launch()`](BRoster.md#BRoster_Launch "Launch\(\)") finds the application it will launch.

If it can translate the `type` or `file` into a reference to an application executable, `FindApp()` returns `B_OK`. If not, it returns an error code, typically one describing a file system error.

See also: [`Launch()`](BRoster.md#BRoster_Launch "Launch\(\)")

####  GetAppInfo(), GetRunningAppInfo(), GetActiveAppInfo()

`status_t GetAppInfo(const char* signature,  
[app_info](BRoster.md#app_info "app_info")* appInfo) const;` `status_t GetAppInfo(entry_ref* executable,  
[app_info](BRoster.md#app_info "app_info")* appInfo) const;`

`status_t GetRunningAppInfo(team_id team,  
[app_info](BRoster.md#app_info "app_info")* appInfo) const;``status_t GetActiveAppInfo([app_info](BRoster.md#app_info "app_info")* appInfo) const;`

These functions return (in `appInfo`) information about a specific application. In all cases, the application must be running.

`GetAppInfo()` finds an app that has the given `signature`, or that was launched from the `executable` file. If there's more than one such app, the function chooses one at random.

`GetRunningAppInfo()` reports on the app that corresponds to the given `team` identifier.

`GetActiveAppInfo()` reports on the currently active app.

If they're able to fill in the [app_info](BRoster.md#app_info "app_info") structure with meaningful values, these functions return `B_OK`. `GetActiveAppInfo()` returns `B_ERROR` if there's no active application. `GetRunningAppInfo()` returns `B_BAD_TEAM_ID` if `team` isn't a valid team identifier for a running application. `GetAppInfo()` returns `B_ERROR` if the application isn't running.

The app_info structure contains the following fields:

Field| Description  
---|---  
thread_id `thread`| The identifier for the application's main thread of execution, or -1 if the application isn't running. (The main thread is the thread in which the application is launched and in which its `main()` function runs.)  
thread_id `team`| The identifier for the application's team, or -1 if the application isn't running. (This will be the same as the team passed to `GetRunningAppInfo()`.)  
port_id `port`| The port where the application's main thread receives messages, or -1 if the application isn't running.  
uint32 `flags`| A bitfield that contains information about the behavior of the application.The `flags` bitfield can be tested (with the bitwise & operator) against these two constants:

`B_BACKGROUND_APP`.
    The application won't appear in the Deskbar's application list.
`B_ARGV_ONLY`.
    The application can't receive messages. Information can be passed to it at launch only, in an array of argument strings (as on the command line).
`flags` also contains a value that explains the application's launch behavior. This value is retrieved by masking flags with the `B_LAUNCH_MASK` constant. For example:
    
    
    unit32 behavior = theInfo.flags & B_LAUNCH_MASK;

The result will match one of these three modes:

`B_EXCLUSIVE_LAUNCH`.
    The application can be launched only if an application with the same signature isn't already running.
`B_SINGLE_LAUNCH`.
    The application can be launched only once from the same executable file. However, an application with the same signature might be launched from a different executable. For example, if the user copies an executable file to another directory, a separate instance of the application can be launched from each copy.
`B_MULTIPLE_LAUNCH`.
    There are no restrictions. The application can be launched any number of times from the same executable file.
These modes affect `BRoster`'s `Launch()` function. `Launch()` can always start up a `B_MULTIPLE_LAUNCH` application. However, it can't launch a `B_SINGLE_LAUNCH` application if a running application was already launched from the same executable file. It can't launch a `B_EXCLUSIVE_LAUNCH` application if an application with the same signature is already running.  
entry_ref `ref`| A reference to the file that was, or could be, executed to run the application. (This will be the same as the executable passed to `GetAppInfo()`.)  
char[] `signature`| The signature of the application. (This will be the same as the signature passed to `GetAppInfo()`.)  
  
See also: [`Launch()`](BRoster.md#BRoster_Launch "Launch\(\)"), [`BApplication::GetAppInfo()`](BApplication.md#BApplication_GetAppInfo "GetAppInfo\(\)")

#### GetAppList()

`void GetAppList([`BList`](BList.md "BList")* teams) const;` `void GetAppList(const char* signature,  
[`BList`](BList.md "BList")* teams) const;`

Fills in the `teams` [`BList`](BList.md "BList") with team identifiers for applications in the roster. Each item in the list will be of type team_id. It must be cast to that type when retrieving it from the list, as follows:
    
    
    [BList](BList.md "BList")* teams = new BList;
    be_roster->GetAppList(teams);
    team_id who = (team_id)teams->ItemAt(someIndex);

The list will contain one item for each instance of an application that's running. For example, if the same application has been launched three times, the list will include the team_ids for all three running instances of that application.

If a `signature` is passed, the list identifies only applications running under that signature. If a `signature` isn't specified, the list identifies all running applications.

See also: [`TeamFor()`](BRoster.md#BRoster_TeamFor "TeamFor\(\) , IsRunning\(\)"), the [BMessenger constructor](BMessenger.md#BMessenger_Constructor "BMessenger\(\)")

#### GetRecentApps()

`void GetRecentApps([`BMessage`](BMessage.md "BMessage")* refList,  
int32 maxCount) const;`

`GetRecentApps()` returns a list of the most recently-launched applications. The [`BMessage`](BMessage.md "BMessage") `refList` will be filled out with information about the `maxCount` most recently-launched applications.

The resulting `refList` will have a field, "refs", containing the entry_refs to the resulting applications.

#### Launch()

`status_t Launch(const char* type,  
[`BMessage`](BMessage.md "BMessage")* message = NULL,  
team_id* team = NULL) const;` `status_t Launch(const char* type,  
[`BList`](BList.md "BList")* messages,  
team_id* team = NULL) const;` `status_t Launch(const char* type,  
int argc,  
char** argv,  
team_id* team = NULL) const;` `status_t Launch(const entry_ref* file,  
const [`BMessage`](BMessage.md "BMessage")* message = NULL,  
team_id* team = NULL) const;` `status_t Launch(const entry_ref* file,  
const [`BList`](BList.md "BList")* messages,  
team_id* team = NULL) const;` `status_t Launch(const entry_ref* file,  
int argc,  
char** argv,  
team_id* team = NULL) const;`

Launches the application associated with a MIME `type` or with a particular `file`. If the MIME `type` is an application signature, this function launches the application with that signature. Otherwise, it launches the preferred application for the type. If the `file` is an application executable, it launches that application. Otherwise, it launches the preferred application for the file type and passes the `file` reference to the application in a `B_REFS_RECEIVED` message. In other words, `Launch()` finds the application to launch just as [`FindApp()`](BRoster.md#BRoster_FindApp "FindApp\(\)") finds the application for a particular `type` or `file`.

If a `message` is specified, it will be sent to the application on-launch where it will be received and responded to before the application is notified that it's ready to run. Similarly, if a list of `messages` is specified, each one will be delivered on-launch. The caller retains ownership of the [`BMessage`](BMessage.md "BMessage") objects (and the container [`BList`](BList.md "BList")); they won't be deleted for you.

Sending an on-launch message is appropriate if it helps the launched application configure itself before it starts getting other messages. To launch an application and send it an ordinary message, call `Launch()` to get it running, then set up a [`BMessenger`](BMessenger.md "BMessenger") object for the application and call [`BMessenger`](BMessenger.md "BMessenger")'s [`SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)") function.

If the target application is already running, `Launch()` won't launch it again, unless it permits multiple instances to run concurrently (it doesn't wait for the messages to be sent or report errors encountered when they are). It fails for `B_SINGLE_LAUNCH` and `B_EXCLUSIVE_LAUNCH` applications that have already been launched. Nevertheless, it assumes that you want the messages to get to the application and so delivers them to the currently running instance.

Instead of messages, you can launch an application with an array of argument strings that will be passed to its `main()` function. `argv` contains the array and `argc` counts the number of strings. If the application accepts messages, this information will also be packaged in a `B_ARGV_RECEIVED` message that the application will receive on-launch.

If successful, `Launch()` places the identifier for the newly launched application in the variable referred to by `team` and returns `B_OK`. If unsuccessful, it sets the `team` variable to -1 and returns an error code, typically one of the following:

Return Code| Description  
---|---  
`B_BAD_VALUE`.| The `type` or `file` is not valid, or an attempt is being made to send an on-launch message to an application that doesn't accept messages (that is, to a `B_ARGV_ONLY` application).  
`B_ALREADY_RUNNING`.| The application is already running and can't be launched again (it's a `B_SINGLE_LAUNCH` or `B_EXCLUSIVE_LAUNCH` application).  
`B_LAUNCH_FAILED`.| The attempt to launch the application failed for some other reason, such as insufficient memory.  
A file system error.| The `file` or `type` can't be matched to an application.  
  
See also: the [`BMessenger`](BMessenger.md "BMessenger") class, [`GetAppInfo()`](BRoster.md#BRoster_GetAppInfo "GetAppInfo\(\), GetRunningAppInfo\(\), GetActiveAppInfo\(\)"), [`FindApp()`](BRoster.md#BRoster_FindApp "FindApp\(\)")

####  StartWatching() , StopWatching()

`status_t StartWatching([`BMessenger`](BMessenger.md "BMessenger") target,  
uint32 events = B_REQUEST_LAUNCHED | B_REQUEST_QUIT) const;``status_t StopWatching([`BMessenger`](BMessenger.md "BMessenger") target) const;`

`StartWatching()` initiates the application event monitor, which is used for keeping track of events such as application launches. The caller specifies the events to monitor through the `events` argument; `target` is the [`BMessenger`](BMessenger.md "BMessenger") to which the corresponding notification messages are sent. The `events` flags and the corresponding messages are listed below:

Flag| Message  
---|---  
`B_REQUEST_LAUNCHED`| `B_SOME_APP_LAUNCHED`  
`B_REQUEST_QUIT`| `B_SOME_APP_QUIT`  
`B_REQUEST_ACTIVATED`| `B_SOME_APP_ACTIVATED`  
  
The fields in a notification message describe the application that was launched, quit, or activated:

Field| Type| Description  
---|---|---  
`mime_sig`| `B_STRING_TYPE`| MIME signature  
`team`| `B_INT32_TYPE`| team_id  
`thread`| `B_INT32_TYPE`| thread_id  
`flags`| `B_INT32_TYPE`| application flags  
`ref`| `B_REF_TYPE`| executable's entry_ref  
  
`StopWatching()` terminates the application monitor previously initiated for a given [`BMessenger`](BMessenger.md "BMessenger").

####  TeamFor() , IsRunning()

`team_id TeamFor(const char* signature) const;` `team_id TeamFor(entry_ref* executable) const;`

`bool IsRunning(const char* signature) const;` `bool IsRunning(entry_ref* executable) const;`

Both these functions query whether the application identified by its `signature` or by a reference to its `executable` file is running. `TeamFor()` returns its team identifier if it is, and `B_ERROR` if it's not. `IsRunning()` returns `true` if it is, and `false` if it's not.

If the application is running, you probably will want its team identifier (to set up a [`BMessenger`](BMessenger.md "BMessenger") , for example). Therefore, it's most economical to simply call `TeamFor()` and forego `IsRunning()`.

If more than one instance of the `signature` application is running, or if more than one instance was launched from the same `executable` file, `TeamFor()` arbitrarily picks one of the instances and returns its team_id.

See also: [`GetAppList()`](BRoster.md#BRoster_GetAppList "GetAppList\(\)")

* * *

### Global Variables

#### be_roster
    
    
    BRoster* be_roster

This variable points to the application's global `BRoster` object. The `BRoster` keeps a roster of all running applications and can add applications to the roster by launching them. It's initialized when the application starts up.

* * *

### Constants

####  Application Flags
    
    
    B_BACKGROUND_APP
    B_ARGV_ONLY
    B_LAUNCH_MASK

These constants are used to get information from the flags field of an [app_info](BRoster.md#app_info "app_info") structure.

See also: [`BRoster::GetAppInfo()`](BRoster.md#BRoster_GetAppInfo "GetAppInfo\(\), GetRunningAppInfo\(\), GetActiveAppInfo\(\)"), "[Launch Constants](BRoster.md#Constants_Launch "Launch Constants")" below

####  Launch Constants
    
    
    B_MULTIPLE_LAUNCH
    B_SINGLE_LAUNCH
    B_EXCLUSIVE_LAUNCH

These constants explain whether an application can be launched any number of times, only once from a particular executable file, or only once for a particular application signature. This information is part of the flags field of an [app_info](BRoster.md#app_info "app_info") structure and can be extracted using the `B_LAUNCH_MASK` constant.

See also: [`BRoster::GetAppInfo()`](BRoster.md#BRoster_GetAppInfo "GetAppInfo\(\), GetRunningAppInfo\(\), GetActiveAppInfo\(\)"), "[Application Flags](BRoster.md#Constants_Application_Flags "Application Flags")" above

* * *

### Defined Types

#### app_info
    
    
    typedef struct {
        thread_id thread;
        team_id   team;
        port_id   port;
        uint32    flags;
        entry_ref ref;
        char      signature[B_MIME_TYPE_LENGTH];
    
        app_info(void);
        ~app_info(void);
    } app_info

This structure is used by [`BRoster`](BRoster.md "BRoster")'s [`GetAppInfo()`](BRoster.md#BRoster_GetAppInfo "GetAppInfo\(\), GetRunningAppInfo\(\), GetActiveAppInfo\(\)"), [`GetRunningAppInfo()`](BRoster.md#BRoster_GetRunningAppInfo), and [`GetActiveAppInfo()`](BRoster.md#BRoster_GetActiveAppInfo) functions to report information about an application. Its constructor ensures that its fields are initialized to invalid values. To get meaningful values for an actual application, you must pass the structure to one of the [`BRoster`](BRoster.md "BRoster") functions. See those functions for a description of the various fields.

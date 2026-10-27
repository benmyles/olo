<!-- Classes And Methods - The Kernel Kit - Threads And Teams -->

## Threads And Teams

Declared in:| `kernel/OS.h`  
---|---  
Library:| `libroot.so`  
  
A thread is a synchronous process that executes a series of program instructions. When you launch an application, an initial thread—the main thread—is automatically created (or spawned) and told to run. From the main thread you can spawn and run additional threads; from each of these threads you can spawn and run more threads, and so on. The collection of threads that are spawned from the main thread—in other words, the threads that comprise an application—is called a team. All the threads in all teams run concurrently and asynchronously with each other.

For more information on threads and teams, see "[Threads And Teams Overview](TheKernelKit_ThreadsAndTeams_Overview.md "Threads And Teams")".

* * *

### Thread and Team Functions

#### estimate_max_scheduling_latency()

Declared in: `kernel/scheduler.h`

`bigtime_t estimate_max_scheduling_latency(thread_id thread = -1);`

Returns the scheduling latency, in microseconds, of the specified thread. Specify a thread_id of -1 to return the scheduling latency of the current thread.

####  exit_thread(), kill_thread(), kill_team(), on_exit_thread()

`void exit_thread(status_t return_value);``status_t kill_thread(thread_id thread);``status_t kill_team(team_id team);`
    
    
    status_t on_exit_thread(void (*callback)(void *), void *data)

These functions command one or more threads to halt execution:

Function| Description  
---|---  
`exit_thread()`| Tells the calling thread to exit with a return value as given by the argument. Declaring the return value is only useful if some other thread is sitting in a [`wait_for_thread()`](TheKernelKit_ThreadsAndTeams.md#wait_for_thread "wait_for_thread\(\)") call on this thread. `exit_thread()` sends a signal to the thread (after caching the return value in a known place).  
`kill_thread()`| Kills the thread given by the argument. The value that the thread will return to [`wait_for_thread()`](TheKernelKit_ThreadsAndTeams.md#wait_for_thread "wait_for_thread\(\)") is undefined and can't be relied upon. `kill_thread()` is the same as sending a SIGKILLTHR signal to the thread.  
`kill_team()`| Kills all the threads within the given team. Again, the threads' return values are random. `kill_team()` is the same as sending a SIGKILL signal to any thread in the team. Each of the threads in the team is then handed a SIGKILLTHR signal.  
`on_exit_thread()`| Sets up the specified callback to be executed when the calling thread exits. The callback will receive the pointer data as an input argument.  
  
Exiting a thread is a fairly safe thing to do—since a thread can only exit itself, it's assumed that the thread knows what it's doing. Killing some other thread or an entire team is a bit more drastic since the death certificate(s) will be delivered at an indeterminate time. In addition, killing a thread can leak memory since resources that were allocated by the thread may not be freed. Killing an entire team, on the other hand, won't leak since the system reclaims all resources when the team dies.

Keep in mind that threads die automatically (and their resources are reclaimed) if they're allowed to exit naturally. You should only need to kill a thread if something has gone screwy.

Return Code| Description  
---|---  
`B_OK`.| The thread or team was successfully killed.  
`B_BAD_THREAD_ID`.| Invalid thread value.  
`B_BAD_TEAM_ID`.| Invalid team value.  
`B_NO_MEMORY`.| Returned by `on_exit_thread()` if there's no memory to construct the internal callback record.  
  
#### find_thread()

`thread_id find_thread(const char* name);`

Finds and returns the thread with the given `name`. A `name` argument of `NULL` returns the calling thread.

A thread's name is assigned when the thread is spawned. The name can be changed thereafter through the [`rename_thread()`](TheKernelKit_ThreadsAndTeams.md#rename_thread "rename_thread\(\)") function. Keep in mind that thread names needn't be unique: If two (or more) threads boast the same name, a `find_thread()` call on that name returns the first so-named thread that it finds. There's no way to iterate through identically-named threads.

Return Code| Description  
---|---  
`B_NAME_NOT_FOUND`.| `name` doesn't identify a valid thread.  
  
####  get_team_info(), get_next_team_info()

`status_t get_team_info(team_id team,  
team_info* info);``status_t get_next_team_info(int32* cookie,  
team_info* info);`

The functions copy, into the `info` argument, the [team_info](TheKernelKit_ThreadsAndTeams.md#team_info "team_info") structure for a particular team. The `get_team_info()` function retrieves information for the team identified by `team`. For information about the kernel, use `B_SYSTEM_TEAM` as the team argument.

The `get_next_team_info()` version lets you step through the list of all teams. The `cookie` argument is a placemark; you set it to 0 on your first call, and let the function do the rest. The function returns `B_BAD_VALUE` when there are no more areas to visit:
    
    
    /* Get the team_info for every team. */
    team_info info;
    int32 cookie = 0;
    
    while (get_next_team_info(0, &cookie, &info) == B_OK)
       ...

See [team_info](TheKernelKit_ThreadsAndTeams.md#team_info "team_info") for a description of that structure.

Return Code| Description  
---|---  
`B_OK`.| The desired team information was found.  
`B_BAD_TEAM_ID`.| `team` doesn't identify an existing team, or there are no more areas to visit.  
  
####  get_thread_info(), get_next_thread_info()

`status_t get_thread_info(thread_id thread,  
thread_info* info);``status_t get_next_thread_info(team_id team,  
int32* cookie,  
thread_info* info);`

These functions copy, into the `info` argument, the [thread_info](TheKernelKit_ThreadsAndTeams.md#thread_info "thread_info") structure for a particular thread:

The `get_thread_info()` function gets the information for the thread identified by `thread`.

The `get_next_thread_info()` function lets you step through the list of a team's threads through iterated calls. The `team` argument identifies the team you want to look at; a `team` value of 0 means the team of the calling thread. The `cookie` argument is a placemark; you set it to 0 on your first call, and let the function do the rest. The function returns `B_BAD_VALUE` when there are no more threads to visit:
    
    
    /* Get the thread_info for every thread in this team. */
    thread_info info;
    int32 cookie = 0;
    
    while (get_next_thread_info(0, &cookie, &info) == B_OK)
       ...

The value of the `priority` field describes the thread's "urgency"; the higher the value, the more urgent the thread. The more urgent the thread, the more attention it gets from the CPU. Expected priority values fall between 0 and 120. See "[Thread Priorities](TheKernelKit_ThreadsAndTeams.md#Constants_ThreadPriorities "Thread Priority Values")" for the full story.

Warning

![Warning](./images/admonitions/Stop_32.png)

Thread info is provided primarily as a debugging aid. None of the values that you find in a thread_info structure are guaranteed to be valid—the thread's state, for example, will almost certainly have changed by the time `get_thread_info()` returns. 

Return Code| Description  
---|---  
`B_OK`.| The thread was found; `info` contains valid information.  
`B_BAD_VALUE`.| `thread` doesn't identify an existing thread, `team` doesn't identify an existing team, or there are no more threads to visit.  
  
#### rename_thread()

`status_t rename_thread(thread_id thread,  
const char* name);`

Changes the name of the given thread to `name`. The name can be no longer than `B_OS_NAME_LENGTH` (32 characters).

Return Code| Description  
---|---  
`B_OK`.| The thread was successfully named.  
`B_BAD_THREAD_ID`.| `thread` argument isn't a valid thread_id number.  
  
#### resume_thread()

`status_t resume_thread(thread_id thread);`

Tells a new or suspended thread to begin executing instructions. If the thread has just been spawned, it enters and executes the thread function declared in [`spawn_thread()`](TheKernelKit_ThreadsAndTeams.md#spawn_thread "spawn_thread\(\)"). If the thread was previously suspended(through [`suspend_thread()`](TheKernelKit_ThreadsAndTeams.md#suspend_thread "suspend_thread\(\)")), it continues from where it was suspended.

You can't use this function to wake up a sleeping thread, or to unblock a thread that's waiting to acquire a semaphore or waiting in a [`receive_data()`](TheKernelKit_ThreadsAndTeams.md#receive_data "receive_data\(\)") call. However, you can unblock any of these threads by suspending and then resuming. Blocked threads that are resumed return `B_INTERRUPTED`.

`resume_thread()` is the same as sending a SIGCONT signal to the thread.

Return Code| Description  
---|---  
`B_OK`.| The thread was successfully resumed.  
`B_BAD_THREAD_ID`.| `thread` argument isn't a valid thread_id number.  
`B_BAD_THREAD_STATE`.| The thread isn't suspended.  
  
#### receive_data()

`int32 receive_data(thread_id sender,  
void* buffer,  
size_t buffer_size);`

Retrieves a message from the thread's message cache. The message will have been placed there through a previous [`send_data()`](TheKernelKit_ThreadsAndTeams.md#send_data "send_data\(\)") function call. If the cache is empty, `receive_data()` blocks until one shows up—it never returns empty-handed.

The thread_id of the thread that called [`send_data()`](TheKernelKit_ThreadsAndTeams.md#send_data "send_data\(\)") is returned by reference in the sender argument. Note that there's no guarantee that the sender will still be alive by the time you get its ID. Also, the value of sender going into the function is ignored—you can't ask for a message from a particular sender.

The [`send_data()`](TheKernelKit_ThreadsAndTeams.md#send_data "send_data\(\)") function copies two pieces of data into a thread's message cache:

  * A single four-byte code that's delivered as `receive_data()`'s return value,

  * and an arbitrarily long data buffer that's copied into `receive_data()`'s buffer argument (you must allocate and free buffer yourself). The `buffer_size` argument tells the function how many bytes of data to copy. If you don't need the data buffer—if the code value returned directly by the function is sufficient—you set `buffer` to `NULL` and `buffer_size` to 0.

Unfortunately, there's no way to tell how much data is in the cache before you call `receive_data()`:

  * If there's more data than buffer can accommodate, the unaccommodated portion is discarded—a second `receive_data()` call will not read the rest of the message.

  * Conversely, if `receive_data()` asks for more data than was sent, the function returns with the excess portion of buffer unmodified—`receive_data()` doesn't wait for another [`send_data()`](TheKernelKit_ThreadsAndTeams.md#send_data "send_data\(\)") call to provide more data with which to fill up the buffer.

Each receive_data() corresponds to exactly one [`send_data()`](TheKernelKit_ThreadsAndTeams.md#send_data "send_data\(\)"). Lacking a previous invocation of its mate, `receive_data()` will block until [`send_data()`](TheKernelKit_ThreadsAndTeams.md#send_data "send_data\(\)") is called. If you don't want to block, you should call [`has_data()`](TheKernelKit_ThreadsAndTeams.md#has_data "has_data\(\)") before calling `receive_data()` (and proceed to `receive_data()` only if [`has_data()`](TheKernelKit_ThreadsAndTeams.md#has_data "has_data\(\)") returns `true`).

Return Code| Description  
---|---  
If successful| Returns the message's four-byte code.  
`B_INTERRUPTED`.| A blocked `receive_data()` call was interrupted by a signal.  
  
#### send_data()

`status_t send_data(thread_id sender,  
void* buffer,  
size_t buffer_size);`

`send_data()` copies a message into thread's message cache. The target thread retrieves the message (and empties the cache) by calling [`receive_data()`](TheKernelKit_ThreadsAndTeams.md#receive_data "receive_data\(\)").

There are two parts to the message:

  * A single four-byte code passed as an argument to `send_data()` and returned directly by [`receive_data()`](TheKernelKit_ThreadsAndTeams.md#receive_data "receive_data\(\)").

  * A `buffer` of data that's `buffer_size` bytes long (`buffer` can be `NULL`, in which case `buffer_size` should be 0). The data is copied into the target thread's cache, and then copied into [`receive_data()`](TheKernelKit_ThreadsAndTeams.md#receive_data "receive_data\(\)")'s `buffer` (which must be allocated). The calling threads retain responsibility for freeing their buffers.

In addition to returning the `code` directly, and copying the message data into its `buffer` argument, [`receive_data()`](TheKernelKit_ThreadsAndTeams.md#receive_data "receive_data\(\)") sets `sender` to the id of the thread that sent the message.

`send_data()` blocks if there's an unread message in the target thread's cache; otherwise it returns immediately (i.e. it doesn't wait for the target to call [`receive_data()`](TheKernelKit_ThreadsAndTeams.md#receive_data "receive_data\(\)"). Analogously, [`receive_data()`](TheKernelKit_ThreadsAndTeams.md#receive_data "receive_data\(\)") blocks until there's a message to retrieve.

In the following example, the main thread spawns a thread, sends it a message, and then tells the thread to run:
    
    
    main()
    {
       thread_id other_thread;
       int32 code = 63;
       char *buf = "Hello";
    
       other_thread = spawn_thread(thread_func, ...);
       send_data(other_thread, code, (void *)buf, strlen(buf));
       resume_thread(other_thread);
       ...
    }

To retrieve the message, the target thread calls [`receive_data()`](TheKernelKit_ThreadsAndTeams.md#receive_data "receive_data\(\)"):
    
    
    int32 thread_func(void *data)
    {
       thread_id sender;
       int32 code;
       char buf[512];
    
       code = receive_data(&sender, (void *)buf, sizeof(buf));
       ...
    }

Keep in mind that the message data is copied into the buffer; you must allocate adequate storage for the data. If the buffer isn't big enough to accommodate all the datain the message, the left-over portion is thrown away. Note, however, that there isn't any way for a thread to determine how much data has been copied into its message cache.

Return Code| Description  
---|---  
`B_OK`.| The data was successfuly sent.  
`B_BAD_THREAD_ID`.| `thread` doesn't identify a valid thread.  
`B_NO_MEMORY`.| The target couldn't allocate enough memory for its copy of buffer.  
`B_INTERRUPTED`.| The function blocked, but a signal unblocked it.  
  
#### has_data()

`bool has_data(thread_id thread);`

`has_data()` returns `true` if `thread` has a message in its message cache. Ostensibly, you use this function before calling [`send_data()`](TheKernelKit_ThreadsAndTeams.md#send_data "send_data\(\)") or [`receive_data()`](TheKernelKit_ThreadsAndTeams.md#receive_data "receive_data\(\)") to avoid blocking:
    
    
    if (!has_data(target_thread))
       err = send_data(target_thread, ...);
    
    /* or */
    
    if (has_data(find_thread(NULL))
       code = receive_data(...);

This works for [`receive_data()`](TheKernelKit_ThreadsAndTeams.md#receive_data "receive_data\(\)"), but notice that there's a race condition between the `has_data()` and [`send_data()`](TheKernelKit_ThreadsAndTeams.md#send_data "send_data\(\)") calls. Another thread could send a message to the target in the interim.

####  set_thread_priority(), suggest_thread_priority()

`status_t set_thread_priority(thread_id thread,  
int32 new_priority);``int32 suggest_thread_priority(uint32 what = B_DEFAULT_MEDIA_PRIORITY,  
int32 period = 0,  
bigtime_t jitter = 0,  
bigtime_t length = 0);`

Declared in: `kernel/scheduler.h`

`set_thread_priority()` resets the given thread's priority to `new_priority`. The priority is expected to be between 0 and 120. See "Thread Priorities" for a description of the priority scheme, and "Thread Priority Values" for a list of pre-defined priority constants.

`suggest_thread_priority()` takes information about a thread and returns a suggested priority that you can pass to `set_thread_priority()` (or, more likely, to [`spawn_thread()`](TheKernelKit_ThreadsAndTeams.md#spawn_thread "spawn_thread\(\)")).

The `what` value is a bit mask that indicates the type of activities the thread will be used for. The possible values are listed in Suggested Thread Priorities.

`period` is the number of times per second the thread needs to be run (specify 0 if it needs to run continuously). `jitter` is an estimate, in microseconds, of how much the period can vary as long as the average stays at `period` times per second.

`length` is an approximation of the amount of time, in microseconds, the thread will typically run per invocation (i.e., the amount of time that will pass between the moment it receives a message, through processing it, until it's again waiting for another message).

For example, if you're spawning a thread to handle video refresh for a computer game, and you want the display to update 30 times per second, you might use code similar to the following:
    
    
    int32 priority;
    priority = suggest_thread_priority(B_LIVE_3D_RENDERING, 30, 1000, 150);
    th = spawn_thread(func, "render_thread", priority, NULL)

This spawns the rendering thread with a priority appropriate for a thread for live 3D rendering which wants to be run 30 times per second, with a variation of only 1000 microseconds. Each invocation of the thread's code is estimated to take 150 microseconds. Obviously the jitter and length values would have to be tuned to the particular application.

`set_thread_priority()` returns…

Return Code| Description  
---|---  
Positive integers.| If the function is successful, the previous priority is returned.  
`B_BAD_THREAD_ID`.| `thread` doesn't identify a valid thread.   
  
####  snooze(), snooze_until()

`status_t snooze(bigtime_t microseconds,  
int32 new_priority);``status_t snooze_until(bigtime_t microseconds,  
int timebase);`

`snooze()` blocks the calling thread for the given number of `microseconds`.

`snooze_until()` blocks until an absolute time measured in the given `timebase`. Currently, the only allowed value for `timebase` is `B_SYSTEM_TIMEBASE`, which measures time against the system clock (as reported by [`system_time()`](TheKernelKit_Time.md#system_time "system_time\(\)")).

Return Code| Description  
---|---  
`B_OK`.|  The thread went to sleep and is now awake.   
`B_INTERRUPTED`.|  The thread received a signal while it was sleeping.   
  
#### spawn_thread()

`thread_id spawn_thread(thread_func func,  
const char* name,  
int32 priority,  
void* data);`

Creates a new thread and returns its thread_id identifier (a positive integer). The arguments are:

Parameter| Description  
---|---  
`func`| Is a pointer to a thread function. This is the function that the thread will execute when it's told to run. See "The Thread Function" for details.  
`name`| Is the name that you wish to give the thread. It can be, at most, `B_OS_NAME_LENGTH` (32) characters long.  
`priority`| Is the CPU priority level of the thread. This value should be between 0 and 120; he higher the priority, the more attention the thread gets. See Thread Priorities for a description of the priorities, and Thread Priority Values for a list of priority constants.  
`data`| Is forwarded as the argument to the thread function.  
  
A newly spawned thread is in a suspended state (`B_THREAD_SUSPENDED`). To tell the thread to run, you pass its thread_id to the [`resume_thread()`](TheKernelKit_ThreadsAndTeams.md#resume_thread "resume_thread\(\)") function. The thread will continue to run until the thread function exits, or until the thread is explicitly killed (through a signal or a call to [`exit_thread()`](TheKernelKit_ThreadsAndTeams.md#exit_thread "exit_thread\(\), kill_thread\(\), kill_team\(\), on_exit_thread\(\)"), [`kill_thread()`](TheKernelKit_ThreadsAndTeams.md#kill_thread), or [`kill_team()`](TheKernelKit_ThreadsAndTeams.md#kill_team)).

Return Code| Description  
---|---  
`B_NO_MORE_THREADS`.| All thread_id numbers are currently in use.  
`B_NO_MEMORY`.| Not enough memory to allocate the resources for another thread.  
  
#### suspend_thread()

`status_t suspend_thread(thread_id thread);`

Halts the execution of the given thread, but doesn't kill the thread entirely. The thread remains suspended (`suspend_thread()` blocks) until it's told to run through the [`resume_thread()`](TheKernelKit_ThreadsAndTeams.md#resume_thread "resume_thread\(\)") function. Nothing prevents you from suspending your own thread, i.e.:
    
    
    suspend_thread(find_thread(NULL));

Of course, this is only smart if you have some other thread that will resume you later.

You can suspend any thread, regardless of its current state. But be careful: If the thread is blocked on a semaphore (for example), the subsequent [`resume_thread()`](TheKernelKit_ThreadsAndTeams.md#resume_thread "resume_thread\(\)") call will "hop over" the semaphore acquisition.

Suspensions don't nest. A single [`resume_thread()`](TheKernelKit_ThreadsAndTeams.md#resume_thread "resume_thread\(\)") unsuspends a thread regardless of the number of `suspend_thread()` calls it has received.

`suspend_thread()` is the same as sending a SIGSTOP signal to the thread.

Return Code| Description  
---|---  
`B_OK`.| The thread is now suspended.  
`B_BAD_THREAD_ID`.| thread isn't a valid thread_id number.  
  
#### wait_for_thread()

`status_t wait_for_thread(thread_id thread,  
status_t* exit_value);`

This function causes the calling thread to wait until thread (the "target thread") has died. If thread is suspended (or freshly spawned), `wait_for_thread()` will resume it.

When the target thread is dead, the value that was returned by its thread function (or imposed by [`exit_thread()`](TheKernelKit_ThreadsAndTeams.md#exit_thread "exit_thread\(\), kill_thread\(\), kill_team\(\), on_exit_thread\(\)")) is returned in `exit_value`. If the target thread was killed (by [`kill_thread()`](TheKernelKit_ThreadsAndTeams.md#kill_thread) or [`kill_team()`](TheKernelKit_ThreadsAndTeams.md#kill_team)), or if the thread function doesn't return a value, the value returned in `exit_value` will be unreliable.

You must pass a valid pointer as the second argument to `wait_for_thread()`. You mustn't pass `NULL` even if you're not interested in the return value.

Return Code| Description  
---|---  
`B_OK`.| The target is now dead.  
`B_BAD_THREAD_ID`.| `thread` isn't a valid thread_id number.  
`B_INTERRUPTED`.| The target was killed by a signal. This includes [`kill_thread()`](TheKernelKit_ThreadsAndTeams.md#kill_thread), [`kill_team()`](TheKernelKit_ThreadsAndTeams.md#kill_team), and [`exit_thread()`](TheKernelKit_ThreadsAndTeams.md#exit_thread "exit_thread\(\), kill_thread\(\), kill_team\(\), on_exit_thread\(\)").  
  
* * *

### Thread and Team Structures and Types

####  team_id, thread_id
    
    
    typedef int32 team_id ;
    
    typedef int32 thread_id ;

These id numbers uniquely identify teams and threads, respecitvely.

#### team_info
    
    
    typedef struct {
        team_id   team;
        int32     thread_count;
        int32     image_count;
        int32     area_count;
        thread_id debugger_nub_thread;
        port_id   debugger_nub_port;
        int32     argc;
        char      args[64];
        uid_t     uid;
        gid_t     gid;
    } team_info;

The team_info structure returns information about a team. To retrieve one of these structures, use [`get_team_info()`](TheKernelKit_ThreadsAndTeams.md#get_team_info "get_team_info\(\), get_next_team_info\(\)") or [`get_next_team_info()`](TheKernelKit_ThreadsAndTeams.md#get_next_team_info).

The first field is obvious; the next three reasonably so: They give the number of threads that have been spawned, images that have been loaded, and areas that have been created or cloned within this team.

The debugger fields are used by the, uhm, the…debugger?

The `argc` field is the number of command line arguments that were used to launch the team; `args` is a copy of the first 64 characters from the command line invocation. If this team is an application that was launched through the user interface (by double-clicking, or by accepting a dropped icon), then `argc` is 1 and `args` is the name of the application's executable file.

`uid` and `gid` identify the user and group that "owns" the team. You can use these values to play permission games.

#### thread_func
    
    
    typedef int32 (*thread_func)(void *data);

thread_func is the prototype for a thread's thread function. You specify a thread function by passing a thread_func as the first argument to [`spawn_thread()`](TheKernelKit_ThreadsAndTeams.md#spawn_thread "spawn_thread\(\)"); the last argument to [`spawn_thread()`](TheKernelKit_ThreadsAndTeams.md#spawn_thread "spawn_thread\(\)") is forwarded as the thread function's data argument. When the thread function exits, the spawned thread is automatically killed. To retrieve a thread_func's return value, some other thread must be waiting in a [`wait_for_thread()`](TheKernelKit_ThreadsAndTeams.md#wait_for_thread "wait_for_thread\(\)") call.

Note that [`spawn_thread()`](TheKernelKit_ThreadsAndTeams.md#spawn_thread "spawn_thread\(\)") doesn't copy the data that data points to. It simply passes the pointer through literally. Never pass a pointer that's allocated locally (on the stack).

#### thread_info
    
    
    typedef struct {
        thread_id    thread;
        team_id      team;
        char         name[B_OS_NAME_LENGTH];
        thread_state state;
        sem_id       sem;
        int32        priority;
        bigtime_t    user_time;
        bigtime_t    kernel_time;
        void *       stack_base;
        void *       stack_end;
    } thread_info

The thread_info structure contains information about a thread. To retrieve one of these structure, use [`get_thread_info()`](TheKernelKit_ThreadsAndTeams.md#get_thread_info "get_thread_info\(\), get_next_thread_info\(\)") or [`get_next_thread_info()`](TheKernelKit_ThreadsAndTeams.md#get_next_thread_info).

The `thread`, `team`, and `name` fields contain the indicated information.

`state` describes what the thread is currently doing (see [thread_state](TheKernelKit_ThreadsAndTeams.md#thread_state "thread_state") for the list of states). If the thread is waiting to acquire a semaphore, sem is that semaphore.

`priority` is a value that indicates the level of attention the thread gets (see Thread Priority).

`user_time` and `kernel_time` are the amounts of time, in microseconds, the thread has spent executing user code and the amount of time the kernel has run on the thread's behalf, respectively.

`stack_base` and `stack_end` are pointers to the first byte and last bytes in the thread's execution stack. Currently, the stack size is fixed at around 256k.

Warning

![Warning](./images/admonitions/Stop_32.png)

The two stack pointers are currently inverted such that `stack_base` is _less_ than `stack_end`. (In a stack-grows-down world, the base should be greater than the end.)

* * *

### Thread and Team Constants

#### B_SYSTEM_TEAM
    
    
    #define B_SYSTEM_TEAM ...

Use this constant as the first argument to [`get_team_info()`](TheKernelKit_ThreadsAndTeams.md#get_team_info "get_team_info\(\), get_next_team_info\(\)") to get team information about the kernel).

#### B_SYSTEM_TIMEBASE
    
    
    #define B_SYSTEM_TIMEBASE ...

The system timebase constant is used as a basis for time measurement in the [`snooze_until()`](TheKernelKit_ThreadsAndTeams.md#snooze_until) function. (Currently, it's the only timebase available.)

####  be_task_flags
    
    
    enum be_task_flags {
        B_DEFAULT_MEDIA_PRIORITY,
        B_OFFLINE_PROCESSING,
        B_STATUS_RENDERING,
        B_USER_INPUT_HANDLING,
        B_LIVE_VIDEO_MANIPULATION,
        B_VIDEO_PLAYBACK,
        B_VIDEO_RECORDING,
        B_LIVE_AUDIO_MANIPULATION,
        B_AUDIO_PLAYBACK,
        B_AUDIO_RECORDING,
        B_LIVE_3D_RENDERING,
        B_NUMBER_CRUNCHING
    };

Declared in: `kernel/scheduler.h`

Constant| Description  
---|---  
`B_DEFAULT_MEDIA_PRIORITY`.| The thread isn't doing anything specialized.  
`B_OFFLINE_PROCESSING`.| The thread is doing non-real-time computations.  
`B_STATUS_RENDERING`.| The thread is rendering a status or preview display.  
`B_USER_INPUT_HANDLING`.| The thread is handling user input.  
`B_LIVE_VIDEO_MANIPULATION`.| The thread is processing live video (filtering, compression, decompression, etc.).  
`B_VIDEO_PLAYBACK`.| The thread is playing back video from a hardware device.  
`B_VIDEO_RECORDING`.| The thread is recording video from a hardware device.  
`B_LIVE_AUDIO_MANIPULATION`.| The thread is doing real-time manipulation of live audio data (filtering, compression, decompression, etc.).  
`B_AUDIO_PLAYBACK`.| The thread is playing back audio from a hardware device.  
`B_AUDIO_RECORDING`.| The thread is recording audio from a hardware device.  
`B_LIVE_3D_RENDERING`.| The thread is performing live 3D rendering.  
`B_NUMBER_CRUNCHING`.| The thread is doing data processing.  
  
These constants describe what the thread is designed to do. You use these constants when asking for a suggested priority (see [`suggest_thread_priority()`](TheKernelKit_ThreadsAndTeams.md#suggest_thread_priority)).

Warning

![Warning](./images/admonitions/Stop_32.png)

These constants may not be used as actual thread priority values—do not pass one of these values as the priority argument to [`spawn_thread()`](TheKernelKit_ThreadsAndTeams.md#spawn_thread "spawn_thread\(\)").

####  Thread Priority Values

Time-Sharing Priority| Value  
---|---  
`B_LOW_PRIORITY`| 5  
`B_NORMAL_PRIORITY`| 10  
`B_DISPLAY_PRIORITY`| 15  
`B_URGENT_DISPLAY_PRIORITY`| 20  
  
Real-Time Priority| Value  
---|---  
`B_REAL_TIME_DISPLAY_PRIORITY`| 100  
`B_URGENT_PRIORITY`| 110  
`B_REAL_TIME_PRIORITY`| 120  
  
The thread priority values are used to set the "urgency" of a thread. Although you can reset a thread's priority through [`set_thread_priority()`](TheKernelKit_ThreadsAndTeams.md#set_thread_priority "set_thread_priority\(\), suggest_thread_priority\(\)"), the priority is initially—and almost always permanently—set in [`spawn_thread()`](TheKernelKit_ThreadsAndTeams.md#spawn_thread "spawn_thread\(\)").

####  thread_state
    
    
    enum { ... } thread_state

Constant| Description  
---|---  
`B_THREAD_RUNNING`.| The thread is currently receiving attention from a CPU.  
`B_THREAD_READY`.| The thread is waiting for its turn to receive attention.  
`B_THREAD_SUSPENDED`.| The thread has been suspended or is freshly-spawned and is waiting to start.  
`B_THREAD_WAITING`.| The thread is waiting to acquire a semaphore. The `sem` field of the thread's [thread_info](TheKernelKit_ThreadsAndTeams.md#thread_info "thread_info") structure will tell you which semaphore.  
`B_THREAD_RECEIVING`.| The thread is sitting in a [`receive_data()`](TheKernelKit_ThreadsAndTeams.md#receive_data "receive_data\(\)") function call.  
`B_THREAD_ASLEEP`.| The thread is sitting in a [`snooze()`](TheKernelKit_ThreadsAndTeams.md#snooze "snooze\(\), snooze_until\(\)") call.  
  
A thread's state tells you what the thread is currently doing. To get the state, look in the state field of the [thread_info](TheKernelKit_ThreadsAndTeams.md#thread_info "thread_info") structure (retrieved through [`get_thread_info()`](TheKernelKit_ThreadsAndTeams.md#get_thread_info "get_thread_info\(\), get_next_thread_info\(\)")).

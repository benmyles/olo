<!-- System Overview - The Kernel Kit - Threads And Teams -->

## Threads And Teams

A thread is a synchronous process that executes a series of program instructions. A team is a group of threads that make up a single program or application.

Every application has at least one thread: When you launch an application, an initial thread—the main thread—is automatically created (or spawned) and told to run. The main thread executes the ubiquitous `main()` function, winds through the functions that are called from `main()`, and is automatically deleted (or killed) when `main()` exits.

The Be operating system is multithreaded: from the main thread you can spawn and run additional threads; from each of these threads you can spawn and run more threads, and so on. All the threads in all applications run concurrently and asynchronously with each other.

Threads are independent of each other. Most notably, a given thread doesn't own the other threads it has spawned. For example, if thread A spawns thread B, and thread A dies (for whatever reason), thread B will continue to run. (But before you get carried away with the idea of leap-frogging threads, you should take note of the caveat in "[Death and the Main Thread](TheKernelKit_ThreadsAndTeams_Overview.md#ThreadsAndTeams_DeathAndTheMainThread "Death and the Main Thread")".)

Warning

![Warning](./images/admonitions/Stop_32.png)

Threads and the POSIX `fork()` function are not compatible. You can't mix calls to [`spawn_thread()`](TheKernelKit_ThreadsAndTeams.md#spawn_thread "spawn_thread\(\)") (the function that creates a new thread) and `fork()` in the same application: If you call [`spawn_thread()`](TheKernelKit_ThreadsAndTeams.md#spawn_thread "spawn_thread\(\)") and then try to call `fork()`, the `fork()` call will fail. And vice versa.

Although threads are independent, they do fall into groups called teams. A team consists of a main thread and all other threads that "descend" from it (that are spawned by the main thread directly, or by any thread that was spawned by the main thread, and so on). Viewed from a higher level, a team is the group of threads that are created by a single application. You can't "transfer" threads from one team to another. The team is set when the thread is spawned; it remains the same throughout the thread's life.

All the threads in a particular team share the same address space: Global variables that are declared by one thread will be visible to all other threads in that team.

* * *

### Spawning a Thread

You spawn a thread by calling the [`spawn_thread()`](TheKernelKit_ThreadsAndTeams.md#spawn_thread "spawn_thread\(\)") function. The function assigns and returns a system-wide thread_id number that you use to identify the new thread in subsequent function calls. Valid thread_id numbers are positive integers; you can check the success of a spawn thus:
    
    
    thread_id my_thread = spawn_thread(...);
    
    if ((my_thread) < B_OK)
       /* failure */
    else
       /* success */

The arguments to [`spawn_thread()`](TheKernelKit_ThreadsAndTeams.md#spawn_thread "spawn_thread\(\)"), which are examined throughout this description, supply information such as what the thread is supposed to do, the urgency of its operation, and so on.

#### Threads and App Images

A conceptual neighbor of spawning a thread is the act of loading an executable (or loading an app image). This is performed by calling the [`load_image()`](TheKernelKit_Images.md#load_image "load_image\(\)") function. Loading an image causes a separate program, identified as a file, to be launched by the system. For more information on the [`load_image()`](TheKernelKit_Images.md#load_image "load_image\(\)") function, see [Images](TheKernelKit_Images.md "Images").

* * *

### Telling a Thread to Run

Spawning a thread isn't enough to make it run. To tell a thread to start running, you must pass its thread_id number to either the [`resume_thread()`](TheKernelKit_ThreadsAndTeams.md#resume_thread "resume_thread\(\)") or [`wait_for_thread()`](TheKernelKit_ThreadsAndTeams.md#wait_for_thread "wait_for_thread\(\)") function:

Function| Description  
---|---  
[`resume_thread()`](TheKernelKit_ThreadsAndTeams.md#resume_thread "resume_thread\(\)")| Starts the new thread running and immediately returns. The new thread runs concurrently and asynchronously with the thread in which [`resume_thread()`](TheKernelKit_ThreadsAndTeams.md#resume_thread "resume_thread\(\)") was called.  
[`wait_for_thread()`](TheKernelKit_ThreadsAndTeams.md#wait_for_thread "wait_for_thread\(\)")| Starts the thread running but doesn't return until the thread has finished. (You can also call [`wait_for_thread()`](TheKernelKit_ThreadsAndTeams.md#wait_for_thread "wait_for_thread\(\)") on a thread that's already running.)  
  
Of these two functions, [`resume_thread()`](TheKernelKit_ThreadsAndTeams.md#resume_thread "resume_thread\(\)") is the more common means for starting a thread that was created through [`spawn_thread()`](TheKernelKit_ThreadsAndTeams.md#spawn_thread "spawn_thread\(\)"). [`wait_for_thread()`](TheKernelKit_ThreadsAndTeams.md#wait_for_thread "wait_for_thread\(\)") is typically used to start the thread that was created through [`load_image()`](TheKernelKit_Images.md#load_image "load_image\(\)").

* * *

### The Thread Function

When you call [`spawn_thread()`](TheKernelKit_ThreadsAndTeams.md#spawn_thread "spawn_thread\(\)"), you must identify the new thread's thread function. This is a global C function (or a static C++ member function) that the new thread will execute when it's told to run. The thread function, defined as [thread_func](TheKernelKit_ThreadsAndTeams.md#thread_func "thread_func"), takes a single (void *) argument and returns an int32 error code. When the thread function exits, the thread is automatically killed.

You pass a thread function as the first argument to [`spawn_thread()`](TheKernelKit_ThreadsAndTeams.md#spawn_thread "spawn_thread\(\)"). For example, here we spawn a thread that uses a function called `lister()` as its thread function. The last argument to [`spawn_thread()`](TheKernelKit_ThreadsAndTeams.md#spawn_thread "spawn_thread\(\)") is forwarded to the thread function:
    
    
    int32 lister(void *data)
    {
       /* Cast the argument. */
       BList *listObj = (BList *)data;
       ...
    }
    
    int32 main()
    {
       BList *listObj = new BList();
       thread_id my_thread;
    
       my_thread = spawn_thread(lister, ..., (void *)listObj);
       resume_thread(my_thread);
       ...
    }

See [Passing Data to a Thread](TheKernelKit_ThreadsAndTeams_Overview.md#ThreadsAndTeams_PassingDataToAThread "Passing Data to a Thread") for other methods of passing data to a thread.

* * *

### Thread Names

A thread can be given a name which you assign through the second argument to [`spawn_thread()`](TheKernelKit_ThreadsAndTeams.md#spawn_thread "spawn_thread\(\)"). The name can be 32 characters long (as represented by the `B_OS_NAME_LENGTH` constant) and needn't be unique—more than one thread can have the same name.

You can look for a thread based on its name by passing the name to the [`find_thread()`](TheKernelKit_ThreadsAndTeams.md#find_thread "find_thread\(\)") function; the function returns the thread_id of the so-named thread. If two or more threads bear the same name, the [`find_thread()`](TheKernelKit_ThreadsAndTeams.md#find_thread "find_thread\(\)") function returns the first of these threads that it finds.

You can retrieve the thread_id of the calling thread by passing `NULL` to [`find_thread()`](TheKernelKit_ThreadsAndTeams.md#find_thread "find_thread\(\)"):
    
    
    thread_id this_thread = find_thread(NULL);

To retrieve a thread's name, you must look in the thread's [thread_info](TheKernelKit_ThreadsAndTeams.md#thread_info "thread_info") structure. This structure is described in the [`get_thread_info()`](TheKernelKit_ThreadsAndTeams.md#get_thread_info "get_thread_info\(\), get_next_thread_info\(\)") function description.

Dissatisfied with a thread's name? Use the [`rename_thread()`](TheKernelKit_ThreadsAndTeams.md#rename_thread "rename_thread\(\)") function to change it. Fool your friends.

* * *

### Thread Priorities

In a multi-threaded environment, the CPUs must divide their attention between the candidate threads, executing a few instructions from this thread, then a few from that thread, and so on. But the division of attention isn't always equal: You can assign a higher or lower priority to a thread and so declare it to be more or less important than other threads.

You assign a thread's priority (an integer) as the third argument to [`spawn_thread()`](TheKernelKit_ThreadsAndTeams.md#spawn_thread "spawn_thread\(\)"). There are two categories of priorities: "time-sharing" and "real-time."

Time-sharing (values from 1 to 99).
    

A time-sharing thread is executed only if there are no real-time threads in the ready queue. In the absence of real-time threads, a time-sharing thread is elected to run once every "scheduler quantum" (currently, every three milliseconds). The higher the time-sharing thread's priority value, the greater the chance that it will be the next thread to run.

Real-time (100 and greater).
    

A real-time thread is executed as soon as it's ready. If more than one real-time thread is ready at the same time, the thread with the highest priority is executed first. The thread is allowed to run without being preempted (except by a real-time thread with a higher priority) until it blocks, snoozes, is suspended, or otherwise gives up its plea for attention.

The Kernel Kit defines seven priority constants (see Thread Priority Values for the list). Although you can use other, "in-between" value as the priority argument to [`spawn_thread()`](TheKernelKit_ThreadsAndTeams.md#spawn_thread "spawn_thread\(\)"), it's suggested that you stick with these.

Furthermore, you can call the [`suggest_thread_priority()`](TheKernelKit_ThreadsAndTeams.md#suggest_thread_priority) function to let the Kernel Kit determine a good priority for your thread. This function takes information about the thread's scheduling and CPU needs, and returns a reasonable priority value to use when spawning the thread.

* * *

### Synchronizing Threads

There are times when you may want a particular thread to pause at a designated point until some other (known) thread finishes some task. Here are three ways to effect this sort of synchronization:

  * The most general means for synchronizing threads is to use a semaphore. The semaphore mechanism is described in great detail in [Semaphores](TheKernelKit_Semaphores.md "Semaphores").

  * Synchronization is sometimes a side-effect of sending data between threads. This is explained in "[Passing Data to a Thread](TheKernelKit_ThreadsAndTeams_Overview.md#ThreadsAndTeams_PassingDataToAThread "Passing Data to a Thread")", and in [Ports](TheKernelKit_Ports.md "Ports").

  * Finally, you can tell a thread to wait for some other thread to die by calling [`wait_for_thread()`](TheKernelKit_ThreadsAndTeams.md#wait_for_thread "wait_for_thread\(\)"), as described earlier.

* * *

### Controlling a Thread

There are four ways to control a thread while it's running:

  1. You can put the calling thread to sleep for some number of microseconds through the [`snooze()`](TheKernelKit_ThreadsAndTeams.md#snooze "snooze\(\), snooze_until\(\)") and [`snooze_until()`](TheKernelKit_ThreadsAndTeams.md#snooze_until) functions.

  2. You can suspend the execution of any thread through the [`suspend_thread()`](TheKernelKit_ThreadsAndTeams.md#suspend_thread "suspend_thread\(\)") function. The thread remains suspended until you "unsuspend" it through a call to [`resume_thread()`](TheKernelKit_ThreadsAndTeams.md#resume_thread "resume_thread\(\)") or [`wait_for_thread()`](TheKernelKit_ThreadsAndTeams.md#wait_for_thread "wait_for_thread\(\)").

  3. You can send a POSIX "signal" to a thread through the `send_signal()` function. The SIGCONT signal tries to unblock a blocked or sleeping thread without killing it; all other signals kill the thread. To override this behavior, you can install your own signal handlers.

  4. You can kill the calling thread through [`exit_thread()`](TheKernelKit_ThreadsAndTeams.md#exit_thread "exit_thread\(\), kill_thread\(\), kill_team\(\), on_exit_thread\(\)"), or kill some other thread through [`kill_thread()`](TheKernelKit_ThreadsAndTeams.md#kill_thread). Feeling itchy? Try killing an entire team of threads: The [`kill_team()`](TheKernelKit_ThreadsAndTeams.md#kill_team) function is more than a system call. It's therapy.

#### Death and the Main Thread

As mentioned earlier, the control that's imposed upon a particular thread isn't visited upon the "children" that have been spawned from that thread. However, the death of an application's main thread can affect the other threads:

Warning

![Warning](./images/admonitions/Stop_32.png)

When a main thread dies, the game is pretty much over. The main thread takes the team's heap, its statically allocated objects, and other team-wide resources—such as access to standard IO—with it. This may seriously cripple any threads that linger beyond the death of the main thread.

It's possible to create an application in which the main thread sets up one or more other threads, gets them running, and then dies. But such applications should be rare. In general, you should try to keep your main thread around until all other threads in the team are dead.

* * *

### Passing Data to a Thread

Every thread has a message cache. You can write to a thread's message cache through the [`send_data()`](TheKernelKit_ThreadsAndTeams.md#send_data "send_data\(\)") function. The thread can pick up your message (a combination of an integer and a buffer) through [`receive_data()`](TheKernelKit_ThreadsAndTeams.md#receive_data "receive_data\(\)"). The cache is only one message deep; if there's a message already in the cache, [`send_data()`](TheKernelKit_ThreadsAndTeams.md#send_data "send_data\(\)") will block. Conversely, if there's no message in the cache, [`receive_data()`](TheKernelKit_ThreadsAndTeams.md#receive_data "receive_data\(\)") will block.

You can also pass data to thread through a port. Arbitrarily deep, ports are more flexible than the message cache. See [Ports](TheKernelKit_Ports.md "Ports") for details.

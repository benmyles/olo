<!-- System Overview - The Kernel Kit - Introduction -->

## Introduction

The Kernel Kit is a collection of C functions that let you define and control the contexts in which your application operates. There are five main topics in the Kit:

[Threads And Teams](TheKernelKit_ThreadsAndTeams_Overview.md "Threads And Teams").
    

A thread is a synchronous computer process. By creating multiple threads, you can make your application perform different tasks at (virtually) the same time. A team is the collection of threads that your application creates.

[Ports](TheKernelKit_Ports_Overview.md "Ports").
    

A port can be thought of as a mailbox for threads: A thread can write a message to a port, and some other thread (or, less usefully, the same thread) can then retrieve the message.

[Semaphores](TheKernelKit_Semaphores_Overview.md "Semaphores").
    

A semaphore is a system-wide counting variable that can be used as a lock that protects a piece of code. Before a thread is allowed to execute the code, it must acquire the semaphore that guards it. Semaphores can also be used to synchronize the execution of two or more threads.

[Areas](TheKernelKit_Areas_Overview.md "Areas").
    

The area functions let you allocate large chunks of virtual memory. The two primary features of areas are: They can be locked into the CPU's on-chip memory, and the data they hold can be shared between applications.

[Images](TheKernelKit_Images_Overview.md "Images").
    

An image is compiled code that can be dynamically linked into a running application. By loading and unloading images you can make run-time decisions about the resources that your application has access to. Images are of particular interest to driver designers.

The rest of this chapter describes these topics in detail. The final three sections…

  * System Information,

  * Time Information,

  * and Miscellaneous Functions and Constants

…fill in the gaps.

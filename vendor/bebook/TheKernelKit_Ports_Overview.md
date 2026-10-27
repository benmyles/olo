<!-- System Overview - The Kernel Kit - Ports -->

## Ports

A port is a system-wide message repository into which any thread can copy a buffer of data, and from which any thread can then retrieve the buffer. This repository is implemented as a first-in/first-out message queue: A port stores its messages in the order in which they're received, and it relinquishes them in the order in which they're stored. Each port has its own message queue.

* * *

### Creating and Destroying a Port

The [`create_port()`](TheKernelKit_Ports.md#create_port "create_port\(\)") function creates a new port and assigns it a unique, system-wide port_id number. Although ports are accessible to all threads, the port_id numbers aren't disseminated by the operating system—there's no "find_port" function. If you create a port and want some other thread to be able to write to or read from it, you have to broadcast the port_id number to that thread.

A port is owned by the team in which it was created. When a team dies (when all its threads are killed), the ports that belong to the team are deleted. A team can bestow ownership of its ports to some other team through the [`set_port_owner()`](TheKernelKit_Ports.md#set_port_owner "set_port_owner\(\)") function.

If you want explicitly get rid of a port, you call [`delete_port()`](TheKernelKit_Ports.md#delete_port "delete_port\(\)"). You can delete any port, not just those that are owned by the team of the calling thread. When you delete a port, all of its unread messages are thrown away. If you want to read this messages, but you don't want any new messages to arrive in the meantime, you should call [`close_port()`](TheKernelKit_Ports.md#close_port "close_port\(\)") before deleting the port. Note that you can't reopen a closed port; after you get done reading the port's messages, you're expected to delete the port.

* * *

### The Message Queue: Reading and Writing Port Messages

The length of a port's message queue—the number of messages that it can hold at a time—is set when the port is created.

The functions [`write_port()`](TheKernelKit_Ports.md#write_port "write_port\(\), write_port_etc\(\)") and [`read_port()`](TheKernelKit_Ports.md#read_port "read_port\(\), read_port_etc\(\)") manipulate a port's message queue: [`write_port()`](TheKernelKit_Ports.md#write_port "write_port\(\), write_port_etc\(\)") places a message at the tail of the port's message queue; [`read_port()`](TheKernelKit_Ports.md#read_port "read_port\(\), read_port_etc\(\)") removes the message at the head of the queue and returns it the caller. [`write_port()`](TheKernelKit_Ports.md#write_port "write_port\(\), write_port_etc\(\)") blocks if the queue is full; it returns when room is made in the queue by an invocation of [`read_port()`](TheKernelKit_Ports.md#read_port "read_port\(\), read_port_etc\(\)"). Similarly, if the queue is empty, [`read_port()`](TheKernelKit_Ports.md#read_port "read_port\(\), read_port_etc\(\)") blocks until [`write_port()`](TheKernelKit_Ports.md#write_port "write_port\(\), write_port_etc\(\)") is called.

You can provide a timeout for your port-writing and port-reading operations by using the "full-blown" functions [`write_port_etc()`](TheKernelKit_Ports.md#write_port_etc) and [`read_port_etc()`](TheKernelKit_Ports.md#read_port_etc). By supplying a timeout, you can ensure that your port operations won't block forever.

Although each port has its own message queue, all ports share a global "queue slot" pool—there are only so many message queue slots that can be used by all ports taken cumulatively. If too many port queues are allowed to fill up, the slot pool will drain, which will cause write_port() calls on less-than-full ports to block. To avoid this situation, you should make sure that your write_port() and read_port() calls are reasonably balanced.

The [`write_port()`](TheKernelKit_Ports.md#write_port "write_port\(\), write_port_etc\(\)") and [`read_port()`](TheKernelKit_Ports.md#read_port "read_port\(\), read_port_etc\(\)") functions are the only way to traverse a port's message queue. There's no notion of "peeking" at the queue's unread messages, or of erasing messages that are in the queue.

* * *

### Port Messages

A port message—the data that's sent through a port—consists of a "message code" and a "message buffer." Either of these elements can be used however you like, but they're intended to fit these purposes:

  * The message code (a single four-byte value) should be a mask, flag, or other predictable value that gives a general representation of the flavor or import of the message. For this to work, the sender and receiver of the message must agree on the meanings of the values that the code can take.

  * The data in the message buffer can elaborate upon the code, identify the sender of the message, or otherwise supply additional information. The length of the buffer isn't restricted. To get the length of the message buffer that's at the head of a port's queue, you call the [`port_buffer_size()`](TheKernelKit_Ports.md#port_buffer_size "port_buffer_size\(\), port_buffer_size_etc\(\)") function.

The message that you pass to [`write_port()`](TheKernelKit_Ports.md#write_port "write_port\(\), write_port_etc\(\)") is copied into the port. After [`write_port()`](TheKernelKit_Ports.md#write_port "write_port\(\), write_port_etc\(\)") returns, you may free the message data without affecting the copy that the port holds.

When you read a port, you have to supply a buffer into which the port mechanism can copy the message. If the buffer that you supply isn't large enough to accommodate the message, the unread portion will be lost—the next call to [`read_port()`](TheKernelKit_Ports.md#read_port "read_port\(\), read_port_etc\(\)") won't finish reading the message.

You typically allocate the buffer that you pass to [`read_port()`](TheKernelKit_Ports.md#read_port "read_port\(\), read_port_etc\(\)") by first calling [`port_buffer_size()`](TheKernelKit_Ports.md#port_buffer_size "port_buffer_size\(\), port_buffer_size_etc\(\)"), as shown below:
    
    
    char *buf = NULL;
    ssize_t size;
    int32 code;
    
    /* We'll assume that my_port is valid.
    * port_buffer_size() will block until a message shows up.
    */
    if ((size = port_buffer_size(my_port)) < B_OK)
       /* Handle the error */
    
    if (size > 0)
       buf = (char *)malloc(size);
    
    if (buf) {
       /* Now we can read the buffer. */
       if (read_port(my_port, &code, (void *)buf, size) < B_OK)
       /* Handle the error */

Obviously, there's a race condition (in the example) between [`port_buffer_size()`](TheKernelKit_Ports.md#port_buffer_size "port_buffer_size\(\), port_buffer_size_etc\(\)") and the subsequent [`read_port()`](TheKernelKit_Ports.md#read_port "read_port\(\), read_port_etc\(\)") call—some other thread could read the port in the interim. If you're going to use [`port_buffer_size()`](TheKernelKit_Ports.md#port_buffer_size "port_buffer_size\(\), port_buffer_size_etc\(\)") as shown in the example, you shouldn't have more than one thread reading the port at a time.

As stated in the example, [`port_buffer_size()`](TheKernelKit_Ports.md#port_buffer_size "port_buffer_size\(\), port_buffer_size_etc\(\)") blocks until a message shows up. If you don't want to (potentially) block forever, you should use the [`port_buffer_size_etc()`](TheKernelKit_Ports.md#port_buffer_size_etc) version of the function. As with the other `…etc()` functions, [`port_buffer_size_etc()`](TheKernelKit_Ports.md#port_buffer_size_etc) provides a timeout option.

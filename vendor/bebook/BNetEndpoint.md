<!-- Classes And Methods - The Network Kit - BNetEndpoint -->

## BNetEndpoint

| Derived From:| None  
---|---  
Mix-in Classes:| [`BArchivable`](BArchivable.md "BArchivable")  
Declared In:| `net/NetEndpoint.h`  
Library:| `libnetapi.so`  
Allocation:| Constructor only  
[Class Overview](BNetEndpoint_Overview.md "BNetEndpoint")

  * [Constructor and Destructor](BNetEndpoint.md#BNetEndpoint_ConstructorDestructor)
  * [Member Functions](BNetEndpoint.md#BNetEndpoint_MemberFunctions)
  * [Operators](BNetEndpoint.md#BNetEndpoint_Operators)

  
  
* * *

### Constructor and Destructor

#### BNetEndpoint()

`BNetEndpoint(int protocol = SOCK_STREAM);` `BNetEndpoint(const BNetEndpoint& from);` `BNetEndpoint([`BMessage`](BMessage.md "BMessage")* archive);`

Creates a `BNetEndpoint` representing a network connection endpoint on the local system. After construction, you must call [`InitCheck()`](BNetEndpoint.md#BNetEndpoint_InitCheck "InitCheck") to ensure that no errors occurred during setup.

The `protocol` argument lets you specify whether the `BNetEndpoint` will use a stream socket (`SOCK_STREAM`) or a datagram socket (`SOCK_DGRAM`).

By default, I/O is blocking and address reusing is off. You can change these by calling [`SetNonBlocking()`](BNetEndpoint.md#BNetEndpoint_SetNonBlocking) and [`SetReuseAddr()`](BNetEndpoint.md#BNetEndpoint_SetReuseAddr).

#### ~BNetEndpoint()

`virtual ~BNetEndpoint();`

A typical destructor.

* * *

### Member Functions

#### Bind()

`virtual status_t Bind(const [`BNetAddress`](BNetAddress.md "BNetAddress")& address);` `virtual status_t Bind(int port = 0);`

Binds the `BNetEndpoint` to a specific local address. That address can be specified by using a [`BNetAddress`](BNetAddress.md "BNetAddress") or a simple `port` number. This selects the port that will handle the local end of the connection.

If your `BNetEndpoint` is using a stream protocol and is going to be listening for connections, you must call `Bind()`.

If your stream `BNetEndpoint` is a client, it doesn't have to call `Bind()` but you can if you want to. There aren't any significant benefits to doing so, however.

A stream accept `BNetEndpoint`s must not be bound.

Datagram `BNetEndpoint`s that are going to receive data must be bound; datagram `BNetEndpoint`s that will only be sending data don't have to be. However, if an endpoint will both send and receive, it must be bound.

If you don't specify an `address` or `port` number, or specify a `port` number of 0, `Bind()` will bind the `BNetEndpoint` to a random local port. You can determine which one by calling [`LocalAddr()`](BNetEndpoint.md#BNetEndpoint_LocalAddr "LocalAddr\(\), RemoteAddr\(\)").

The only way to unbind a `BNetEndpoint` from an address or port is to close the endpoint.

Return Code| Description  
---|---  
`B_OK`| The address was successfully bound to.  
`B_ERROR`| An error occurred.  
  
#### Close()

`virtual void Close();`

Closes the connection, if there is one. If there's unread data buffered up, it's disposed of.

#### Connect()

`virtual status_t Connect(const [`BNetAddress`](BNetAddress.md "BNetAddress")& address);` `virtual status_t Connect(const char* address,  
unsigned short port);`

Opens a connection to the specified remote system. The system's address can be specified by either using a `BNetAddress` or by specifying the IP address or domain name and port number. For example, to connect to the Megaburger, Inc. web server, your software would call:
    
    
    status_t err = myEndpoint->Connect("www.megaburger.com", 80);
    if (err != B_OK) {
       /* error occurred */
    }
    else {
       /* all is well, connection is open */
    }

Return Code| Description  
---|---  
`B_OK`| The connection was opened.  
`B_ERROR`| An error occurred.  
  
####  Error(), ErrorStr()

`int Error() const;``char* ErrorStr() const;`

`Error()` returns the integer error code for the last send or receive error. If you receive a `B_ERROR` result from a send or receive function, you can find out the specific error using this function.

`ErrorStr()` returns a pointer to a text string describing the error; this string isn't yours, so don't try to `free()` it.

#### InitCheck

`status_t InitCheck() const;`

Returns a status_t indicating whether or not the object was successfully instantiated.

Return Code| Description  
---|---  
`B_OK`| The `BNetEndpoint` was constructed without error.  
`B_ERROR`| An error occurred during construction.  
  
#### IsDataPending

`virtual bool IsDataPending(bigtime_t timeout = 0);`

Returns `true` if there's data waiting to be received, otherwise returns `false`. If you specify a `timeout` other than 0, the function will block until either data is available or the timeout period elapses.

####  Listen(), Accept()

`virtual status_t Listen(int backlog = 5);``virtual BNetEndpoint* Accept(int32 timeout = -1);`

`Listen()` tells the `BNetEndpoint` to begin listening for incoming connection attempts on its local port. These attempts are queued; up to backlog attempts can be in the queue at any time. If more attempts are backlogged than that, the later attempts will be rejected until there's room in the queue.

You can accept an incoming connection attempt by calling `Accept()`. If there are no connection attempts queued up, this function returns `NULL`. If there are connection attempts in the queue, a new `BNetEndpoint` object is created with the connection between your local port and the remote system opened, and that `BNetEndpoint` is returned to you.

The new connection is yours to do with as you please. When you're finished with the connection, you must delete the returned `BNetEndpoint`. Typically your listener thread will look something like this:
    
    
    long MyListener(void* data) {
       BNetEndpoint endpoint;
    
       if (endpoint.InitCheck() < B_OK) {
          return -1;
       }
    
       /* bind to the desired port */
       endpoint.Bind(portNumber);
    
       /* listen for connections */
       endpoint.Listen();
    
       while (keepListening) {
          BNetEndpoint* connect = NULL;
          connect = endpoint.Accept();
          if (connect) {
             /* call a function do the work */
             handle_connection(connect, data);
             delete connect;
          }
       }
       endpoint.Close();
    }

Return Code| Description  
---|---  
`B_OK`| Success.  
`B_ERROR`| Failure.  
  
####  LocalAddr(), RemoteAddr()

`const [`BNetAddress`](BNetAddress.md "BNetAddress")& LocalAddr();``const [`BNetAddress`](BNetAddress.md "BNetAddress")& RemoteAddr();`

These functions return a [`BNetAddress`](BNetAddress.md "BNetAddress") representing the local or remote system on the connection. `LocalAddr()` returns the address of the local machine, and `RemoteAddr()` (amazingly enough) returns the address of the remote system.

If there isn't a remote connection, `RemoteAddr()` will return a [`BNetAddress`](BNetAddress.md "BNetAddress") indicating an IP address of 0.0.0.0.

####  Receive(), ReceiveFrom()

`virtual int32 Receive(const void* buffer,  
size_t size,  
int flags = 0);` `virtual int32 Receive([`BNetBuffer`](BNetBuffer.md "BNetBuffer")& buffer,  
size_t size,  
int flags = 0);`

`virtual int32 ReceiveFrom(const void* buffer,  
size_t size,  
const [`BNetAddress`](BNetAddress.md "BNetAddress")& from,  
int flags = 0);` `virtual int32 ReceiveFrom([`BNetBuffer`](BNetBuffer.md "BNetBuffer")& buffer,  
size_t size,  
const [`BNetAddress`](BNetAddress.md "BNetAddress")& from,  
int flags = 0);`

`Receive()` receives a buffer of data from the remote end of the connection. If there's no connection established, `B_ERROR` is returned immediately. Up to `size` bytes of data are received.

`ReceiveFrom()` receives the buffer from the remote system specified by the from [`BNetAddress`](BNetAddress.md "BNetAddress"). `ReceiveFrom()` only works if the connection is using a datagram protocol.

The first form of each function function sends an arbitrary buffer of the specified `size`, and the second form sends the contents of a [`BNetBuffer`](BNetBuffer.md "BNetBuffer"). When using a [`BNetBuffer`](BNetBuffer.md "BNetBuffer"), incoming data is appended to the end of the buffer, so you can use the same buffer in a loop to buffer incoming data in chunks until the desired number of bytes have been read.

The `flags` argument, which is passed on to the `socket.h` [`recv()`](NetworkSockets.md#recv) or [`recvfrom()`](NetworkSockets.md#recvfrom) function, is currently unused and must be 0.

When you call these functions in blocking mode (which is the default), they block until there's data available to receive or a timeout occurs. The timeout period is set by calling [`SetTimeout()`](BNetEndpoint.md#BNetEndpoint_SetTimeout "SetTimeout\(\)"). You can turn on or off blocking by calling [`SetNonBlocking()`](BNetEndpoint.md#BNetEndpoint_SetNonBlocking). If you're in nonblocking mode and there's no data waiting, these functions return 0 immediately, indicating that there's no data.

These functions return the number of bytes actually received, or -1 if an error occurred. You can call [`Error()`](BNetEndpoint.md#BNetEndpoint_Error "Error\(\), ErrorStr\(\)") to get the specific error that occurred.

####  Send(), SendTo()

`virtual int32 Send(const void* buffer,  
size_t size,  
int flags = 0);` `virtual int32 Send([`BNetBuffer`](BNetBuffer.md "BNetBuffer")& buffer,  
int flags = 0);`

`virtual int32 SendTo(const void* buffer,  
size_t size,  
const [`BNetAddress`](BNetAddress.md "BNetAddress")& to,  
int flags = 0);` `virtual int32 SendTo([`BNetBuffer`](BNetBuffer.md "BNetBuffer")& buffer,  
const [`BNetAddress`](BNetAddress.md "BNetAddress")& to,  
int flags = 0);`

`Send()` sends a buffer of data to the remote end of the connection. If there's no connection established, `B_ERROR` is returned immediately. In addition, if the `BNetEndpoint` is configured to use a datagram protocol, this function fails unless [`Connect()`](BNetEndpoint.md#BNetEndpoint_Connect "Connect\(\)") has been called, since that function caches the destination address.

`SendTo()` sends the buffer to the remote system specified by the to [`BNetAddress`](BNetAddress.md "BNetAddress"). `SendTo()` only works if the connection is using a datagram protocol.

The first form of each function function sends an arbitrary buffer of the specified size, and the second form sends the contents of a [`BNetBuffer`](BNetBuffer.md "BNetBuffer").

The `flags` argument, which is passed on to the the `socket.h` [`send()`](NetworkSockets.md#send "send\(\), recv\(\)") or [`sendto()`](NetworkSockets.md#sendto "sendto\(\), recvfrom\(\)") function, is currently unused and must be 0.

These functions return the number of bytes actually sent, or -1 if an error occurred. You can call [`Error()`](BNetEndpoint.md#BNetEndpoint_Error "Error\(\), ErrorStr\(\)") to get the specific error that occurred.

####  SetOption(), SetNonBlocking(), SetReuseAddr()

`int SetOption(int32 option,  
int32 level,  
const void* data,  
unsigned int dataSize);``int SetNonBlocking(bool enable = true);``int SetReuseAddr(bool enable = true);`

`SetOption()` lets you set any option for the `BNetEndpoint`. This provides access to the `setsockopt()` function of the underlying socket.

`SetNonBlocking()` controls whether I/O should block or not. If `enable` is `true`, the connection will be nonblocking. If `enable` is `false`, the connection will block on I/O calls until the transmission is completed.

`SetReuseAddr()` controls whether addresses should be reused or not. If `enable` is `true`, addresses will be reused. If `enable` is `false`, the connection won't reuse addresses.

These functions return 0 if successful; otherwise they return -1.

#### SetProtocol()

`status_t SetProtocol(int protocol);`

Sets the protocol type for the connection. Possible values for the protocol argument are `SOCK_STREAM` (to use the stream protocol) or `SOCK_DGRAM` (for datagram protocol).

Return Code| Description  
---|---  
`B_OK`| The protocol was set successfully.  
`B_ERROR`| An error occurred setting the protocol.  
  
#### SetTimeout()

`void SetTimeout(bigtime_t timeout);`

Sets the timeout for calls to [`Receive()`](BNetEndpoint.md#BNetEndpoint_Receive "Receive\(\), ReceiveFrom\(\)") and [`ReceiveFrom()`](BNetEndpoint.md#BNetEndpoint_ReceiveFrom). If blocking I/O is in use, and `timeout` microseconds pass, the function will abort with an error. By default, there's no timeout. You can specify that you want no timeout by specifying -1.

#### Socket()

`int Socket() const;`

Returns the actual socket used by the `BNetEndpoint` for data communications.

* * *

### Operators

#### = (assignment)

`BNetEndpoint& operator =(const BNetEndpoint& from);`

Copies the `BNetEndpoint` specified by `from` into the left-side object, thereby duplicating that object. If `from` is connected, the left-side object will duplicate and open the same connection.

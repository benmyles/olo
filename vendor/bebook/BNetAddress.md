<!-- Classes And Methods - The Network Kit - BNetAddress -->

## BNetAddress

| Derived From:| None  
---|---  
Mix-in Classes:| [`BArchivable`](BArchivable.md "BArchivable")  
Declared In:| `net/NetAddress.h`  
Library:| `libnetapi.so`  
Allocation:| Constructor only  
[Class Overview](BNetAddress_Overview.md "BNetAddress")

  * [Constructor and Destructor](BNetAddress.md#BNetAddress_ConstructorDestructor)
  * [Member Functions](BNetAddress.md#BNetAddress_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BNetAddress()

`BNetAddress(const char* hostname = NULL,  
unsigned short port = 0);` `BNetAddress(const struct sockaddr_in& sa);` `BNetAddress(in_addr addr,  
int port = 0);` `BNetAddress(uint32 addr,  
int port = 0);` `BNetAddress(const char* hostname,  
const char* protocol,  
const char* service);` `BNetAddress(BNetAddress& from);` `BNetAddress([`BMessage`](BMessage.md "BMessage")* archive);`

Sets up the `BNetAddress` object to refer to the specified address. The address can be specified in a number of ways:

  * By `hostname` and `port` number. For example, to connect to the HTTP port at www.be.com, you would specify "www.be.com" as the `hostname`, and 80 for the `port` number.

  * By sockaddr_in structure. This structure contains the network family, port number, and IP address that make up the address.

  * By IP address and port number. The IP address can be specified either using an in_addr, or by using a uint32. The IP address must be specified in network byte order. See the `htonl()` function.

  * By `hostname`, `protocol`, and `service` name. This causes the port to be looked up in the `/etc/services` file by matching the protocol (typically "tcp" or "udp") and service name (such as "http" or "ftp") against the entries in the file. See `getservbyname()` for details on the format of this file.

  * By copying an existing `BNetAddress`.

  * By unflattening an archived `BNetAddress` from a [`BMessage`](BMessage.md "BMessage").

After creating your `BNetAddress`, you must call [`InitCheck()`](BNetAddress.md#BNetAddress_InitCheck "InitCheck\(\)") to ensure that no errors occurred during setup. You can change the address later by calling [`SetTo()`](BNetAddress.md#BNetAddress_SetTo "SetTo\(\)").

#### ~BNetAddress()

`virtual ~BNetAddress();`

A typical destructor.

* * *

### Member Functions

#### InitCheck()

`status_t InitCheck() const;`

Returns a status_t indicating whether or not the object was successfully instantiated.

Return Code| Description  
---|---  
`B_OK`| The `BNetAddress` was constructed without error.  
`B_ERROR`| An error occurred during construction.  
  
#### GetAddr()

`status_t GetAddr(const char* hostname = NULL,  
unsigned short* port = NULL) const;` `status_t GetAddr(const struct sockaddr_in& sa) const;` `status_t GetAddr(in_addr& addr,  
unsigned short* port = NULL) const;`

Returns the address represented by the `BNetAddress` object in the format indicated by the form of the function used.

If you don't care about the hostname (in the first form), you can specify `NULL` for the `hostname` argument; if you don't care about the port number, you can specify `NULL` for the `port` argument.

Return Code| Description  
---|---  
`B_OK`| The address was returned successfully.  
`B_ERROR`| An error occurred fetching the address information.  
  
#### SetTo()

`status_t SetTo(const char* hostname,  
const char* protocol,  
const char* service);` `status_t SetTo(const char* hostname = NULL,  
unsigned short port = 0);` `status_t SetTo(const struct sockaddr_in& sa);` `status_t SetTo(in_addr addr,  
int port = 0);` `status_t SetTo(uint32 addr = INADDR_ANY,  
int port = 0);`

Sets the address represented by the `BNetAddress` object in the format indicated by the form of the function used. These formats are described in the discussion of the constructor.

Return Code| Description  
---|---  
`B_OK`| The address was set successfully.  
`B_ERROR`| An error occurred setting the address information.

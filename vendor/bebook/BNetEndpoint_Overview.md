<!-- System Overview - The Network Kit - BNetEndpoint -->

## BNetEndpoint

The [`BNetEndpoint`](BNetEndpoint.md "BNetEndpoint") class represents a network endpoint, which can send and receive data, establish network connections, bind to local addresses, and listen for and accept new connections.

Rather than replacing the existing network architecture, the [`BNetEndpoint`](BNetEndpoint.md "BNetEndpoint") class provides a C++ wrapper to the standard socket functions. All the same rules of usage apply, so be sure to review the BSD-like C socket function material.

* * *

### Archiving BNetEndpoints

[`BNetEndpoint`](BNetEndpoint.md "BNetEndpoint") objects are archivable. All attributes of the [`BNetEndpoint`](BNetEndpoint.md "BNetEndpoint") are preserved when archived. Upon reinstantiation, the object is duplicated precisely. This has interesting ramifications—if the [`BNetEndpoint`](BNetEndpoint.md "BNetEndpoint") is connected to a remote system when it's archived, the reinstantiated archive will also be connected to that system. You can actually archive active connections to restore them later.

Obviously, however, protocol-specific information won't be saved unless you add that data to the archive yourself. For example, if you archive an FTP connection, then restore the connection from the archive, the working directory or any ongoing downloads won't be restored automatically.

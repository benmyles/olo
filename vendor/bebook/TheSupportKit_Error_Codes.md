<!-- Classes And Methods - The Support Kit - Error Codes -->

## Error Codes

Declared in: `support/Errors.h`

Error codes are returned by various functions to indicate the success or to describe the failure of a requested operation.

All Be error constants except for `B_OK` (`B_NO_ERROR`) are negative integers; any function that returns an error code can thus be lazily tested for success or failure by the following:
    
    
    if ( funcCall() < B_NO_ERROR )
       /* failure*/
    else
       /* success*/
    

All constants (except `B_NO_ERROR` and `B_ERROR`) are less than or equal to the value of the `B_ERRORS_END` constant. If you want to define your own negative-valued error codes, you should begin with the value (`B_ERRORS_END` \+ 1) and work your way toward 0.

* * *

### POSIX Errors

The BeOS supports the POSIX error code constants (these constants start with the letter "E", as in `EBADF` or `ENOENT`). A number of Be-defined constants are synonyms for the POSIX constants; for example, the Be equivalent for `ENOENT` is `B_ENTRY_NOT_FOUND`.

Most of the General Error Codes, and all the File System Error Codes are covers for POSIX errors. The POSIX equivalents are listed where applicable.

The POSIX constants, and the Be synonyms, can be passed to the POSIX `strerror()` function. The function, defined in `posix/string.h`, returns a human-readable description of the error:
    
    
    char* strerror(int error_code)

* * *

### General Error Codes

Code| POSIX| Description  
---|---|---  
`B_NO_MEMORY`| `ENOMEM`| There isn't enough memory for the operation  
`B_IO_ERROR`| `EIO`| A general input/output error occurred  
`B_PERMISSION_DENIED`| `EACCESS`| Illegal access  
`B_BAD_INDEX`|  | The index is out of range  
`B_BAD_VALUE`| `EINVAL`| An illegal value was passed to the function  
`B_MISMATCHED_VALUES`|  | Conflicting values were passed to the function  
`B_BAD_TYPE`|  | An illegal argument type was named or passed  
`B_NAME_NOT_FOUND`|  | There's no match for the specified name.  
`B_NAME_IN_USE`|  | The requested (unique) name is already used  
`B_BUSY`| `EBUSY`| A device is busy, or a file is locked  
`B_NOT_ALLOWED`| `EPERM`| Operation not allowed  
`B_NO_INIT`|  | An object or structure isn't properly initialized  
`B_TIMED_OUT`| `ETIMEDOUT`| Time expired before the operation was finished  
`B_INTERRUPTED`| `EINTR`| A signal interrupted the operation  
`B_WOULD_BLOCK`| `EAGAIN`| But you don't want to block  
`B_WOULD_BLOCK`| `EWOULD`| BLOCK Same as the above  
`B_ERROR = -1`|  | A convenient catchall for general errors  
`B_NO_ERROR = 0`|  | Everything's OK  
`B_OK`|  | Same as B_NO_ERROR  
  
* * *

### File System Error Codes

Code| POSIX| Description  
---|---|---  
`B_FILE_ERROR`| `EBADF`| A file error occurred  
`B_FILE_EXISTS`| `EEXIST`| And you don't want to clobber it  
`B_ENTRY_NOT_FOUND`| `ENOENT`| The requested entry doesn't exist  
`B_NAME_TOO_LONG`| `ENAME_TOOLONG`| Leaf or pathname too long  
`B_NO_MORE_FDS`| `EMFILE`| No more file descriptors  
`B_NOT_A_DIRECTORY`| `ENOTDIR`| When it should have been  
`B_IS_A_DIRECTORY`| `EISDIR`| When it shouldn't be  
`B_DIRECTORY_NOT_EMPTY`| `ENOTEMPTY`| Attempt to delete a non-empty directory  
`B_DEVICE_FULL`| `ENOSPC`| Full disk  
`B_READ_ONLY_DEVICE`| `EROFS`| Write request on a read-only file system  
`B_CROSS_DEVICE_LINK`| `EXDEV`| Hard link across devices not allowed  
`B_LINK_LIMIT`| `ELOOP`| Nested links too deep  
`B_BUSTED_PIPE`| `EPIPE`| Pipe no longer functional.  
`B_UNSUPPORTED`|  | An unsupported operation was attempted.  
`B_PARTITION_TOO_SMALL`|  | A partition is too small for the attempted operation.  
  
* * *

### Application Kit Error Codes

Code| Description  
---|---  
`B_DUPLICATE_REPLY`| A reply message has already been sent  
`B_BAD_REPLY`| The reply message is inappropriate and can't be sent  
`B_BAD_HANDLER`| The designated message handler isn't valid  
`B_MESSAGE_TO_SELF`| A thread is trying to send a message to itself  
`B_ALREADY_RUNNING`| The application can't be launched again  
`B_LAUNCH_FAILED`| The attempt to launch the application failed  
`B_AMBIGUOUS_APP_LAUNCH`| Odd things happening in app land  
`B_UNKOWN_MIME_TYPE`| Application signature unknown  
`B_BAD_SCRIPT_SYNTAX`| Script syntax malformed  
`B_LAUNCH_FAILED_NO_RESOLVE_LINK`| Unable to resolve a link to the application to be launched  
`B_LAUNCH_FAILED_EXECUTABLE`| The attempt to launch the application failed because the application isn't a valid executable  
`B_LAUNCH_FAILED_APP_NOT_FOUND`| The attempt to launch the application failed because the application wasn't found  
`B_LAUNCH_FAILED_APP_IN_TRASH`| The attempt to launch the application failed because the application was in the trash  
`B_LAUNCH_FAILED_NO_PREFERRED_APP`| The attempt to launch the application failed because there is no preferred application for the document  
`B_LAUNCH_FAILED_FILES_APP_NOT_FOUND`| The attempt to launch the application failed because the document's application is missing  
`B_BAD_MIME_SNIFFER_RULE`| A MIME type sniffer rule is invalid  
  
* * *

### Kernel Kit Error Codes

Code| Description  
---|---  
`B_BAD_THREAD_ID`| Specified thread identifier (thread_id) is invalid  
`B_BAD_THREAD_STATE`| The thread is in the wrong state for the operation  
`B_NO_MORE_THREADS`| All thread identifiers are currently taken  
`B_BAD_TEAM_ID`| Specified team identifier (team_id) is invalid  
`B_NO_MORE_TEAMS`| All team identifiers are currently taken  
`B_BAD_PORT_ID`| Specified port identifier (port_id) is invalid  
`B_NO_MORE_PORTS`| All port identifiers have been taken  
`B_BAD_SEM_ID`| Semaphore identifier (sem_id) is invalid  
`B_NO_MORE_SEMS`| All semaphores are currently taken  
`B_BAD_IMAGE_ID`| Specified image identifier (image_id)is invalid  
`B_NOT_AN_EXECUTABLE`| An executable image was expected  
`B_BAD_ADDRESS`| Illegal address  
`B_MISSING_LIBRARY`| A library needed by an application is missing  
`B_MISSING_SYMBOL`| A symbol needed by an application is missing  
`B_DEBUGGER_ALREADY_INSTALLED`| An attempt was made to install the debugger when it was already installed.  
  
* * *

### Media Kit Error Codes

Code| Description  
---|---  
`B_MEDIA_SYSTEM_FAILURE`| The media system has failed spectacularly  
`B_MEDIA_BAD_NODE`| An invalid or nonexistent node was specified  
`B_MEDIA_NODE_BUSY`| The specified node was busy  
`B_MEDIA_BAD_FORMAT`| The specified media_format is invalid  
`B_MEDIA_TOO_MANY_NODES`| The media system has reached its limit of nodes  
`B_MEDIA_TOO_MANY_BUFFERS`| The media system has too many buffers  
`B_MEDIA_NODE_ALREADY_EXISTS`| Trying to create a node that already exists is a no-no  
`B_MEDIA_BUFFER_ALREADY_EXISTS`| Trying to create a buffer that already exists is a bad idea  
`B_MEDIA_CANNOT_SEEK`| A seek failure occurred  
`B_MEDIA_CANNOT_CHANGE_RUN_MODE`| Unable to change run mode  
`B_MEDIA_APP_ALREADY_REGISTERED`| An application has already registered with the media system  
`B_MEDIA_APP_NOT_REGISTERED`| An application hasn't registered with the media system  
`B_MEDIA_CANNOT_RECLAIM_BUFFERS`| Reclaiming of BBuffers failed  
`B_MEDIA_BUFFERS_NOT_RECLAIMED`| BBuffers weren't reclaimed  
`B_MEDIA_TIME_SOURCE_STOPPED`| The time source isn't running  
`B_MEDIA_TIME_SOURCE_BUSY`| The time source is busy  
`B_MEDIA_BAD_SOURCE`| The specified media_source is invalid  
`B_MEDIA_BAD_DESTINATION`| The specified media_destination is invalid  
`B_MEDIA_ALREADY_CONNECTED`| Can't connect nodes that are already connected  
`B_MEDIA_NOT_CONNECTED`| There's no connection in place  
`B_MEDIA_BAD_CLIP_FORMAT`| The clip format isn't valid  
`B_MEDIA_ADDON_FAILED`| An add-on failed  
`B_MEDIA_ADDON_DISABLED`| The add-on is disabled  
`B_MEDIA_CHANGE_IN_PROGRESS`| A change is in progress, preventing the requested action  
`B_MEDIA_STALE_CHANGE_COUNT`| The change count is stale  
`B_MEDIA_ADDON_RESTRICTED`| The add-on is restricted  
`B_MEDIA_NO_HANDLER`| There's no handler available meeting the specified needs  
`B_MEDIA_DUPLICATE_FORMAT`| The specified format already exists  
`B_MEDIA_REALTIME_DISABLED`| Real-time memory allocation is disabled  
`B_MEDIA_REALTIME_UNAVAILABLE`| Real-time memory allocation isn't available  
  
* * *

### Mail Error Codes

Code| Description  
---|---  
`B_MAIL_NO_DAEMON`| mail_daemon not running  
`B_MAIL_UNKNOWN_USER`| User unknown  
`B_MAIL_WRONG_PASSWORD`| Password doesn't match user  
`B_MAIL_UNKNOWN_HOST`| POP or SMTP host unrecognized  
`B_MAIL_ACCESS_ERROR`| Couldn't access host  
`B_MAIL_UNKNOWN_FIELD`| Unrecognized message field name  
`B_MAIL_NO_RECIPIENT`| Return to sender, address unknown  
`B_MAIL_INVALID_MAIL`| Invalid mail invalid mail invalid  
  
* * *

### Device Kit Error Codes

Code  
---  
`B_DEV_INVALID_IOCTL`  
`B_DEV_NO_MEMORY`  
`B_DEV_BAD_DRIVE_NUM`  
`B_DEV_NO_MEDIA`  
`B_DEV_UNREADABLE`  
`B_DEV_FORMAT_ERROR`  
`B_DEV_TIMEOUT`  
`B_DEV_RECALIBRATE_ERROR`  
`B_DEV_SEEK_ERROR`  
`B_DEV_ID_ERROR`  
`B_DEV_READ_ERROR`  
`B_DEV_WRITE_ERROR`  
`B_DEV_NOT_READY`  
`B_DEV_MEDIA_CHANGE_REQUESTED`  
`B_DEV_RESOURCE_CONFLICT`  
`B_DEV_CONFIGURATION_ERROR`  
`B_DEV_DISABLED_BY_USER`  
`B_DEV_DOOR_OPEN`

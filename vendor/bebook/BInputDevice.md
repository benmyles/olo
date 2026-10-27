<!-- Classes And Methods - The Input Server - BInputDevice -->

## BInputDevice

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/Input.h`  
Library:| `libbe.so`  
Allocation:| By the system only. See [`find_input_device()`](BInputDevice.md#find_input_device "find_input_device\(\), get_input_devices\(\)").  
[Class Overview](BInputDevice_Overview.md "BInputDevice")

  * [Constructor and Destructor](BInputDevice.md#BInputDevice_ConstructorDestructor)
  * [Member Functions](BInputDevice.md#BInputDevice_MemberFunctions)
  * [C Functions](BInputDevice.md#BInputDevice_Functions)

  
  
* * *

### Constructor and Destructor

#### BInputDevice()

`BInputDevice();`

The constructor is private. Use [`find_input_device()`](BInputDevice.md#find_input_device "find_input_device\(\), get_input_devices\(\)") or [`get_input_devices()`](BInputDevice.md#get_input_devices) to retrieve a `BInputDevice` instance.

#### ~BInputDevice()

`BInputDevice();`

Deletes the `BInputDevice` object. Deleting this object doesn't affect the device that it represents.

* * *

### Member Functions

#### Control()

`status_t Control(uint32 code,  
[`BMessage`](BMessage.md "BMessage")* message);` `status_t Control(input_device_type type,  
uint32 code,  
[`BMessage`](BMessage.md "BMessage")* message);`

Sends an input device control message to the object's input device or, in the static version, to all devices of the given type, where, type can be `B_POINTING_DEVICE`, `B_KEYBOARD_DEVICE`, or `B_UNDEFINED_DEVICE`. Input devices receive these messages in their `Control()` function.

The control message is described by the `code` value; it can be supplemented or refined by `message`. For example, `code` can indicate that a certain parameter should be set, and message can supply the requested value.

Note

![Note](./images/admonitions/Info_32.png)

In general, you only use this function to send custom messages to a (custom) device. You never use it to send messages to a Be-defined input device since the messages that these devices respond to are covered by Be-defined functions. See "Input Device Control Messages" for a list of the messages that the Be-defined devices respond to, the functions that cover them, and the German women who love them.

####  Name(), Type()

`const char* Name() const;``input_device_type Type() const;`

`Name()` returns a pointer to the input device's name. The name, which is set when the device is registered, is meant to be human-readable and appropriate for use as the label of a UI element (such as a menu field). Device names are not unique.

`Type()` returns the input device's type, one of `B_POINTING_DEVICE`, `B_KEYBOARD_DEVICE`, and `B_UNDEFINED_DEVICE`.

####  Start(), Stop(), IsRunning()

`status_t Start();` `static status_t Start(input_device_type type);`

`bool IsRunning() const;``static status_t Stop(input_device_type type);`

`Start()` tells the object's input device to start generating events; `Stop()` tells it to stop generating events. `IsRunning()` returns `true` if the device is currently generating events (i.e. if it has been started and hasn't been stopped).

The static versions of `Start()` and `Stop()` start and stop all devices of the given type.

Warning

![Warning](./images/admonitions/Stop_32.png)

The Input Server tells a device to start and stop without asking the device if the operation was successful. This means, for example, that `Start()` can return `B_OK` (and `IsRunning()` can return `true`) even if the device isn't really running. For the Be-provided devices this isn't an issue—starting and stopping always succeeds (as long as the device exists).

Return Code| Description  
---|---  
`B_OK`.| The device is now started (`Start()`) or stopped (`Stop()`)—even if the device was already started or stopped.  
`B_ERROR`.| The device couldn't be found.  
  
* * *

### C Functions

####  find_input_device(), get_input_devices()

`BInputDevice* find_input_device(const char * name);``status_t get_input_devices(BList * list);`

These functions get `BInputDevice` objects for you.

`find_input_device()` creates and hands you a `BInputDevice` object that represents the Input Server device registered as name. If name is invalid, the function returns `NULL`. The caller is responsible for deleting the object. Note that `find_input_device()` returns a new `BInputDevice` object for each (valid) call, even if you ask for the same device more than once.

`get_input_devices()` creates a new BInputDevice object for each registered device, and puts the objects in your list argument. list must already be allocated, and is automatically emptied by the function (even if the function fails). If the function succeeds, the caller owns the contents, and needs to delete the items in the list:
    
    
    #include <interface/Input.h>
    #include <support/List.h>
    
    ...
    
    static bool del_InputDevice( void *ptr )
    {
       if( ptr ) {
          BInputDevice *dev = (BInputDevice *)ptr;
          delete dev;
    
          return false;
       }
    
       return true;
    }
    
    ...
    
    void SomeFunc( void )
    {
       // Get a list of all input devices.
       BList list_o_devices;
    
       status_t retval = get_input_devices( &list_o_devices );
       if( retval != B_OK ) return;
    
       // Do something with the input devices.
       ...
    
       // Dispose of the device list.
       list_o_devices.DoForEach( del_InputDevice );
       list_o_devices.MakeEmpty();
    }

`get_input_devices()` returns:

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_ERROR`.| General failure.  
`B_BAD_PORT_ID`.| Couldn't talk to the Input Server.  
`B_TIMED_OUT`.| The Input Server is no longer on speaking terms with your application.  
`B_WOULD_BLOCK`.| More trouble communicating with the Input Server.  
  
#### watch_input_devices()

`status_t watch_input_devices(BMessenger target,  
bool start);`

Tells the Input Server to start or stop watching (as `start` is `true` or `false`) for changes to the set of registered devices. Change notifications are sent to `target`. The set of messages that the Server may send are listed in Input Server Messages.

Note

![Note](./images/admonitions/Info_32.png)

`watch_input_devices()` is not currently implemented.

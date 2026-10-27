<!-- Classes And Methods - The Input Server - BInputServerDevice -->

## BInputServerDevice

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `add-ons/input_server/InputServerDevice.h`  
Library:| The Input Server  
Allocation:| By the Input Server only  
[Class Overview](BInputServerDevice_Overview.md "BInputServerDevice")

  * [Constructor and Destructor](BInputServerDevice.md#BInputServerDevice_ConstructorDestructor)
  * [Hook Functions](BInputServerDevice.md#BInputServerDevice_HookFunctions)
  * [Member Functions](BInputServerDevice.md#BInputServerDevice_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BInputServerDevice()

`BInputServerDevice();`

Creates a new BInputServerDevice object. You can initialize your object—set initial values, spawn (but not necessarily resume; do that in [`Start()`](BInputServerDevice.md#BInputServerDevice_Start "Start\(\), Stop\(\)")) threads, open drivers, etc.—either here or in the [`InitCheck()`](BInputServerDevice.md#BInputServerDevice_InitCheck "InitCheck\(\)") function, which is called immediately after the constructor.

#### ~BInputServerDevice()

`BInputServerDevice();`

Deletes the `BInputServerDevice` object. The destructor is invoked by the Input Server only—you never delete a `BInputServerDevice` object from your own code. When the destructor is called, the object's devices will already be unregistered and [`Stop()`](BInputServerDevice.md#BInputServerDevice_Stop) will already have been called. If this object spawned its own threads or allocated memory on the heap, it must clean up after itself here.

* * *

### Hook Functions

#### Control()

`status_t Control(const char* name,  
voi* cookie,  
uint32 command,  
[`BMessage`](BMessage.md "BMessage")* message);`

The `Control()` hook function is invoked by the Input Server to send an input device control message or a Node Monitor message to this object. name and cookie are the readable name and pointer-to-whatever-you-want that you used when registering the device (with the [`RegisterDevices()`](BInputServerDevice.md#BInputServerDevice_RegisterDevices "RegisterDevices\(\), UnregisterDevices\(\)") function).

Warning

![Warning](./images/admonitions/Stop_32.png)

The function's return value is ignored.

##### Input Device Control Messages

An input device control message is sent when a downstream change needs to be propagated to an input device. For example, when the user resets the mouse speed (through the Mouse preference), a `B_MOUSE_SPEED_CHANGED` control message is sent to all objects that have registered a `B_POINTING_DEVICE` device (see [`RegisterDevices()`](BInputServerDevice.md#BInputServerDevice_RegisterDevices "RegisterDevices\(\), UnregisterDevices\(\)")). `name` and `cookie` identify the device that this message applies to. The control message itself is represented by the `command` constant, optionally supplemented by `message`.

See "Input Device Control Messages" for a list of the control messages that the BeOS defines, and instructions for how to respond to them. An application can send a custom control message through a [`BInputDevice`](BInputDevice.md "BInputDevice") object; see [`BInputDevice::Control()`](BInputDevice.md#BInputDevice_Control "Control\(\)") for details.

##### Node Monitor Messages

A Node Monitor message is sent if an entry is added to or removed from one of the device directories that the object is monitoring, as set through [`StartMonitoringDevice()`](BInputServerDevice.md#BInputServerDevice_StartMonitoringDevice "StartMonitoringDevice\(\), StopMonitoringDevice\(\)"). In this case, `name` and `cookie` are `NULL`, `command` is `B_NODE_MONITOR`, and `message` describes the file that was added or deleted. The `message`'s `opcode` field will be `B_ENTRY_CREATED` or `B_ENTRY_REMOVED` (or, potentially but nonsensically, `B_ENTRY_MOVED`). For instructions on how to read these messages, see "The Node Monitor" in the Storage Kit (or click on the opcode constants).

#### InitCheck()

`virtual status_t InitCheck();`

Invoked by the Input Server immediately after the object is constructed to test the validity of the initialization. If the object is properly initialized (i.e. all required resources are located or allocated), this function should return `B_OK`. [`Start()`](BInputServerDevice.md#BInputServerDevice_Start "Start\(\), Stop\(\)") will be invoked soon if you need to do any extra initialization. If the object returns non-`B_OK`, the object is deleted and the add-on is unloaded.

The default implementation returns `B_OK`.

####  Start(), Stop()

`virtual status_t Start(const char* name,  
void* cookie);``virtual status_t Stop(const char* name,  
void* cookie);`

`Start()` is invoked by the Input Server to tell the object that it can begin sending events for the registered device identified by the arguments. The values of the arguments are taken from the input_device_ref structure you used to register the device (see [`RegisterDevices()`](BInputServerDevice.md#BInputServerDevice_RegisterDevices "RegisterDevices\(\), UnregisterDevices\(\)")). If your object needs to resume a thread (spawned in the constructor, in [`InitCheck()`](BInputServerDevice.md#BInputServerDevice_InitCheck "InitCheck\(\)"), or here), this is the place to do it.

`Stop()` is invoked to tell the object to stop sending events for the registered device. The device is not unregistered—you can still receive [`Control()`](BInputServerDevice.md#BInputServerDevice_Control "Control\(\)") messages for the device while it's stopped. You should pause or kill any threads associated with the device (that were spawned by this object) from here.

The return value (for both of these functions) is ignored.

#### SystemShuttingDown()

`virtual status_t SystemShuttingDown() const;`

Tells the object that the Input Server is in the process of shutting down. Unless something interrupts the shutdown, this notification will be followed by a [`Stop()`](BInputServerDevice.md#BInputServerDevice_Stop) and delete, thus you don't have to do much from this function (other than note that the end is near).

Warning

![Warning](./images/admonitions/Stop_32.png)

The return value is ignored.

* * *

### Member Functions

#### EnqueueMessage()

`status_t EnqueueMessage([`BMessage`](BMessage.md "BMessage")* message);`

Sends an event message to the Input Server, which passes it through the input methods and input filters before sending it to the App Server. The message you create should be appropriate for the action you're trying to depict. For example, if the user presses a key, you should create and send a `B_KEY_DOWN` message. A list of the system-defined event messages that an input device is expected to create and send is given in "Input Device Event Messages".

Return Code| Description  
---|---  
`B_OK`.| The message was sent.  
Anything else.| The connection to the App Server has been broken—this isn't good, and you may want to check the [`is_computer_on_fire()`](TheKernelKit_SystemInfo.md#is_computer_on_fire "is_computer_on_fire\(\)") function found in the Kernel Kit.  
  
####  RegisterDevices(), UnregisterDevices()

`status_t RegisterDevices(input_device_ref ** devices);``status_t UnregisterDevices(input_device_ref ** devices);`

`RegisterDevices()` tells the Input Server that this object is responsible for the listed devices. This means that when a control message is sent back upstream, the message—which is tagged as being relevant for a specific device, or type of device—will be forwarded (through the [`Control()`](BInputServerDevice.md#BInputServerDevice_Control "Control\(\)") hook) to the responsible `BInputServerDevice` object(s). Typically, you initially register your devices as part of the constructor or [`InitCheck()`](BInputServerDevice.md#BInputServerDevice_InitCheck "InitCheck\(\)"). Registration is cumulative—each `RegisterDevices()` call adds to the object's current list of devices.

`UnregisterDevices()` tells the Input Server that this object is no longer responsible for the listed devices. The devices are automatically unregistered when your object is deleted.

`RegisterDevices()` invokes [`Start()`](BInputServerDevice.md#BInputServerDevice_Start "Start\(\), Stop\(\)") for each device in the devices list; `UnregisterDevices()` invokes [`Stop()`](BInputServerDevice.md#BInputServerDevice_Stop).

For both functions, the devices list must be `NULL`-terminated, and the caller retains ownership of the list and its contents.

Note that the BeOS currently only targets the device types when sending a [`Control()`](BInputServerDevice.md#BInputServerDevice_Control "Control\(\)") message. For example, let's say you've registered two pointing devices and a keyboard:
    
    
    status_t MyISDevice::InitCheck()
    {
    ...
       input_device_ref **devices =
          (input_device_ref **)malloc(sizeof(*input_device_ref * 4));
       input_device_ref mouse1 = {"Mouse 1", B_POINTING_DEVICE,
                                  (void *)this)};
       input_device_ref mouse2 = {"Mouse 2", B_POINTING_DEVICE,
                                  (void *)this)};
       input_device_ref keyboard = {"Keyboard", B_KEYBOARD_DEVICE,
                                  (void *)this)};
       devices[0] = &mouse1;
       devices[1] = &mouse2;
       devices[2] = &keyboard;
       devices[3] = NULL;
       RegisterDevices(devices);
       ...
    }

When the user fiddles with the Mouse preference (more specifically, if an application calls [`set_mouse_speed()`](TheInputServer_Functions.md#set_mouse_speed) et. al.), this object will receive two [`Control()`](BInputServerDevice.md#BInputServerDevice_Control "Control\(\)") messages: one targets "Mouse 1", and the other targets "Mouse 2". That's because the mouse and keyboard functions (as defined by the BeOS and as used by the system preferences) know which type of device to control, but they don't provide a means for more granular identification. If you need a UI that identifies specific devices, you have to create the UI yourself, and use a [`BInputDevice`](BInputDevice.md "BInputDevice") object to tune the control messages that are sent back upstream.

Warning

![Warning](./images/admonitions/Stop_32.png)

The functions don't let you un/register the same device definition twice, and `RegisterDevices()` won't register a device that doesn't have a name (although the name can be ""). However, the functions don't complain about violations of these conditions as long as at least one definition is properly formed.

Return Code| Description  
---|---  
`B_OK`.| At least one of the devices was registered.  
`B_ERROR`.| None of the devices were registered.  
  
####  StartMonitoringDevice(), StopMonitoringDevice()

`status_t StartMonitoringDevice(const char* deviceDir);``status_t StopMonitoringDevice(const char* deviceDir);`

These are convenient covers for the [Node Monitor](The_Node_Monitor.md "The Node Monitor")'s [`watch_node()`](The_Node_Monitor.md#watch_node "watch_node\(\)") and [`stop_watching()`](The_Node_Monitor.md#stop_watching "stop_watching\(\)") functions. You use them to watch for physical devices that are attached and detached, as indicated by changes to subdirectories of the system device directory (`/dev`).

`deviceDir` is the name of the device subdirectory that you want to watch. The `/dev/` root is automatically prepended; for example, if you want to watch for new ps2 mice, you would pass `input/mouse/ps2` as the `deviceDir` name. The Node Monitor is told to look for changes to the directory (`B_WATCH_DIRECTORY` opcode). When an entry is added or removed, this object receives a `B_NODE_MONITOR` message delivered to its [`Control()`](BInputServerDevice.md#BInputServerDevice_Control "Control\(\)") function.

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_ERROR`.| Unspecified failure.  
`B_NOT_A_DIRECTORY`.| You're trying to monitor a node that isn't a directory.  
`B_BAD_VALUE`.| `deviceDir` not found.

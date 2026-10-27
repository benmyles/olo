<!-- Classes And Methods - The Media Kit - BControllable -->

## BControllable

| Derived From:| [`BParameter`](BParameter.md "BParameter")  
---|---  
Mix-in Classes:| –  
Declared In:| `media/ParameterWeb.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BControllable_Overview.md "BControllable")

  * [Constructor and Destructor](BControllable.md#BControllable_ConstructorDestructor)
  * [Member Functions](BControllable.md#BControllable_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BControllable()

##### protected

`explicit BControllable();`

The `BControllable` constructor. You should override this in your derived class to create a [`BParameterWeb`](BParameterWeb.md "BParameterWeb") object, configure it to describe the available parameters, and call [`SetParameterWeb()`](BControllable.md#BControllable_SetParameterWeb "SetParameterWeb\(\), Web\(\)") with that object before returning.

* * *

### Member Functions

#### BroadcastChangedParameter()

##### protected

`status_t BroadcastChangedParameter(int32 parameterID);`

When the configuration of a specific parameter changes, you should call this function with the ID of the changed parameter so that clients know that they need to check with the node to determine the parameter's new configuration.

Note

![Note](./images/admonitions/Info_32.png)

The configuration of a parameter changes only when the range of possible values for the parameter changes. For example, if the parameter's value is a CD track number, the configuration would change (thus requiring a call to [`BroadcastChangedParameter()`](BControllable.md#BControllable_BroadcastChangedParameter "BroadcastChangedParameter\(\)")) if the user put in a different CD with a different number of tracks on it.

Return Code| Description  
---|---  
`B_OK`.| No errors.  
Other errors.| See [`BMessenger::SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)")  
  
#### BroadcastNewParameterValue()

##### protected

`status_t BroadcastNewParameterValue(bigtime_t when,  
int32 id,  
void* newValue,  
size_t valueSize);`

Call this function when a parameter value change takes effect and you want people that are interested in knowing about the change to stay in sync with you. Unlike [`BroadcastChangedParameter()`](BControllable.md#BControllable_BroadcastChangedParameter "BroadcastChangedParameter\(\)"), this function actually passes along the new value of the parameter.

The `when` argument indicates the performance time at which the change took effect. The `id` indicates the parameter ID of the parameter whose value changed. `newValue` is a pointer to the parameter's data, and `valueSize` defines the size of that data.

Return Code| Description  
---|---  
`B_OK`.| No error.  
Other errors.| An error occurred communicating with the Media Server, or with a node in the roster.  
  
####  GetParameterValue(), SetParameterValue()

##### protected

`virtual status_t GetParameterValue(int32 parameterID,  
bigtime_t* lastChangeTime,  
void* value,  
size_t* ioSize)  = 0;``virtual void SetParameterValue(int32 parameterID,  
bigtime_t changeTime,  
const void* value,  
size_t size)  = 0;`

You should implement `GetParameterValue()` to store the value of the parameter with the specified `parameterID` in the memory pointed to by `value`. The size_t value pointed to by `ioSize` specifies the size of the value buffer; prior to returning, your implementation of `GetParameterValue()` should change `ioSize` to the actual size of the returned data.

Also, you should set `lastChangeTime` to the time at which the control's value most recently changed.

`GetParameterValue()` should return `B_OK` when done, or an appropriate error code if something goes wrong.

Likewise, you should implement `SetParameterValue()` to change the value of the parameter; the `changeTime` argument is the performance time at which the change should occur; in other words, you may need to queue the request so it can be handled at the requested time. `value` points to the value to which the parameter should be set, and `size` is the number of bytes of data in the value.

Note

![Note](./images/admonitions/Info_32.png)

It's possible that a single parameter may have several channels of values, if that parameter is a multi-channel parameter. For example, if the parameter is a two-channel slider (such as a stereo gain control, where the left and right channels are controlled individually within a single parameter), the `value` argument would point to an array of two floats, and `size` would be 8 (`sizeof`(float) * 2).

#### HandleMessage()

`virtual status_t HandleMessage(int32 message,  
const void* data,  
size_t* size_t);`

When your node's service loop receives a message, in addition to passing it to [`BMediaNode`](BMediaNode.md "BMediaNode") and other superclasses of your node, you should also pass it to `HandleMessage()`. You should start at the most-derived class' implementation of `HandleMessage()` and work your way upward until `B_OK` is returned.

If it's a message intended for the [`BControllable`](BControllable.md "BControllable") interface, it'll be dispatched and `B_OK` will be returned; otherwise, `HandleMessage()` will return an error so you know to try something else.
    
    
    /* Message received */
    
    if ((BControllable::HandleMessage(message, data, size) != B_OK) &&
             (BMediaNode::HandleMessage(message, data, size) != B_OK)) {
       BMediaNode::HandleMessage(message, data, size);
    }

In this example, the `BControllable` implementation of `HandleMessage()` gets the first crack at handling the request. If it doesn't know what to do with the message, it's forwarded to [`BMediaNode`](BMediaNode.md "BMediaNode")'s implementation. If the message still isn't handled, it's then sent to [`BMediaNode::HandleBadMessage()`](BMediaNode.md#BMediaNode_HandleBadMessage "HandleBadMessage\(\)") to be dealt with.

Return Code| Description  
---|---  
`B_OK`.| The message was dispatched.  
Other errors.| Each message code may respond with various error codes.  
  
See also: "[About Multiple Virtual Inheritance](TheMediaKit_Overview_Introduction.md#TheMediaKit_AboutMultipleVirtualInheritance "About Multiple Virtual Inheritance")"

####  LockParameterWeb(), UnlockParameterWeb()

`bool LockParameterWeb();``void UnlockParameterWeb();`

`LockParameterWeb()` locks the web to prevent access to it while you're working on it, and `UnlockParameterWeb()` releases it when you're done. You should surround your accesses to the web with these calls:
    
    
    LockParameterWeb();
    Web()->MakeGroup("EqualizerControls");
    ...
    UnlockParameterWeb();

####  MakeParameterData(), ApplyParameterData()

##### protected

`status_t MakeParameterData(const int32* parameterList,  
int32 numParameters,  
void* buffer,  
size_t* ioSize);``status_t ApplyParameterData(const void* value,  
size_t ioSize);`

The `MakeParameterData()` utility function takes a list of parameter IDs from `parameterList` and calls [`GetParameterValue()`](BControllable.md#BControllable_GetParameterValue "GetParameterValue\(\), SetParameterValue\(\)") for each of them, storing the values in the specified `buffer` until the size specified in `ioSize` is filled, or all the parameters are read. The number of bytes of the buffer used will be returned in `ioSize`.

If your `BControllable` is also a [`BBufferConsumer`](BBufferConsumer.md "BBufferConsumer") that accepts `B_MEDIA_PARAMETERS` type data on some input, call `ApplyParameterData()` with `value` set to the result of [`BBuffer::Data()`](BBuffer.md#BBuffer_Data "Data\(\)") and `size` set to [`BBuffer::SizeUsed()`](BBuffer.md#BBuffer_SizeUsed "SizeUsed\(\), SetSizeUsed\(\)"). This function will then parse the parameter change requests in the buffer and dispatch them to your `SetParameterValue()` function to fulfill the requests.

This lets your node support easy automation of parameter information. Even more benefit can be obtained by also deriving from [`BBufferProducer`](BBufferProducer.md "BBufferProducer"), and providing an output for the `B_MEDIA_PARAMETERS` data format, so that changes can be recorded as they occur. This provides a mechanism for automating the parameters by recording a user's changes to them, then playing back the changes later.

Return Code| Description  
---|---  
`B_OK`.| No errors.  
`B_NO_MEMORY`.| The output buffer is too small.  
  
####  SetParameterWeb(), Web()

##### protected

`status_t SetParameterWeb(BParameterWeb* web);``BParameterWeb* SetParameterWeb();`

Your constructor should create a [`BParameterWeb`](BParameterWeb.md "BParameterWeb") object and call `SetParameterWeb()` with it as an argument. This will describe to the outside world what parameters are available and how they relate to each other; in other words, this describes your internal signal path, and how it can be manipulated.

If the `web` argument isn't `NULL`, and is different from the previously-established web for the `BControllable` node, a `B_MEDIA_WEB_CHANGED` message is sent to everyone watching for media notifications. See [`StartWatching()`](BMediaRoster.md#BMediaRoster_StartWatching "StartWatching\(\), StopWatching\(\)") for more information.

`SetParameterWeb()` will return `B_OK` if the web was set without errors; otherwise an error code will be returned.

The `Web()` function returns the [`BParameterWeb`](BParameterWeb.md "BParameterWeb") assigned to the `BControllable`.

#### StartControlPanel()

##### protected

`virtual status_t StartControlPanel(BMessenger* outMessenger);`

This hook function is called whenever a client application wants the node to present its own control panel user interface (so that the user can configure the node).

On return, `outMessenger` is a [`BMessenger`](BMessenger.md "BMessenger") that you can use to communicate with the control panel.

Because the add-on lives in the Media Server, and a problem in the user interface could bring down the entire system, it's recommended that the control panel run as its own team. This can be done easily by writing your node as both a Media Server add-on (by exporting `make_media_addon()`) and an application (by implementing `main()` and including `start_dyn.o` among the link libraries). Be sure you have the multi-launch application flags set on your add-on, or this won't work right.

Then your `StartControlPanel()` implementation can simply launch the add-on as an application, and if the user double-clicks the add-on, they'll be presented with the control panel. In addition, the user benefits by only having to install a single file for your add-on to work properly.

The first argv argument to your `main()` function will be a string of the format "node=%d" with the node ID in question as "%d".

Note

![Note](./images/admonitions/Info_32.png)

The above implementation suggestion (providing your control panel by launching the add-on as an application) is the default behavior of `StartControlPanel()`, so if that's how you implement your `BControllable`, you don't have to override `StartControlPanel()` at all. In this case, the returned [`BMessenger`](BMessenger.md "BMessenger") is for the control panel application, and not for a particular [`BWindow`](BWindow.md "BWindow") or [`BView`](BView.md "BView") therein.

Return Code| Description  
---|---  
`B_OK`.| No errors occurred.  
`B_ERROR`.| Node wasn't loaded from an add-on.  
`B_BAD_VALUE`.| An error occurred locating the image from which the node was loaded, or the add-on can't be launched as an application.  
`B_LAUNCH_FAILED`.| The control panel couldn't be launched for some other reason, such as insufficient memory.

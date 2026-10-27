<!-- Classes And Methods - The Device Kit - BJoystick -->

## BJoystick

| Derived From:| –  
---|---  
mix-in classes:| –  
Declared In:| `device/Joystick.h`  
Library:| `libdevice.so`  
Allocation:| Constructor only  
[Class Overview](BJoystick_Overview.md "BJoystick")

  * [Data Members](BJoystick.md#BJoystick_DataMembers)
  * [Constructor and Destructor](BJoystick.md#BJoystick_ConstructorDestructor)
  * [Member Functions](BJoystick.md#BJoystick_MemberFunctions)

  
  
* * *

### Data Members

Field| Description  
---|---  
bigtime_t `timestamp`| The time of the most recent update, as measured in microseconds from the beginning of 1970.  
int16 `horizontal`| The horizontal position of the joystick at the time of the last update. Values increase as the joystick moves from left to right.  
int16 `vertical`| The vertical position of the joystick at the time of the last update. Values decrease as the joystick moves forward and increase as it's pulled back.  
bool `button1`| `false` if the first button was pressed at the time of the last update, and `true` if not.  
bool `button2`| `false` if the second button was pressed at the time of the last update, and `true` if not.  
  
The `horizontal` and `vertical` data members record values read directly from the ports, values that simply digitize the analog output of the joysticks. This class makes no effort to translate the values to a standard scale or range. Values can range from 0 through 4,095, but joysticks typically don't use the full range and some don't register all values within the range that is used. The scale is not linear—identical increments in different parts of the range can reflect differing amounts of horizontal and vertical movement. The exact variance from linearity and the extent of the usable range are partly characteristics of the individual joystick and partly functions of the computer's hardware.

Note

![Note](./images/admonitions/Info_32.png)

Typically you won't use these data members if you're using enhanced mode; they're provided primarily for backward compatibility.

* * *

### Constructor and Destructor

#### BJoystick()

`BJoystick();`

Initializes the `BJoystick` object so that all values are set to 0. Before using the object, you must call [`Open()`](BJoystick.md#BJoystick_Open "Open\(\) , Close\(\)") to open a particular joystick port. For the object to register any meaningful values, you must call [`Update()`](BJoystick.md#BJoystick_Update "Update\(\)") to query the open port.

#### ~BJoystick()

`virtual ~BJoystick();`

Closes the port, if it was not closed already.

* * *

### Member Functions

####  CountAxes() , GetAxisValues()

##### Enhanced Mode Only

`int32 CountAxes();``status_t GetAxisValues(int16* outValues,  
int32 forStick = 0);`

`CountAxes()` returns the number of axes on the opened game port.

`GetAxisValues()` fills the array pointed to by `outValues` with the values of all the axes connected to the specified joystick. The `forStick` parameter lets you choose which joystick on the game port to read the values from (the default is to read from the first stick on the port).

The returned values range from -32,768 to 32,767.

The array `outValues` must be large enough to contain the values for all the axes. You can ensure that this is the case by using the following code:
    
    
    int16* axes;
    
    axes = (int16*) malloc(sizeof(int16) * stick->CountAxes());
    stick->GetAxisValues(axes);

Return Code| Description  
---|---  
`B_OK`.| The name was returned successfully.  
`B_BAD_VALUE`.| The joystick specified by `forStick` doesn't exist.  
  
####  CountButtons() , GetButtonValues()

##### Enhanced Mode Only

`int32 CountButtons();``uint32 ButtonValues();`

`CountButtons()` returns the number of buttons on the opened game port.

`ButtonValues()` returns a 32-bit number in which each bit represents the state of one button. The `forStick` parameter lets you choose which joystick on the game port to read the values from (the default is to read from the first stick on the port). You can deterimine if a particular button is down by using the following code:
    
    
    uint32 buttonValues = stick->ButtonValues();
    
    if (buttonValues & (1 << whichButton)) {
       /* button number whichButton is pressed */
    }

####  CountDevices() , GetDeviceName()

`int32 CountDevices();``status_t GetDeviceName(int32 index,  
char* outName,  
size_t bufSize = B_OS_NAME_LENGTH);`

`CountDevices()` returns the number of game ports on the computer.

`GetDeviceName()` returns the name of the device specified by the given `index`. The buffer pointed to by `outName` is filled with the device name; `bufSize` indicates the size of the buffer.

The names returned by `GetDeviceName()` can be passed into the [`Open()`](BJoystick.md#BJoystick_Open "Open\(\) , Close\(\)") function to open a device.

Note

![Note](./images/admonitions/Info_32.png)

The `BJoystick` doesn't need to have an open device before you use these functions; in fact, your application will typically use these to provide user interface allowing the user to choose the joystick device they'd like to use.

Return Code| Description  
---|---  
`B_OK`.| The name was returned successfully.  
`B_BAD_INDEX`.| The specified `index` is invalid.  
`B_NAME_TOO_LONG`.| The device name is too long for the buffer.  
  
####  CountHats() , GetHatValues()

##### Enhanced Mode Only

`int32 CountHats();``status_t GetHatValues(int8* outHats,  
int32 forStick = 0);`

`CountHats()` returns the number of hats on the opened game port.

`GetHatValues()` fills the array pointed to by `outHats` with the values of all the hats connected to the specified joystick. The `forStick` parameter lets you choose which joystick on the game port to read the values from (the default is to read from the first stick on the port).

The return value means the following:

Value| Description  
---|---  
0| Centered  
1| Up  
2| Up and right  
3| Right  
4| Down and right  
5| Down  
6| Down and left  
7| Left  
8| Up and left  
  
The array `outHats` must be large enough to contain the values for all the hats. You can ensure that this is the case by using the following code:
    
    
    int8* hats;
    
    hats = (int8*) malloc(sizeof(int8) * stick->CountAxes());
    stick->GetHatValues(hats);

Return Code| Description  
---|---  
`B_OK`.| The name was returned successfully.  
`B_BAD_VALUE`.| The joystick specified by `forStick` doesn't exist.  
  
#### CountSticks()

##### Enhanced Mode Only

`int32 CountSticks();`

Returns the number of joysticks connected to the opened game port.

####  GetAxisNameAt() , GetHatNameAt() , GetButtonNameAt()

##### Enhanced Mode Only

`status_t GetAxisNameAt(int32 index,  
[`BString`](BString.md "BString")* outName);``status_t GetHatNameAt(int32 index,  
[`BString`](BString.md "BString")* outName);``status_t GetButtonNameAt(int32 index,  
[`BString`](BString.md "BString")* outName);`

Returns the name of the control specified by the given `index`. The [`BString`](BString.md "BString") object pointed to by `outName` is set to the control's name.

`GetAxisNameAt()` returns the specified axis' name, `GetHatNameAt()` returns the specified hat's name, and `GetButtonNameAt()` returns the specified button's name.

Return Code| Description  
---|---  
`B_BAD_INDEX`.| The name was returned successfully.  
`B_BAD_INDEX`.| The specified `index` is invalid.  
  
####  GetControllerModule(), GetControllerName()

##### Enhanced Mode Only

`status_t GetControllerModule([`BString`](BString.md "BString")* outName);``status_t GetControllerName([`BString`](BString.md "BString")* outName);`

`GetControllerModule()` returns the name of the joystick module that represents the opened joystick device. If the device isn't in enhanced mode, this always returns "Legacy".

`GetControllerName()` returns the name of the joystick that's been configured for the opened device. This is the same string that appears in the Joysticks preference application. The returned string is always "2-axis" if the device isn't in enhanced mode.

Return Code| Description  
---|---  
`B_OK`.| The name was returned successfully.  
`B_BAD_INDEX`.| The specified `index` is invalid.  
  
#### EnterEnhancedMode()

`bool EnterEnhancedMode(const entry_ref* ref = NULL);`

Switches the device into enhanced mode. If `ref` isn't `NULL`, it's treated as a reference to a joystick description file (such as those in `/boot/beos/etc/joysticks`). If `ref` is `NULL`, the currently-configured joystick settings (as per the Joysticks preference application) are used.

If enhanced mode is entered successfully (or the device is already in enhanced mode), `true` is returned. Otherwise, `EnterEnhancedMode()` returns `false`.

####  IsCalibrationEnabled() , EnableCalibration()

##### Enhanced Mode Only

`bool IsCalibrationEnabled();``status_t EnableCalibration(bool calibrates = true);`

`IsCalibrationEnabled()` returns `true` if axis values returned by the joystick will be calibrated automatically into the range -32,768 to 32,767, or `false` if the raw values will be returned.

`EnableCalibration()` enables or disables automatic calibration for the joystick's axes. Specify a value of `true` for calibrates to enable calibration; otherwise, specify `false`.

Note

![Note](./images/admonitions/Info_32.png)

The Joysticks preference application lets the user calibrate the joystick. Calibration is enabled by default.

Return Code| Description  
---|---  
`B_OK`.| The name was returned successfully.  
`B_NO_INIT`.| The device isn't in enhanced mode.  
  
####  Open() , Close()

`status_t Open(const char* devName);` `status_t Open(const char* devName,  
bool enterEnhanced);`

`void Close();`

These functions open the joystick port specified by `devName` and close it again. The `devName` should be the name of a game port device. The easiest way to determine valid device names is by using the [`CountDevices()`](BJoystick.md#BJoystick_CountDevices "CountDevices\(\) , GetDeviceName\(\)") and [`GetDeviceName()`](BJoystick.md#BJoystick_GetDeviceName) functions.

If it's able to open the port, `Open()` returns a positive integer. If unable or if the `devName` isn't valid, it returns `B_ERROR`. If the `devName` port is already open, `Open()` tries to close it first, then open it again.

By default, `Open()` opens the device in enhanced mode. If you want to use the old, unenhanced mode, you can use the second form of `Open()` to specify whether or not you want to use enhanced mode; set `enterEnhanced` to `true` to use enhanced mode; specify `false` if you don't want enhanced mode.

Note

![Note](./images/admonitions/Info_32.png)

Even in enhanced mode, the classic `BJoystick` data members are valid (for compatibility with older applications). However, they only give you access to the first two axes and buttons of the joystick.

To be able to obtain joystick data, a `BJoystick` object must have a port open.

#### SetMaxLatency()

`status_t SetMaxLatency(bigtime_t maxLatency);`

Specifies the maximum latency to allow when accessing the joystick. Returns `B_OK` if the change is applied successfully, otherwise returns an error code.

#### Update()

`status_t Update();`

Updates the data members of the object so that they reflect the current state of the joystick. An application would typically call `Update()` periodically to poll the condition of the device, then read the values of the data members, or—if in enhanced mode—call the various functions for obtaining joystick state information.

This function returns `B_ERROR` if the `BJoystick` object doesn't have a port open, and `B_OK` if it does.

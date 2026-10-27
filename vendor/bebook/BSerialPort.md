<!-- Classes And Methods - The Device Kit - BSerialPort -->

## BSerialPort

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `device/SerialPort.h`  
Library:| `libdevice.so`  
Allocation:| –  
[Class Overview](BSerialPort_Overview.md "BSerialPort")

  * [Constructor and Destructor](BSerialPort.md#BSerialPort_ConstructorDestructor)
  * [Member Functions](BSerialPort.md#BSerialPort_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BSerialPort()

`BSerialPort();`

Initializes the `BSerialPort` object to the following default values:

  * Hardware flow control (see [`SetFlowControl()`](BSerialPort.md#BSerialPort_SetFlowControl "SetFlowControl\(\), FlowControl\(\)"))

  * A data rate of 19,200 bits per second (see [`SetDataRate()`](BSerialPort.md#BSerialPort_SetDataRate "SetDataRate\(\), DataRate\(\), data_rate"))

  * A serial unit with 8 bits of data, 1 stop bit, and no parity (see [`SetDataBits()`](BSerialPort.md#BSerialPort_SetDataBits "SetDataBits\(\), SetStopBits\(\), SetParityMode\(\), DataBits\(\), StopBits\(\), ParityMode\(\), data_bits, stop_bits, parity_mode"))

  * Blocking with no time limit—an infinite timeout—for reading data (see [`Read()`](BSerialPort.md#BSerialPort_Read "Read\(\) , SetBlocking\(\) , SetTimeout\(\)"))

The new object doesn't represent any particular serial port. After construction, it's necessary to open one of the ports by name.

The type of flow control must be decided before a port is opened. But the other default settings listed above can be changed before or after opening a port.

See also: [`Open()`](BSerialPort.md#BSerialPort_Open "Open\(\), Close\(\)")

#### ~BSerialPort()

`virtual ~BSerialPort();`

Makes sure the port is closed before the object is destroyed.

* * *

### Member Functions

#### ClearInput(), ClearOutput()

`void ClearInput();``void ClearOutput();`

These functions empty the serial port driver's input and output buffers, so that the contents of the input buffer won't be read (by the [`Read()`](BSerialPort.md#BSerialPort_Read "Read\(\) , SetBlocking\(\) , SetTimeout\(\)") function) and the contents of the output buffer (after having been written by [`Write()`](BSerialPort.md#BSerialPort_Write "Write\(\)")) won't be transmitted over the connection.

The buffers are cleared automatically when a port is opened.

####  CountDevices() , GetDeviceName()

`int32 CountDevices();``status_t GetDeviceName(int32 index,  
char* outName,  
size_t bufSize = B_OS_NAME_LENGTH);`

`CountDevices()` returns the number of serial ports on the computer.

`GetDeviceName()` returns the name of the device specified by the given `index`. The buffer pointed to by `outName` is filled with the device name; `bufSize` indicates the size of the buffer.

The names returned by `GetDeviceName()` can be passed into the [`Open()`](BSerialPort.md#BSerialPort_Open "Open\(\), Close\(\)") function to open a device.

Return Code| Description  
---|---  
`B_OK`.| The name was returned successfully.  
`B_BAD_INDEX`.| The specified `index` is invalid.  
`B_NAME_TOO_LONG`.| The device name is too long for the buffer.  
  
#### IsCTS()

`bool IsCTS();`

Returns `true` if the Clear to Send (CTS) pin is asserted, and `false` if not.

#### IsDCD()

`bool IsDCD();`

Returns `true` if the Data Carrier Detect (DCD) pin is asserted, and `false` if not.

#### IsDSR()

`bool IsDSR();`

Returns `true` if the Data Set Ready (DSR) pin is asserted, and `false` if not.

#### IsRI()

`bool IsRI();`

Returns `true` if the Ring Indicator (RI) pin is asserted, and `false` if not.

####  Open(), Close()

`status_t Open(const char* name);``void Close();`

These functions open the `name` serial port and close it again. To get a serial port name, use the `GetDeviceName()` function

To be able to read and write data, the `BSerialPort` object must have a port open. It can open first one port and then another, but it can have no more than one open at a time. If it already has a port open when `Open()` is called, that port is closed before an attempt is made to open the `name` port. (Thus, both `Open()` and `Close()` close the currently open port.)

`Open()` can't open the `name` port if some other entity already has it open. (If the `BSerialPort` itself has `name` open, `Open()` first closes it, then opens it again.)

When a serial port is opened, its input and output buffers are emptied and the Data Terminal Ready (DTR) pin is asserted.

Return Code| Description  
---|---  
positive integers (not 0).| Success.  
`B_PERMISSION_DENIED`.| The port is already open.  
`B_ERROR`.| The port couldn't be opened for some other reason.  
  
####  Read() , SetBlocking() , SetTimeout()

`ssize_t Read(void* buffer,  
size_t maxBytes);``void SetBlocking(bool shouldBlock);``status_t SetTimeout(bigtime_t timeout);`

`Read()` takes incoming data from the serial port driver and places it in the data `buffer` provided. In no case will it read more than `maxBytes`—a value that should reflect the capacity of the buffer. The input buffer of the driver, from which `Read()` takes the data, holds a maximum of 2,024 bytes (2048 on Mac hardware). This function fails if the `BSerialPort` object doesn't have a port open.

The number of bytes that `Read()` will read before returning depends not only on `maxBytes`, but also on the `shouldBlock` flag and the `timeout` set by the other two functions.

  * `SetBlocking()` determines whether `Read()` should block and wait for `maxBytes` of data to arrive at the serial port if that number isn't already available to be read. If the `shouldBlock` flag is `true`, `Read()` will block. However, if `shouldBlock` is `false`, `Read()` will take however many bytes are waiting to be read, up to the maximum asked for, then return immediately. If no data is waiting at the serial port, it returns without reading anything.

  * The default `shouldBlock` setting is `true`.

  * `SetTimeout()` sets a time limit on how long `Read()` will block while waiting for data to arrive at the port's input buffer. The timeout is relevant to `Read()` only if the `shouldBlock` flag is `true`. However, the time limit also applies to the [`WaitForInput()`](BSerialPort.md#BSerialPort_WaitForInput "WaitForInput\(\)") function, which always blocks, regardless of the `shouldBlock` setting.

  * There is no time limit if the timeout is set to `B_INFINITE_TIMEOUT`—`Read()` and [`WaitForInput()`](BSerialPort.md#BSerialPort_WaitForInput "WaitForInput\(\)") will block forever. Otherwise, the timeout is expressed in microseconds and can range from a minimum of 100,000 (0.1 second) through a maximum of 25,500,000 (25.5 seconds); differences less than 100,000 microseconds are not recognized; they're rounded to the nearest tenth of a second.

  * The default timeout is `B_INFINITE_TIMEOUT`.

`Read()` returns…

Return Code| Description  
---|---  
non-negative integer.| Success; the value is the number of bytes that were read.  
`B_INTERRUPTED`.| The operation was interrupted by a signal.  
`B_FILE_ERROR`.| The `BSerialPort` doesn't have a port open.  
  
`SetTimeout()` returns…

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_BAD_VALUE`.| Out of range value passed to `SetTimeout()`.  
  
(Note that it's not considered an error if a timeout expires.)

####  SetDataBits(), SetStopBits(), SetParityMode(), DataBits(), StopBits(), ParityMode(), data_bits, stop_bits, parity_mode

`void SetDataBits(data_bits count);``void SetStopBits(stop_bits count);``void SetParityMode(parity_mode mode);``data_bits DataBits();``stop_bits StopBits();``parity_mode ParityMode();`
    
    
    typedef enum { B_DATA_BITS_7, B_DATA_BITS_8 } data_bits
    
    
    typedef enum { B_STOP_BITS_1, B_STOP_BITS_2 } stop_bits
    
    
    typedef enum { B_EVEN_PARITY, B_ODD_PARITY, B_NO_PARITY } parity_mode

These functions set and return characteristics of the serial unit used to send and receive data.

  * `SetDataBits()` sets the number of bits of data in each unit; the default is `B_DATA_BITS_8`.

  * `SetStopBits()` sets the number of stop bits in each unit; the default is `B_STOP_BITS_2`.

  * `SetParityMode()` sets whether the serial unit contains a parity bit and, if so, the type of parity used; the default is `B_NO_PARITY`.

####  SetDataRate(), DataRate(), data_rate

`status_t SetDataRate(data_rate bitsPerSecond);``data_rate DataRate();`
    
    
    typedef enum {
        B_0_BPS,     B_50_BPS,    B_75_BPS,    B_110_BPS,    B_134_BPS,
        B_150_BPS,   B_200_BPS,   B_300_BPS,   B_600_BPS,    B_1200_BPS,
        B_1800_BPS,  B_2400_BPS,  B_4800_BPS,  B_9600_BPS,   B_19200_BPS,
        B_31250_BPS, B_38400_BPS, B_57600_BPS, B_115200_BPS, B_230400_BPS
    } data_rate

These functions set and return the rate (in bits per second) at which data is both transmitted and received.

The default data rate is `B_19200_BPS`. If the rate is set to 0 (`B_0_BPS`), data will be sent and received at an indeterminate number of bits per second.

`SetDataRate()` returns…

Return Code| Description  
---|---  
`B_OK`.| The rate was successfully set.  
`B_NO_INIT`.| The `BSerialPort` object doesn't have a port open.  
`B_ERROR`.| The data rate couldn't be set for any other reason.  
  
#### SetDTR()

`status_t SetDTR(bool pinAsserted);`

Asserts the Data Terminal Ready (DTR) pin if the `pinAsserted` flag is `true`, and de-asserts it if the flag is `false`. The function should always return `B_OK`.

####  SetFlowControl(), FlowControl()

`void SetFlowControl(uint32 mask);``uint32 FlowControl();`

These functions set and return the type of flow control the driver should use. There are four possibilities:

Constant| Description  
---|---  
`B_SOFTWARE_CONTROL`| Control is maintained through XON and XOFF characters inserted into the data stream.  
`B_HARDWARE_CONTROL`| Control is maintained through the Clear to Send (CTS) and Request to Send (RTS) pins.  
`B_SOFTWARE_CONTROL` \+ `B_HARDWARE_CONTROL`| Both of the above.  
0 (zero)| No control.  
  
`SetFlowControl()` should be called before a specific serial port is opened. You can't change the type of flow control the driver uses in midstream.

#### SetRTS()

`status_t SetRTS(bool pinAsserted);`

Asserts the Request to Send (RTS) pin if the `pinAsserted` flag is `true`, and de-asserts it if the flag is `false`. The function always returns `B_OK`.

#### WaitForInput()

`ssize_t WaitForInput();`

Waits for input data to arrive at the serial port and returns the number of bytes available to be read. If data is already waiting, the function returns immediately.

This function doesn't respect the flag set by [`SetBlocking()`](BSerialPort.md#BSerialPort_SetBlocking); it blocks even if blocking is turned off for the [`Read()`](BSerialPort.md#BSerialPort_Read "Read\(\) , SetBlocking\(\) , SetTimeout\(\)") function. However, it does respect the timeout set by [`SetTimeout()`](BSerialPort.md#BSerialPort_SetTimeout). If the timeout expires before input data arrives at the serial port, it returns 0.

#### Write()

`ssize_t Write(const void* data,  
size_t numBytes);`

Writes up to `numBytes` of data to the serial port's output buffer. This function will be successful in writing the data only if the `BSerialPort` object has a port open. The output buffer holds a maximum of 512 bytes (1024 on Mac hardware).

Return Code| Description  
---|---  
non-negative integer.| Success; the value is the number of bytes that were written.  
`B_INTERRUPTED`.| The operation was interrupted by a signal.  
`B_FILE_ERROR`.| The `BSerialPort` doesn't have a port open.

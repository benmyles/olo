<!-- System Overview - The Device Kit - BSerialPort -->

## BSerialPort  
  
A [`BSerialPort`](BSerialPort.md "BSerialPort") object represents an RS-232 serial connection to the computer. Through [`BSerialPort`](BSerialPort.md "BSerialPort") functions, you can read data received at a serial ports and write data over the connection. You can also configure the connection—for example, set the number of data and stop bits, determine the rate at which data is sent and received, and select the type of flow control (hardware or software) that should be used.

To read and write data, a [`BSerialPort`](BSerialPort.md "BSerialPort") object must first open one of the serial ports by name. To find the names of all the serial ports on the computer, use the [`CountDevices()`](BSerialPort.md#BSerialPort_CountDevices "CountDevices\(\) , GetDeviceName\(\)") and [`GetDeviceName()`](BSerialPort.md#BSerialPort_GetDeviceName) functions:
    
    
    BSerialPort serial;
    char devName[B_OS_NAME_LENGTH];
    int32 n = 0;
    
    for (int32 n = serial.CountDevices() - 1; n >= 0; n--) {
       serial.GetDeviceName(n, devName);
    
       if ( serial.Open(devName) > 0 )
          ....
    }

The [`BSerialPort`](BSerialPort.md "BSerialPort") object communicates with the driver for the port it has open. The driver maintains an input buffer to collect incoming data and a smaller output buffer to hold outgoing data. When the object reads and writes data, it reads from and writes to these buffers.

The serial port drivers, and therefore [`BSerialPort`](BSerialPort.md "BSerialPort") objects, send and receive data asynchronously only.

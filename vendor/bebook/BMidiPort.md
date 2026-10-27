<!-- Classes And Methods - The Midi Kit - BMidiPort -->

## BMidiPort

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `midi/MidiPort.h`  
Library:| `libmidi.so`  
Allocation:| –  
[Class Overview](BMidiPort_Overview.md "BMidiPort")

  * [Constructor and Destructor](BMidiPort.md#BMidiPort_ConstructorDestructor)
  * [Member Functions](BMidiPort.md#BMidiPort_MemberFunctions)
  * [MIDI Hook Functions](BMidiPort.md#BMidiPort_MIDIHookFunctions)

  
  
* * *

### Constructor and Destructor

#### BMidiPort()

`BMidiPort(const char* name = NULL);`

Creates a new `BMidiPort` object and opens it on the named port. If no `name` is given, the object remains unopened until [`Open()`](BMidiPort.md#BMidiPort_Open "Open\(\)") is called.

#### ~BMidiPort()

`virtual ~BMidiPort();`

Closes the connections to the MIDI ports.

* * *

### Member Functions

#### AllNotesOff()

`bool AllNotesOff(bool controlMsgOnly,  
uint32 time = B_NOW);`

Commands the `BMidiPort` object to issue an All Notes Off MIDI message to the MIDI-Out port. If `controlOnly` is `true`, only the All Notes Off message is sent. If it's `false`, a Note Off message is also sent for every key number on every channel.

#### Close()

`void Close();`

Closes the object's MIDI port. The port should have been previously opened through a call to [`Open()`](BMidiPort.md#BMidiPort_Open "Open\(\)").

####  CountDevices(), GetDeviceName()

`int32 CountDevices();``status_t GetDeviceName(int32 n,  
char* name,  
size_t bufSize = B_OS_NAME_LENGTH);`

These two function work together to let you retrieve the names of all MIDI ports. `CountDevices()` returns the number of MIDI ports that are supported by the machine. `GetDeviceName()` returns, in `name`, the name of the `n`'th MIDI port. `bufSize` is the length of the `name` buffer, in bytes. It needn't be longer than `B_OS_NAME_LENGTH` (defined in `OS.h`).

`GetDeviceName()` returns…

Return Code| Description  
---|---  
`B_OK`| Success.  
`B_BAD_VALUE`| `n` is out of range.  
`B_NAME_TOO_LONG`| The device name length is greater than `bufSize`.  
  
#### InitCheck()

`status_t InitCheck() const;`

Returns the status of the previous port-opening call. This function is provided primarily so you can get the status after opening the port through the constructor.

Return Code| Description  
---|---  
`B_OK`| The port was successfully opened.  
POSIX errors| The open was thwarted.  
  
#### Open()

`status_t Open(const char* name);`

Opens the MIDI port identified by `name`, so the object can read and write MIDI data. Use the [`GetDeviceName()`](BMidiPort.md#BMidiPort_GetDeviceName) function to get the names of the MIDI ports. The object isn't given exclusive access to the ports that it has opened—other `BMidiPort` objects, potentially from other applications, can open the same MIDI ports. When you're finished with the ports, you should close them through a (single) call to [`Close()`](BMidiPort.md#BMidiPort_Close "Close\(\)").

The MIDI-Out connection is active from the moment the object is opened: Messages that arrive through the MIDI hook functions are automatically sent to the MIDI-Out port. To begin reading from the MIDI-In port, you have to invoke the object's [`Start()`](BMidiPort.md#BMidiPort_Start "Start\(\), Stop\(\)") function.

The function returns `B_OK` if the port was successfully opened.

####  Start(), Stop()

`virtual status_t Start();``virtual void Stop();`

`Start()` tells the object to begin listening to MIDI-In. For each MIDI message that it hears, the object calls the appropriate spray function.

`Stop()` tells the object to stop listening to MIDI-In.

Neither of these functions affects the MIDI-Out side of the port.

* * *

### MIDI Hook Functions

The `BMidiPort` class implements the MIDI hook functions to send MIDI data to the MIDI-Out side of the port. For the syntax of the MIDI hook functions, see the [`BMidi`](BMidi.md "BMidi") class.

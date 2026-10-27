<!-- System Overview - The Midi Kit - BMidiPort -->

## BMidiPort

A [`BMidiPort`](BMidiPort.md "BMidiPort") object reads and writes MIDI data through a MIDI hardware port. A MIDI hardware port has an input side (MIDI-In) and an output side (MIDI-Out); you can use a single [`BMidiPort`](BMidiPort.md "BMidiPort") object to communicate with both sides. Also, you can create and use any number of [`BMidiPort`](BMidiPort.md "BMidiPort") objects in your application—multiple [`BMidiPort`](BMidiPort.md "BMidiPort") objects can open and use the same hardware port at the same time.

You identify a MIDI port by name, passing it to the [`BMidiPort`](BMidiPort.md "BMidiPort") constructor or [`Open()`](BMidiPort.md#BMidiPort_Open "Open\(\)") function. Use the [`GetDeviceName()`](BMidiPort.md#BMidiPort_GetDeviceName) function to retrieve the names of the MIDI ports. The ports are closed through [`Close()`](BMidiPort.md#BMidiPort_Close "Close\(\)"); they're automatically closed when the [`BMidiPort`](BMidiPort.md "BMidiPort") object is destroyed.

On a Macintosh, there are two midi ports:

  * "midi1" is the modem port.

  * "midi2" is the printer port.

On Intel hardware…

A [`BMidiPort`](BMidiPort.md "BMidiPort") object can only open one port at a time.

* * *

### Running and the MIDI Hook Functions

  * Running a [`BMidiPort`](BMidiPort.md "BMidiPort") corresponds to MIDI-In: When you tell a [`BMidiPort`](BMidiPort.md "BMidiPort") to run (through the [`Start()`](BMidiPort.md#BMidiPort_Start "Start\(\), Stop\(\)") function) the object begins reading from MIDI-In and spraying the MIDI messages to its connected objects.

  * On the other side, the MIDI hook functions ([`NoteOn()`](BMidi.md#BMidi_NoteOn "NoteOn\(\), SprayNoteOn\(\)"), [`NoteOff()`](BMidi.md#BMidi_NoteOff "NoteOff\(\), SprayNoteOff\(\)"), etc.) send MIDI messages to MIDI-Out.

It's possible to use the same [`BMidiPort`](BMidiPort.md "BMidiPort") object to accept data from MIDI-In and broadcast different data to MIDI-Out. You can even connect a [`BMidiPort`](BMidiPort.md "BMidiPort") object to itself to create a "MIDI through" effect: Anything that shows up at the MIDI-In port will automatically be sent out the MIDI-Out port.

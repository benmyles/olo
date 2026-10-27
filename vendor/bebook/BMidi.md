<!-- Classes And Methods - The Midi Kit - BMidi -->

## BMidi

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `midi/Midi.h`  
Library:| `libmidi.so`  
Allocation:| –  
[Class Overview](BMidi_Overview.md "BMidi")

  * [Constructor and Destructor](BMidi.md#BMidi_ConstructorDestructor)
  * [Member Functions](BMidi.md#BMidi_MemberFunctions)
  * [MIDI Hook and Spray Functions](BMidi.md#BMidi_MIDIHookAndSprayFunctions)

  
  
* * *

### Constructor and Destructor

#### BMidi()

`BMidi();`

Creates and returns a new `BMidi` object. The object's inflow thread is spawned and started in this function—in other words, `BMidi` objects are born with the ability to accept incoming messages. The "run" thread, on the other hand, isn't spawned until [`Start()`](BMidi.md#BMidi_Start "Start\(\)") is called.

#### ~BMidi()

`virtual ~BMidi();`

Kills the inflow and run threads after they've gotten to suitable stopping points (as defined below), deletes the list that holds the connections (but doesn't delete the objects contained in the list), then destroys the `BMidi` object.

The inflow thread is stopped after all currently-waiting MIDI hook messages have been read. No more messages are accepted while the "inflow queue" is being drained. The run thread is allowed to complete its current pass through the run loop and then told to stop (in the manner of the [`Stop()`](BMidi.md#BMidi_Stop "Stop\(\)") function).

While the destructor severs the connections that this `BMidi` object has formed, it doesn't sever the connections from other objects to this one. For example, consider the following (improper) sequence of calls:
    
    
    /* DON'T DO THIS... */
    a_midi->Connect(b_midi);
    b_midi->Connect(c_midi);
    ...
    delete b_midi;

The delete call severs the connection from `b_midi` to `c_midi`, but it doesn't disconnect `a_midi` and `b_midi`. You have to disconnect the object's "back-connections" explicitly:
    
    
    /* ...DO THIS INSTEAD */
    a_midi->Connect(b_midi);
    b_midi->Connect(c_midi);
    ...
    a_midi->Disconnect(b_midi);
    delete b_midi;

* * *

### Member Functions

#### AllNotesOff()

`virtual void AllNotesOff(bool controlMsgOnly,  
uint32 time = B_NOW);`

Sends a `B_ALL_NOTES_OFF` Control Change message, passing along the timestamp, to all 16 MIDI channels. If `channelMsgOnly` is `false`, the function also sends a Note Off message for all key numbers on all channels.

#### Connect()

`void Connect(BMidi* toObject);`

Connects the `BMidi` object's output to `toObject`'s input. The `BMidi` object can connect its output to any number of other objects. Each of these connected objects receives a MIDI hook call as the `BMidi` sprays messages.

Any object that's been the argument in a `Connect()` call should eventually be disconnected through a call to [`Disconnect()`](BMidi.md#BMidi_Disconnect "Disconnect\(\)"). In particular, care should be taken to disconnect objects when deleting a `BMidi` object, as described in the destructor.

See also: [`~BMidi()`](BMidi.md#BMidi_Destructor "~BMidi\(\)"), [`Connections()`](BMidi.md#BMidi_Connections "Connections\(\)"), [`IsConnected()`](BMidi.md#BMidi_IsConnected "IsConnected\(\)")

#### Connections()

`BList* Connections() const;`

Returns a [`BList`](BList.md "BList") that contains the objects that are connected to this object's output.

#### Disconnect()

`void Disconnect(BMidi* toObject);`

Severs the `BMidi`'s connection to the argument. The connection must have previously been formed through a call to [`Connect()`](BMidi.md#BMidi_Connect "Connect\(\)") with a like disposition of receiver and argument.

#### IsConnected()

`bool IsConnected(BMidi* toObject) const;`

Returns `true` if the argument is present in the receiver's list of connected objects.

#### IsRunning()

`bool IsRunning() const;`

Returns `true` if the object's [`Run()`](BMidi.md#BMidi_Run "Run\(\)") loop is looping; in other words, if the object has received a [`Start()`](BMidi.md#BMidi_Start "Start\(\)") function call, but hasn't been told to [`Stop()`](BMidi.md#BMidi_Stop "Stop\(\)") (or otherwise hasn't fallen out of the loop).

#### KeepRunning()

##### protected

`bool KeepRunning();`

Used by the [`Run()`](BMidi.md#BMidi_Run "Run\(\)") function to predicate its while() loop, as explained in the class description. This function should only be called from within [`Run()`](BMidi.md#BMidi_Run "Run\(\)").

#### Run()

##### private

`virtual void Run();`

A `BMidi`-derived class places its data-generating machinery in the `Run()` function, as described in the section "[Generating MIDI Messages](BMidi_Overview.md#BMidi_Overview_GeneratingMIDIMessages "Generating MIDI Messages")". Of particular note: Your implementation of `Run()` must exit when [`KeepRunning()`](BMidi.md#BMidi_KeepRunning "KeepRunning\(\)") returns false:
    
    
    void MyMidi::Run()
    {
       while (KeepRunning()) {
          /* Generate a message and spray it. */
       }
       /* You MUST exit when KeepRunning() returns false. */
    }

#### SnoozeUntil()

`void SnoozeUntil(uint32 tick) const;`

Puts the calling thread to sleep until `tick` milliseconds have elapsed since the computer was booted. This function is meant to be used in the implementation of the MIDI hook functions, as explained in the section "[Time](BMidi_Overview.md#BMidi_Overview_Time "Time")".

#### Start()

`virtual status_t Start();`

Tells the object to spawn its run loop (wherein it executes the [`Run()`](BMidi.md#BMidi_Run "Run\(\)") function) and then immediately returns. You can override this function in a `BMidi`-derived class to provide your own pre-running initialization. Make sure you call the inherited version of this function within your implementation.

Return Code| Description  
---|---  
`B_OK`| The run thread was successfully spawned and resumed.  
Thread and port error codes.| Something went wrong.  
  
#### Stop()

`virtual void Stop();`

Tells the object to stop generating MIDI data. Calling `Stop()` tells the [`KeepRunning()`](BMidi.md#BMidi_KeepRunning "KeepRunning\(\)") function to return `false`, thus causing the run loop to terminate. `Stop()` may return before the thread is dead. This function doesn't affect the state of in-coming data: The object will still be able to receive MIDI messages through its MIDI hook functions.

You can override this function in a `BMidi`-derived class to predicate the stop, or to perform post-performance clean-up (as two examples). Make sure, however, that you invoke the inherited version of this function within your implementation.

* * *

### MIDI Hook and Spray Functions

The protocols for the MIDI hook and spray functions are given below. What a particular function means (and the values of its arguments) is defined by the MIDI spec and isn't explained here. See "[Spray Functions](BMidi_Overview.md#BMidi_Overview_SprayFunctions "Spray Functions")" and "[MIDI Hook Functions](BMidi_Overview.md#BMidi_Overview_MIDIHookFunctions "MIDI Hook Functions")" for more information on how these functions are used.

####  ChannelPressure(), SprayChannelPressure()

`virtual void ChannelPressure(uchar channel,  
uchar pressure,  
uint32 time = B_NOW);``protected void SprayChannelPressure(uchar channel,  
uchar pressure,  
uint32 time);`

####  ControlChange(), SprayControlChange()

`virtual void ControlChange(uchar channel,  
uchar controlNumber,  
uchar controlValue,  
uint32 time = B_NOW);``protected void SprayControlChange(uchar channel,  
uchar controlNumber,  
uchar controlValue,  
uint32 time);`

See also: [`AllNotesOff()`](BMidi.md#BMidi_AllNotesOff "AllNotesOff\(\)")

####  KeyPressure(), SprayKeyPressure()

`virtual void KeyPressure(uchar channel,  
uchar note,  
uchar pressure,  
uint32 time = B_NOW);``protected void SprayKeyPressure(uchar channel,  
uchar note,  
uchar pressure,  
uint32 time);`

####  NoteOff(), SprayNoteOff()

`virtual void NoteOff(uchar channel,  
uchar note,  
uchar velocity,  
uint32 time = B_NOW);``protected void SprayNoteOff(uchar channel,  
uchar note,  
uchar velocity,  
uint32 time);`

####  NoteOn(), SprayNoteOn()

`virtual void NoteOn(uchar channel,  
uchar note,  
uchar velocity,  
uint32 time = B_NOW);``protected void SprayNoteOn(uchar channel,  
uchar note,  
uchar velocity,  
uint32 time);`

####  PitchBend(), SprayPitchBend()

`virtual void PitchBend(uchar channel,  
uchar lsb,  
uchar msb,  
uint32 time = B_NOW);``protected void SprayPitchBend(uchar channel,  
uchar lsb,  
uchar msb,  
uint32 time);`

####  ProgramChange(), SprayProgramChange()

`virtual void ProgramChange(uchar channel,  
uchar progNum,  
uint32 time = B_NOW);``protected void SprayProgramChange(uchar channel,  
uchar progNum,  
uint32 time);`

####  SystemCommon(), SpraySystemCommon()

`virtual void ProgramChange(uchar channel,  
uchar data1,  
uchar data2,  
uint32 time = B_NOW);``protected void SprayProgramChange(uchar channel,  
uchar data1,  
uchar data2,  
uint32 time);`

####  SystemExclusive(), SpraySystemExclusive()

`virtual void SystemExclusive(void* data,  
size_t dataLength,  
uint32 time = B_NOW);``protected void SpraySystemExclusive(void* data,  
size_t dataLength,  
uint32 time);`

####  SystemRealTime(), SpraySystemRealTime()

`virtual void SystemRealTime(uchar status,  
uint32 time = B_NOW);``protected void SpraySystemRealTime(uchar status,  
uint32 time);`

####  TempoChange(), SprayTempoChange()

`virtual void TempoChange(int32 beatsPerMinute,  
uint32 time = B_NOW);``protected void SprayTempoChange(int32 beatsPerMinute,  
uint32 time);`

<!-- Classes And Methods - The Midi Kit - BMidiText -->

## BMidiText

| Derived From:| [`BMidi`](BMidi.md "BMidi")  
---|---  
Mix-in Classes:| –  
Declared In:| `midi/MidiText.h`  
Library:| `libmidi.so`  
Allocation:| –  
[Class Overview](BMidiText_Overview.md "BMidiText")

  * [Constructor and Destructor](BMidiText.md#BMidiText_ConstructorDestructor)
  * [Member Functions](BMidiText.md#BMidiText_MemberFunctions)
  * [Output Text](BMidiText.md#BMidiText_OutputText)

  
  
* * *

### Constructor and Destructor

#### BMidiText()

`BMidiText();`

The constructor creates a new `BMidiText` object. The object's timer is set to zero and doesn't start ticking until the first message is received. To force the timer to start, call [`ResetTimer`](BMidiText.md#BMidiText_ResetTimer "ResetTimer\(\)")(`true`).

#### ~BMidiText()

`virtual ~BMidiText();`

The destructor destroys the object.

* * *

### Member Functions

#### ResetTimer()

`void AllNotesOff(bool start = false);`

Sets the object's internal timer to zero. If `start` is `false` the timer doesn't start ticking until the next MIDI message is received. If `start` is `true`, the timer begins immediately.

The timer value is used to compute the timestamp that's displayed at the beginning of each message description.

* * *

### Output Text

The text strings that a `BMidiText` object displays are shown below, listed by MIDI event

#### Channel Pressure
    
    
    timestamp: CHANNEL PRESSURE; channel = channel, pressure = pressure

#### Control Change
    
    
     _timestamp_ : CONTROL CHANGE; channel = _channel_ , control = _ctrl_num_ ,
       value = _ctrl_value_

#### Key Pressure
    
    
     _timestamp_ : KEY PRESSURE; channel = _channel_ , note = _note_ , pressure =
    _pressure_

#### Note Off
    
    
     _timestamp_ : NOTE OFF; channel = _channel_ , note = _note_ , velocity = _velocity_

#### Note On
    
    
     _timestamp_ : NOTE ON; channel = _channel_ , note = _note_ , velocity = _velocity_

#### Pitch Bend
    
    
     _timestamp_ : PITCH BEND; channel = _channel_ , lsb = _lsb_ , msb = _msb_

#### Program Change
    
    
     _timestamp_ : PROGRAM CHANGE; channel = _channel_ , program = _program_num_

#### System Common
    
    
     _timestamp_ : SYSTEM COMMON; status = _status_ , data1 = _data1_ , data2= _data2_

#### System Exclusive
    
    
     _timestamp_ : SYSTEM EXCLUSIVE;

Followed by the data itself (in hex, byte-by-byte), starting on the next line.

#### SystemRealTime()
    
    
    _timestamp_ : SYSTEM REAL TIME; status = _status_

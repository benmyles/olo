<!-- System Overview - The Midi Kit - BMidiText -->

## BMidiText

A [`BMidiText`](BMidiText.md "BMidiText") object prints, on standard output, a text description of each MIDI message it receives. You use [`BMidiText`](BMidiText.md "BMidiText") objects to debug and monitor your application; it has no other purpose.

To use a [`BMidiText`](BMidiText.md "BMidiText") object, you construct it and connect it to some other [`BMidi`](BMidi.md "BMidi") object as shown below:
    
    
    BMidiText midiText;
    
    otherMidiObj.Connect(midiText);
    otherMidiObj.Start();

BMidiText's output (the text it displays) is timed: When it receives a MIDI message that's timestamped for the future, the object waits until that time has come before it displays its message. While it's waiting, the object won't process any other in-coming messages. Because of this, you shouldn't connect the same [`BMidiText`](BMidiText.md "BMidiText") object to more than one [`BMidi`](BMidi.md "BMidi") object. To monitor two or more MIDI-producing objects, you should connect a separate [`BMidiText`](BMidiText.md "BMidiText") object to each.

The text that's displayed by a [`BMidiText`](BMidiText.md "BMidiText") follows this general format:
    
    
    timestamp: MESSAGE TYPE; message data

(Message-specific formats are given in the section "[Output Text](BMidiText.md#BMidiText_OutputText "Output Text")") Of particular note is the timestamp field. Its value is the number of milliseconds that have elapsed since the object received its first message. The time value is computed through the use of an internal timer; to reset this timer—a useful thing to do between performances, for example—you call the [`ResetTimer()`](BMidiText.md#BMidiText_ResetTimer "ResetTimer\(\)") function.

The [`BMidiText`](BMidiText.md "BMidiText") class doesn't generate or spray MIDI messages, so the performance and connection functions that it inherits from [`BMidi`](BMidi.md "BMidi") have no effect.

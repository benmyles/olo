<!-- System Overview - The Midi Kit - BMidiStore -->

## BMidiStore

The [`BMidiStore`](BMidiStore.md "BMidiStore") class defines a MIDI recording and playback mechanism. The MIDI messages that a [`BMidiStore`](BMidiStore.md "BMidiStore") object receives (at its input) are stored as events in an event list, allowing a captured performance to be played back later. The object can also read and write—or import and export—standard MIDI files.

* * *

### Recording

The ability to record a MIDI performance is vested in [`BMidiStore`](BMidiStore.md "BMidiStore")'s MIDI hook functions (NoteOn(), NoteOff(), etc.). When a MIDI hook is invoked, the function fabricates a discrete event based on the data it has received in its arguments, and adds the event to its event list. You don't need to tell a [`BMidiStore`](BMidiStore.md "BMidiStore") to start recording; it can record from the moment it's constructed.

For example, to record a performance from an external MIDI keyboard, you connect a [`BMidiStore`](BMidiStore.md "BMidiStore") to a [`BMidiPort`](BMidiPort.md "BMidiPort") object and then tell the [`BMidiStore`](BMidiStore.md "BMidiStore") to start:
    
    
    /* Record a keyboard performance. */
    BMidiStore MyStore;
    BMidiPort MyPort;
    
    MyPort.Open(...);
    MyPort.Connect(MyStore);
    MyPort.Start();
    /* Start playing... */

At the end of the performance, you tell the [`BMidiPort`](BMidiPort.md "BMidiPort") to stop:
    
    
    MyPort.Stop();

#### Timestamps

Events are added to a [`BMidiStore`](BMidiStore.md "BMidiStore")'s event list immediately upon arrival. Each event is given a timestamp as it arrives; the value of the timestamp is the value of the time argument that was passed to the MIDI hook function by the "upstream" object's spray function. There's no guarantee that the time arguments of successive MIDI events will be in chronological order. To ensure that the events are properly ordered, you should call [`SortEvents()`](BMidiStore.md#BMidiStore_SortEvents) before you read from the list (note that writing to a MIDI file automatically sorts the list).

[`BMidiStore`](BMidiStore.md "BMidiStore")'s input functions don't call [`SnoozeUntil()`](BMidi.md#BMidi_SnoozeUntil "SnoozeUntil\(\)"): A [`BMidiStore`](BMidiStore.md "BMidiStore") writes to its event list as soon as it gets a new message, it doesn't wait until the time indicated by the time argument.

#### Erasing and Editing a Recording

You can't. If you make a mistake while you're recording (for example) and want to try again, you can simulate emptying the object by disconnecting the input to the [`BMidiStore`](BMidiStore.md "BMidiStore"), destroying the object, making a new one, and re-connecting. For example:

Editing the events in the event list is less than impossible (were such a state possible). You can't do it, and you can't simulate it, at least not with the default implementation of [`BMidiStore`](BMidiStore.md "BMidiStore"). If you want to edit MIDI data, you have to provide your own [`BMidi`](BMidi.md "BMidi")-derived class.

* * *

### Playback

To "play" a [`BMidiStore`](BMidiStore.md "BMidiStore")'s list of events, you call the object's [`Start()`](BMidi.md#BMidi_Start "Start\(\)") function. As described in the [`BMidi`](BMidi.md "BMidi") class specification, [`Start()`](BMidi.md#BMidi_Start "Start\(\)") invokes [`Run()`](BMidi.md#BMidi_Run "Run\(\)"). [`BMidiStore`](BMidiStore.md "BMidiStore")'s [`Run()`](BMidi.md#BMidi_Run "Run\(\)") reads events in the order that they appear in the event list, and sprays the appropriate messages to the connected objects. You can interrupt a [`BMidiStore`](BMidiStore.md "BMidiStore") playback by calling [`Stop()`](BMidi.md#BMidi_Stop "Stop\(\)"); uninterrupted, the object will stop by itself after it has sprayed the last event in the list.

The events' timestamps are used as the time arguments in the spray functions that are called from within [`Run()`](BMidi.md#BMidi_Run "Run\(\)"). But with a twist: The time argument that's passed in the first spray call (for a given performance) is always `B_NOW`; subsequent time arguments are re-computed to maintain the correct timing in relation to the first event. In other words, when you tell a [`BMidiStore`](BMidiStore.md "BMidiStore") to start playing, the first event is performed immediately regardless of the actual value of its timestamp.

#### Setting the Current Event

You can tell the [`BMidiStore`](BMidiStore.md "BMidiStore") to begin playing from somewhere in the middle of the list by calling [`SetCurrentEvent()`](BMidiStore.md#BMidiStore_SetCurrentEvent "SetCurrentEvent\(\), CurrentEvent\(\)") before starting the playback. The function takes an index into the list.

If you want to start playing from a particular time offset into the event list, you first have to figure out which event lies at that time. To do this, you ask for the event that occurs at or after the time offset (in milliseconds) through the [`EventAtDelta()`](BMidiStore.md#BMidiStore_EventAtDelta) function. The value that's returned by this function is suitable as the argument to [`SetCurrentEvent()`](BMidiStore.md#BMidiStore_SetCurrentEvent "SetCurrentEvent\(\), CurrentEvent\(\)").

Keep in mind that [`EventAtDelta()`](BMidiStore.md#BMidiStore_EventAtDelta) returns the index of the first event at or after the desired offset. If you need to know the actual offset of the winning event, you can pass its index to [`DeltaOfEvent()`](BMidiStore.md#BMidiStore_DeltaOfEvent):
    
    
    long firstEvent = MyStore->EventAtDelta(3000);
    long actualDelta = MyStore->DeltaOfEvent(firstEvent);

* * *

### Reading and Writing MIDI Files

You add events to a BMidiStore's event list by reading, or importing, a Standard MIDI File through the [`Import()`](BMidiStore.md#BMidiStore_Import) function. You can import any number of files into the same [`BMidiStore`](BMidiStore.md "BMidiStore") object. After you import a file, the event list is automatically sorted.

One thing you shouldn't do is import a MIDI file into a [`BMidiStore`](BMidiStore.md "BMidiStore") that contains events that were previously recorded from a [`BMidiPort`](BMidiPort.md "BMidiPort") (in an attempt to mix the file and the recording). Nor does the reverse work: You can't import a file and then record from a [`BMidiPort`](BMidiPort.md "BMidiPort"). The file's timestamps are incompatible with those that are generated for events that are received from the [`BMidiPort`](BMidiPort.md "BMidiPort"); the result certainly won't be satisfactory.

To write the event list as a MIDI file, you call [`BMidiStore`](BMidiStore.md "BMidiStore")'s [`Export()`](BMidiStore.md#BMidiStore_Export "Export\(\), Import\(\)") function:

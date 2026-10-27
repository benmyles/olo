<!-- System Overview - The Midi Kit - BMidi -->

## BMidi

[`BMidi`](BMidi.md "BMidi") is an abstract class that defines protocols and default implementations for functions that most of the other Midi Kit classes inherit. The functions that [`BMidi`](BMidi.md "BMidi") defines fall into four categories:

Connection functions.
    

The connection functions let you connect the output of one [`BMidi`](BMidi.md "BMidi") object to the input of another [`BMidi`](BMidi.md "BMidi") object.

Performance functions.
    

[`BMidi`](BMidi.md "BMidi") objects that generate MIDI data (by reading from a port or file, as examples) implement the [`Run()`](BMidi.md#BMidi_Run "Run\(\)") hook function. [`Run()`](BMidi.md#BMidi_Run "Run\(\)") is the brains of a MIDI performance; [`Start()`](BMidi.md#BMidi_Start "Start\(\)") and [`Stop()`](BMidi.md#BMidi_Stop "Stop\(\)") control the performance.

"Spray" functions.
    

The "spray" functions send MIDI messages to each of the objects that are connected to the output of the sprayer. There's a spray function for each type of MIDI message; for example, [`SprayNoteOn()`](BMidi.md#BMidi_SprayNoteOn) corresponds to MIDI's Note On message.

"MIDI hook" functions.
     The "MIDI hook" functions define the object's response to a particular type of MIDI message. There's a hook function for each MIDI message ([`NoteOn()`](BMidi.md#BMidi_NoteOn "NoteOn\(\), SprayNoteOn\(\)"), [`NoteOff()`](BMidi.md#BMidi_NoteOff "NoteOff\(\), SprayNoteOff\(\)"), etc.). The hook functions are invoked automatically as "upstream" objects call the corresponding spray functions. 

* * *

### Forming Connections

MIDI data streams through your application, passing from one [`BMidi`](BMidi.md "BMidi")-derived object to another. Each object does whatever it's designed to do: Sends the data to a MIDI port, writes it to a file, modifies it and passes it on, and so on.

You form a tree of [`BMidi`](BMidi.md "BMidi") objects through [`BMidi`](BMidi.md "BMidi")'s [`Connect()`](BMidi.md#BMidi_Connect "Connect\(\)") function, which takes a single [`BMidi`](BMidi.md "BMidi") argument. By calling [`Connect()`](BMidi.md#BMidi_Connect "Connect\(\)"), you connect the output of the calling object to the input of the argument object. (i.e. the caller is the source; the argument is the destination.)

Let's say you want to connect a MIDI keyboard to your computer, play it, and have the performance recorded in a file. You connect a [`BMidiPort`](BMidiPort.md "BMidiPort") object, which reads data from the MIDI port, to a [`BMidiStore`](BMidiStore.md "BMidiStore") object, which stores the data that's sent to it and can write it to a file:
    
    
    /* Connect the output of a BMidiPort to the input of a BMidiStore. */
    BMidiPort m_port;
    BMidiStore m_store;
    
    m_port.Connect(&m_store);

You also have to tell the [`BMidiPort`](BMidiPort.md "BMidiPort") to start listening to the MIDI port, by calling its [`Start()`](BMidi.md#BMidi_Start "Start\(\)") function. This is explained in a later section.

Once you've made the recording, you can play it back by re-connecting the objects in the opposite direction:
    
    
    /* We'll disconnect first, although this isn't strictly necessary. */
    m_port.Disconnect(&m_store);
    m_store.Connect(&m_port);

In this configuration, a [`Start()`](BMidi.md#BMidi_Start "Start\(\)") call to `m_store` would cause its MIDI data to flow into the [`BMidiPort`](BMidiPort.md "BMidiPort") (and thence to a synthesizer, for example, for realization).

#### Sources and Destinations

A [`BMidi`](BMidi.md "BMidi") object can be the source for any number of other objects:
    
    
    a_midi.Connect(&b_object);
    a_midi.Connect(&c_object);
    a_midi.Connect(&d_object);

A source can get a list of its destinations through the [`Connections()`](BMidi.md#BMidi_Connections "Connections\(\)") function.

A BMidi object can be the destination for any number of sources:
    
    
    b_midi.Connect(&e_object);
    c_midi.Connect(&e_object);
    d_midi.Connect(&e_object);

However, a destination can't get a list of its sources.

* * *

### Generating MIDI Messages

If your class wants to generate new MIDI data (as opposed to filtering or realizing the data it receives), it must implement the [`Run()`](BMidi.md#BMidi_Run "Run\(\)") function. An implementation of [`Run()`](BMidi.md#BMidi_Run "Run\(\)") should include a while() loop that broadcasts one or more MIDI message on each pass (typically it broadcasts only one), by invoking a spray function. To predicate the loop you must test the value of the [`KeepRunning()`](BMidi.md#BMidi_KeepRunning "KeepRunning\(\)") boolean function.

The outline of a [`Run()`](BMidi.md#BMidi_Run "Run\(\)") implementation looks like this:
    
    
    void MyMidi::Run()
    {
       while (KeepRunning()) {
          /* Generate a message and spray it. For example... */
          SprayNoteOn(...);
       }
       /* You MUST exit when KeepRunning() returns false. */
    }

Warning

![Warning](./images/admonitions/Stop_32.png)

The [`Run()`](BMidi.md#BMidi_Run "Run\(\)") function _must_ exit when [`KeepRunning()`](BMidi.md#BMidi_KeepRunning "KeepRunning\(\)") returns `false`. If it doesn't, you'll leak threads.

To tell an object to perform its [`Run()`](BMidi.md#BMidi_Run "Run\(\)") function, you call the object's [`Start()`](BMidi.md#BMidi_Start "Start\(\)") function—you never call [`Run()`](BMidi.md#BMidi_Run "Run\(\)") directly. [`Start()`](BMidi.md#BMidi_Start "Start\(\)") causes the object to spawn a thread (its "run" thread) and execute [`Run()`](BMidi.md#BMidi_Run "Run\(\)") within it. When you're tired of the object's performance, you call its [`Stop()`](BMidi.md#BMidi_Stop "Stop\(\)") function.

Keep in mind that the [`Run()`](BMidi.md#BMidi_Run "Run\(\)") function is only needed in classes that introduce new MIDI data into a performance. As examples, [`BMidiStore`](BMidiStore.md "BMidiStore")'s [`Run()`](BMidi.md#BMidi_Run "Run\(\)") sprays messages that correspond to the MIDI data that it stores, and [`BMidiPort`](BMidiPort.md "BMidiPort") reads data from the MIDI port and sprays messages accordingly.

Another point to keep in mind is that the [`Run()`](BMidi.md#BMidi_Run "Run\(\)") function can run ahead of real time. It doesn't have to generate and spray data precisely at the moment that the data needs to be realized. This is further explained in the section "[Time](BMidi_Overview.md#BMidi_Overview_Time "Time")".

Note

![Note](./images/admonitions/Info_32.png)

The [`BMidiSynthFile`](BMidiSynthFile.md "BMidiSynthFile") class differs from the other classes in the way that it implements and uses its [`Run()`](BMidi.md#BMidi_Run "Run\(\)") function; in particular, it doesn't spawn a run thread. See the [`BMidiSynthFile`](BMidiSynthFile.md "BMidiSynthFile") class for more information.

* * *

### Spray Functions

The spray functions send data to the [`BMidi`](BMidi.md "BMidi") objects that are connected to the running object's output. There's a separate spray function for each of the MIDI message types: [`SprayNoteOn()`](BMidi.md#BMidi_SprayNoteOn), [`SprayNoteOff()`](BMidi.md#BMidi_SprayNoteOff), [`SprayPitchBend()`](BMidi.md#BMidi_SprayPitchBend), and so on. Spray functions are always found within a [`BMidi`](BMidi.md "BMidi")'s [`Run()`](BMidi.md#BMidi_Run "Run\(\)") loop, but they can be found in other places as well. For example, if you're creating a MIDI filter, you would use spray functions in the implementations of the object's MIDI hook functions (as explained next).

* * *

### MIDI Hook Functions

The MIDI hook functions are hooks that are invoked upon an object's connections when the object sprays MIDI data. The functions take the names of the MIDI messages to which they respond: [`NoteOn()`](BMidi.md#BMidi_NoteOn "NoteOn\(\), SprayNoteOn\(\)") responds to a Note On message, [`NoteOff()`](BMidi.md#BMidi_NoteOff "NoteOff\(\), SprayNoteOff\(\)") responds to a Note Off, and so on. For example, this…
    
    
    a_midi.Connect(&b_midi);
    a_midi.SprayNoteOn(...);

…causes `b_midi`'s [`NoteOn()`](BMidi.md#BMidi_NoteOn "NoteOn\(\), SprayNoteOn\(\)") function to be invoked. The arguments that are passed to [`NoteOn()`](BMidi.md#BMidi_NoteOn "NoteOn\(\), SprayNoteOn\(\)") are taken directly from the [`SprayNoteOn()`](BMidi.md#BMidi_SprayNoteOn) call.

[`BMidi`](BMidi.md "BMidi") doesn't provide default implementations for any of the MIDI hooks; it's up to each [`BMidi`](BMidi.md "BMidi")-derived class to decide how to respond to MIDI messages.

Every [`BMidi`](BMidi.md "BMidi") object automatically spawns an "inflow" thread when it's constructed. It's in this thread that the spray-invoked MIDI hooks are executed. The inflow thread is always running—the [`Start()`](BMidi.md#BMidi_Start "Start\(\)") and [`Stop()`](BMidi.md#BMidi_Stop "Stop\(\)") functions don't affect it. As soon as you construct an object, it's ready to receive data.

#### Calling MIDI Hooks Directly

You can also feed MIDI data to a [`BMidi`](BMidi.md "BMidi") object by invoking the MIDI hook functions directly. For example, let's say you just want to play a note on the General MIDI synthesizer. You don't have to create your own [`BMidi`](BMidi.md "BMidi") class simply to implement a [`Run()`](BMidi.md#BMidi_Run "Run\(\)") function that sprays the note; instead, all you have to do is this:
    
    
    BMidiSynth midiSynth;
    
    /* Initialize the BMidiSynth as described in that class. */
    ...
    /* Play a note. */
    midiSynth.NoteOn(...);

Keep in mind that when you invoke a hook function directly, it executes synchronously in the calling thread. Furthermore, the object may also be receiving MIDI messages in its inflow thread. For the Midi Kit-defined classes, this isn't a problem.

#### Creating a MIDI Filter

Some BMidi objects "act as filters: They receive data, modify it, and then pass it on. To do this, you call the appropriate spray functions from within the implementations of the MIDI hooks. Below is the implementation of the NoteOn() function for a proposed class called Transposer. It takes each Note On, transposes it up a half step, and then sprays it:
    
    
    void Transposer::NoteOn(uchar channel, uchar keyNumber,
                      uchar velocity, uint32 time)
    {
       uchar new_key = max(keyNumber + 1, 127);
       SprayNoteOn(channel, new_key, velocity, time);
    }

There's a subtle but important distinction between a filter class and a "performance" class (where the latter is a class that's designed to actually realize the MIDI data it receives). The distinction has to do with time, and is explained in the next section. An implication of the distinction that affects the current discussion is that it may not be a great idea to invest, in a single object, the ability to filter and perform MIDI data. Both [`BMidiStore`](BMidiStore.md "BMidiStore") and [`BMidiPort`](BMidiPort.md "BMidiPort") are performance classes—objects of these classes realize the data they receive, the former by caching it, the latter by sending it out the MIDI port. In neither of these classes do the MIDI hooks spray data.

* * *

### Time

Every spray and MIDI hook function takes a final time argument. This argument declares when the message that the function represents should be performed. The argument is given in ticks (milliseconds). Tick 0 occurs when you boot your computer; the tick counter automatically starts running at that point. To get the current tick measurement, you call the global, Kernel Kit-defined [`system_time()`](TheKernelKit_Time.md#system_time "system_time\(\)") function and divide by 1000 ([`system_time()`](TheKernelKit_Time.md#system_time "system_time\(\)") returns microseconds).

By convention, time arguments are applied at an object's input. In other words, a MIDI hook should look at the time argument, wait until the designated time, and then do whatever it does that it does do. However, this only applies to [`BMidi`](BMidi.md "BMidi")-derived classes that are designed to perform MIDI data. Objects that simply filter data shouldn't apply the time argument.

To apply the time argument, you call the [`SnoozeUntil()`](BMidi.md#BMidi_SnoozeUntil "SnoozeUntil\(\)") function, passing the value of time. For example, a "performance" [`NoteOn()`](BMidi.md#BMidi_NoteOn "NoteOn\(\), SprayNoteOn\(\)") function would look like this:
    
    
    void MyPerformer::NoteOn(uchar channel, uchar keyNumber,
                      uchar velocity, uint32 time)
    {
       SnoozeUntil(time);
       /* Perform the data here. */
    }

If time designates a tick that has already tocked, [`SnoozeUntil()`](BMidi.md#BMidi_SnoozeUntil "SnoozeUntil\(\)") returns immediately; otherwise it tells the inflow thread to snooze until the designated tick is at hand.

* * *

### Spraying Time

If you're implementing the [`Run()`](BMidi.md#BMidi_Run "Run\(\)") function, then you have to generate a time value yourself which you pass as the final argument to each spray function that you call. The value you generate depends on whether your class runs in real time, or ahead of time.

#### Running in Real Time

If your class conjures MIDI data that needs to be performed immediately, you should use the `B_NOW` macro as the value of the time arguments that you pass to your spray functions. `B_NOW` is simply a cover for ([`system_time()`](TheKernelKit_Time.md#system_time "system_time\(\)")/1000) (converted to uint). By using `B_NOW` as the time argument you're declaring that the data should be performed in the same tick in which it was generated. This probably won't happen; by the time the MIDI hooks are called and the data realized, a couple of ticks may have elapsed. In this case, the MIDI hooks' [`SnoozeUntil()`](BMidi.md#BMidi_SnoozeUntil "SnoozeUntil\(\)") calls will see that the time value has passed and will return immediately, allowing the data to be realized as quickly as possible.

#### Running Ahead of Time

If you're generating data ahead of its performance time, you need to compute the time value so that it pinpoints the correct time in the future. For example, if you want to create a class that generates a note every 100 milliseconds, you need to do something like this:
    
    
    void MyTicker::Run()
    {
       uint32 when = B_NOW;
       uchar key_num;
    
       while (KeepRunning()) {
    
          /* Make a new note. */
          SprayNoteOn(1, 60, 64, when);
    
          /* Turn the note off 99 ticks later. */
          when += 99;
          SprayNoteOff(1, 60, 0, when);
    
          /* Bump the when variable so the next Note On
          * will be 100 ticks after this one.
          */
          when += 1;
       }
    }

When a `MyTicker` object is told to start running, it generates a sequence of Note On/Note Off pairs, and sprays them to its connected objects. Somewhere down the line, a performance object will apply the time value by calling [`SnoozeUntil()`](BMidi.md#BMidi_SnoozeUntil "SnoozeUntil\(\)").

But what keeps `MyTicker` from running wild and generating thousands or millions of notes—which aren't scheduled to be played for hours—as fast as possible? It's because the spray functions pass data to the MIDI hooks through Kernel Kit ports that are 1 (one) message deep. So, as long as one of the MIDI hooks calls [`SnoozeUntil()`](BMidi.md#BMidi_SnoozeUntil "SnoozeUntil\(\)"), the spraying object will never be more than one message ahead.

A useful feature of this mechanism is that if you connect a series of [`BMidi`](BMidi.md "BMidi") object that don't invoke [`SnoozeUntil()`](BMidi.md#BMidi_SnoozeUntil "SnoozeUntil\(\)"), you can process MIDI data faster than real-time. For example, let's say you want to spray data from one [`BMidiStore`](BMidiStore.md "BMidiStore") object, pass the data through a filter, and then store it in another [`BMidiStore`](BMidiStore.md "BMidiStore"). The [`BMidiStore`](BMidiStore.md "BMidiStore") MIDI hooks don't call [`SnoozeUntil()`](BMidi.md#BMidi_SnoozeUntil "SnoozeUntil\(\)"); thus, data will flow out of the first object, through the filter, and into its destination as quickly as possible, allowing you to process hours of real-time data in just a few seconds. Of course, if you add a performance object into this mix (so you can hear the data while it's being processed), the data flow will be tethered, as described above.

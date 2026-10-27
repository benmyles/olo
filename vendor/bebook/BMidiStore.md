<!-- Classes And Methods - The Midi Kit - BMidiStore -->

## BMidiStore

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `midi/MidiStore.h`  
Library:| `libmidi.so`  
Allocation:| –  
[Class Overview](BMidiStore_Overview.md "BMidiStore")

  * [Constructor and Destructor](BMidiStore.md#BMidiStore_ConstructorDestructor)
  * [Member Functions](BMidiStore.md#BMidiStore_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BMidiStore()

`BMidiStore();`

The constructor creates a new, empty `BMidiStore` object.

#### ~BMidiStore()

`virtual ~BMidiStore();`

The destructor destroys the object and its storage.

* * *

### Member Functions

#### BeginTime()

`uint32 BeginTime() const;`

Returns the time, in ticks, at which the most recent performance started. This function is only valid if the object has actually performed.

####  CountEvents(), SortEvents(), DeltaOfEvent(), EventAtDelta()

`uint32 CountEvents() const;``void SortEvents(bool force = false);``uint32 DeltaOfEvent(uint32 index) const;``uint32 EventAtDelta(uint32 delta) const;`

`CountEvents()` returns the number of events in the object's event list.

`SortEvents()` time-sorts the events in the event list. The object maintains a (conservative) notion of whether the events are already sorted; if `force` is `false` and the object doesn't think the operation is necessary, the sorting isn't performed. If `force` is `true`, the operation is always performed, regardless of its necessity.

`DeltaOfEvent()` returns the "delta time" of the `index`'th event in the object's list of events. An event's delta time is the time span, in milliseconds, between the first event in the event list and itself.

`EventAtDelta()` Returns the index of the event that occurs on or after `delta` milliseconds from the beginning of the event list.

####  Export(), Import()

`status_t Export(const entry_ref* to_file,  
int32 format);``status_t Export(const entry_ref* from_file);`

`Export()` time-sorts the object's event list, and then writes the list as a standard MIDI file in the designated `format`; the `format` argument should be either 0 or 1. The file that `to_file` refers to must already exist, and is not cleared before it's written.

`Import()` reads the standard MIDI file from the designated file and adds its events to the object's event list.

Return Code| Description  
---|---  
`B_OK`| The file was successfully written or read.  
`B_NO_MEMORY` (`Import()` only)| Couldn't allocate enough memory to accommodate the file.  
POSIX errors| Some other file error prevented the operation.  
  
####  SetCurrentEvent(), CurrentEvent()

`void SetCurrentEvent(uint32 index);``uint32 CurrentEvent();`

Sets the object's "current event"—the event that it will perform next—to the event at index in the event list.

`CurrentEvent()` returns the index of the event that will be performed next.

####  SetTempo(), Tempo()

`void SetTempo(int32 beatsPerMinute);``int32 Tempo() const;`

Sets and returns the object's tempo—the speed at which it performs events—in beats per minute. The default tempo is 60 bpm.

<!-- Classes And Methods - The Support Kit - BStopWatch -->

## BStopWatch

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `support/StopWatch.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BStopWatch_Overview.md "BStopWatch")

  * [Constructor and Destructor](BStopWatch.md#BStopWatch_ConstructorDestructor)
  * [Member Functions](BStopWatch.md#BStopWatch_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BStopWatch()

`BStopWatch(const char* name,  
bool silent = false);`

Creates a `BStopWatch` object, names it, and starts its timer. If `silent` is `false` (the default), the object will print its elapsed time when it's destroyed; otherwise the message isn't printed. To get the elapsed time from a silent `BStopWatch`, call [`ElapsedTime()`](BStopWatch.md#BStopWatch_ElapsedTime "ElapsedTime\(\)").

#### ~BStopWatch()

`~BStopWatch();`

Stops the object's timer, prints a timing message to standard out (unless it's running silently), and then destroys the object. By default the timing message looks like this:
    
    
    StopWatch "name": _f_ usecs.
    

If you've recorded some lap points (through the [`Lap()`](BStopWatch.md#BStopWatch_Lap "Lap\(\)") function), you'll also see the lap times as well:
    
    
    StopWatch "name": _f_ usecs.
    [_lap#_ : _soFar_ #_thisLap_] [_lap#_ : _soFar_ #_thisLap_] [_lap#_ : _soFar_ #_thisLap_]...
    

…where _`lap#`_ is the number of the lap, _`soFar`_ was the total elapsed time at that lap, and _`thisLap`_ was the time it took to complete the lap.

* * *

### Member Functions

#### ElapsedTime()

`bigtime_t ElapsedTime() const;`

Returns the elapsed time, in microseconds, since the object was created or last Reset(). This function doesn't print the time message, nor does it touch the timer (the timer keeps running—unless it's [`Suspend()`](BStopWatch.md#BStopWatch_Suspend "Suspend\(\), Resume\(\), Reset\(\)")ed).
    
    
    BStopWatch watch("Timer 0");
    ...
    printf("Elapsed time: %Ldn", watch.ElapsedTime());

#### Lap()

`bigtime_t Lap();`

Records a "lap point" and returns the total elapsed time so far. The lap point times are printed individually when the object is destroyed, provided the object isn't silent. You can record as many as eight lap points; if you ask for a ninth lap point, the lap isn't recorded and this function returns 0. See [`~BStopWatch()`](BStopWatch.md#BStopWatch_Destructor "~BStopWatch\(\)") for a description of what the lap points look like when they're printed.

#### Name()

`const char* Name() const;`

Returns the name of the object, as set in the constructor.

####  Suspend(), Resume(), Reset()

`void Suspend();``void Resume();``void Reset();`

These functions affect the object's timer.

`Suspend()` stops the timer but doesn't reset it.

`Resume()` starts the timer running again.

`Reset()` sets the elapsed time to 0, but doesn't stop the timer. You can call `Reset()` at any time, regardless of whether the object is running or suspended.

<!-- Classes And Methods - The Kernel Kit - Time Information -->

## Time Information

Declared in:| `kernel/OS.h`  
---|---  
Library:| `libroot.so`  
  
* * *

### Time Functions

####  real_time_clock(), real_time_clock_usecs(), set_real_time_clock()

`uint32 real_time_clock();``bigtime_t real_time_clock_usecs();``void set_real_time_clock(int32 secs_since_jan1_1970);`

`real_time_clock()` returns the number of seconds that have elapsed since January 1, 1970.

`real_time_clock_usecs()` measures the same time span in microseconds.

`set_real_time_clock()` sets the value that the other two functions refer to.

#### system_time()

`bigtime_t system_time();`

Returns the number of microseconds that have elapsed since the computer was booted.

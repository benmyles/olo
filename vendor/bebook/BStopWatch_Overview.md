<!-- System Overview - The Support Kit - BStopWatch -->

## BStopWatch

The [`BStopWatch`](BStopWatch.md "BStopWatch") class is a debugging tool designed to time the execution of portions of your code. When a [`BStopWatch`](BStopWatch.md "BStopWatch") object is constructed, it starts an internal timer. When it's deleted, it stops the timer and prints the elapsed time to standard out in this format:
    
    
    StopWatch "_name_ ": _f_ usecs.

Where _`name`_ is the name that you gave to the object when you constructed it, and _`f`_ is the elapsed time in microseconds.

Look at all these other things you can do…

  * Suspend, resume, and reset the timer ([`Suspend()`](BStopWatch.md#BStopWatch_Suspend "Suspend\(\), Resume\(\), Reset\(\)"), [`Resume()`](BStopWatch.md#BStopWatch_Resume), [`Reset()`](BStopWatch.md#BStopWatch_Reset)).

  * Ask for the current elapsed time without stopping the timer ([`ElapsedTime()`](BStopWatch.md#BStopWatch_ElapsedTime "ElapsedTime\(\)")).

  * Record "lap points" that are printed out at the end of the run ([`Lap()`](BStopWatch.md#BStopWatch_Lap "Lap\(\)")).

Using a [`BStopWatch`](BStopWatch.md "BStopWatch") is simple; this…
    
    
    BStopWatch *watch = new BStopWatch("Timer 0");
    /* The code you want to time goes here. */
    delete watch;
    ...

…will produce, on standard out, a message that goes something like this:
    
    
    StopWatch "Timer 0": 492416 usecs.

This would indicate that the timed code took about half a second to execute—remember, you're looking at microseconds.

If you want to time an entire function, just toss a StopWatch on the stack:
    
    
    void MyFunc()
    {
       BStopWatch watch("Timer 0");
       ...
    }

When the function returns, the [`BStopWatch`](BStopWatch.md "BStopWatch") prints its message.

Warning

![Warning](./images/admonitions/Stop_32.png)

[`BStopWatch`](BStopWatch.md "BStopWatch") objects are useful if you want to get an idea of where your cycles are going. But you shouldn't rely on them for painfully accurate measurements.

<!-- Classes And Methods - The Support Kit - Debugging Tools -->

## Debugging Tools

Declared in: `support/Debug.h`

The Support Kit provides a set of macros that help you debug your application. These tools let you print information to standard output or to the serial port, and conditionally enter the debugger.

To enable the Support Kit's debugging macros, simply compile your code with the DEBUG preprocessor variable defined. The debugging facilities also provide a debug flag which you can use to selectively turn debugging on and off in your code. You therefore have a both compile time and run time methods for enabling debugging. Note that the DEBUG variable supercedes the runtime `debug` flag; if it is not defined, debugging will be disabled, irrespective of the state of the debug flag.

Serial debugging is enabled if you hold down the F1 key (Intel) or the Delete key (Macintosh) while booting into BeOS.

* * *

### The DEBUG Preprocessor Variable

You can define the DEBUG preprocessor variable by adding the following line to your `makefile`:
    
    
    CFLAGS := -DDEBUG

There is currently no mechanism for specifying #define's from the MetroWerks IDE. If you are building with the IDE, you must add the following line to your source files:
    
    
    #define DEBUG

When you're through debugging your application, simply remove the DEBUG definition and the debugging macros will be compiled away—you don't have to actually go into the code and remove the macros or comment them out.

* * *

### The Debug Flag

The `SET_DEBUG_ENABLED()` macro turns the application-wide debug flag on or off. Debug macros perform their intended functions only if DEBUG is #defined and the debug flag is turned on. Unlike the DEBUG preprocessor variable, which enables or disables debugging for sections of code, the state of the debug flag follows the flow of program execution; turning the debug flag on before a function call, for example, will cause it to be on inside the function as well.

Note

![Note](./images/admonitions/Info_32.png)

The debug flag is on by default. If you want to (initially, at least) turn it off, you should call `SET_DEBUG_ENABLED`(`FALSE`) as one of your first acts in `main()`.

* * *

### Macros

#### DEBUG_ONLY()
    
    
    DEBUG_ONLY(expression)

This macro blindly executes expression and is useful when you wish to execute a piece of code only when debugging is enabled.

####  DEBUGGER(), TRESPASS(), ASSERT(), ASSERT_WITH_MESSAGE()
    
    
    DEBUGGER(var_args)
    TRESPASS()
    ASSERT(condition)
    ASSERT_WITH_MESSAGE(condition, message)

These macros cause your program to enter the debugger: `DEBUGGER()` and `TRESPASS()` always enter the debugger, while `ASSERT()` and `ASSERT_WITH_MESSAGE()` enter only if condition (which can be any normal C or C++ expression) evaluates to `FALSE`.

`DEBUGGER()` takes a `printf()`-style variable-length argument that must be wrapped inside a second set of parentheses; for example:
    
    
    DEBUGGER(("What time is it? %f\n", system_time()));

The argument is evaluated and printed in the debugger's shell.

`TRESPASS()` is equivalent to `DEBUGGER`("Should not be here");.

If `ASSERT()` enters the debugger, the following message is printed:
    
    
    Assert failed: File: _filename_ , Line: _number_. _condition_

If `ASSERT_WITH_MESSAGE()` enters the debugger, the following message is printed:
    
    
    Assert failed: File: _filename_ , Line: _number_. _message_

Note that the condition argument needn't be wrapped in a second set of parentheses.

####  PRINT(), SERIAL_PRINT()
    
    
    PRINT(var_args)
    SERIAL_PRINT(var_args)

These macros print the message given by var_args. The argument takes the variable argument form of a `printf()` call and must be wrapped inside a second set of parenthesis; for example:
    
    
    PRINT(("The time is %f\n", system_time()));

`PRINT()` sends the message to standard out; `SERIAL_PRINT()` to a serial port at a data rate of 19,200 bits per second. The output is sent to /dev/ports/serial4 on BeBoxes, /dev/modem on Macs, and /dev/ports/serial1 on Intel machines.

#### PRINT_OBJECT()
    
    
    PRINT_OBJECT(object)

Prints information about the argument object (which must be a pointer to a C++ object) by calling the object's `PrintToStream()` function. The macro doesn't check to make sure that object actually implements the function, so you should use this macro with care.

Object information is always printed to standard out (there isn't a serial port version of the call).

####  SET_DEBUG_ENABLED(), IS_DEBUG_ENABLED()
    
    
    SET_DEBUG_ENABLED(flag)
    IS_DEBUG_ENABLED(void)

The `SET_DEBUG_ENABLED()` macro sets the state of the run time debug flag: a `TRUE` argument turns it on, `FALSE` turns it off. When the flag is on, the other debugging macros work; when it's off, they're ignored. The debug flag is set to `TRUE` by default.

The debug flag is only meaningful if your code was compiled with the DEBUG preprocessor variable defined. Without this definition, the debugging macros will always be disabled.

`IS_DEBUG_ENABLED()` returns the current state of the debug flag.

####  TRACE(), SERIAL_TRACE()
    
    
    TRACE(void)
    SERIAL_TRACE(void)

These macros print the name of the source code file that contains the currently executing code (in other words, the file that contains the TRACE() call itself), the line number of the code, and the thread_id of the calling thread. The information is printed in this form:
    
    
    File: _filename_ , Line: _number_ , Thread: _id_

`TRACE()` sends the message to standard out; `SERIAL_TRACE()` to a serial port at a data rate of 19,200 bits per second. The output is sent to /dev/ports/serial4 on BeBoxes, /dev/modem on Macs, and /dev/ports/serial1 on Intel machines.

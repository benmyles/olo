<!-- System Overview - The Kernel Kit - Images -->

## Images  
  
An image is compiled code. There are three types of image:

An app image
    

Is an application. Every application has a single app image.

A library image
    

Is a dynamically linked library (a "shared library"). Most applications link against the system libraries (`libroot.so`, `libbe.so`, and so on) that Be provides.

An add-on image
    

Is an image that you load into your application as it's running. Symbols from the add-on image are linked and references are resolved when the image is loaded. An add-on image provides a sort of "heightened dynamic linking" beyond that of a DLL.

The following sections explain how to load and run an app image, how to create a shared library, and how to create and load an add-on image.

* * *

### Loading an App Image

Loading an app image is like running a "sub-program." The image that you load is launched in much the same way as had you double-clicked it in the Tracker, or launched it from the command line. It runs in its own team—it doesn't share the address space of the application from which it was launched—and, generally, leads its own life.

Any application can be loaded as an app image; you don't need to issue special compile instructions or otherwise manipulate the binary. The one requirement of an app image is that it must have a `main()` function.

To load an app image, you call the [`load_image()`](TheKernelKit_Images.md#load_image "load_image\(\)") function:
    
    
    thread_id load_image(int32 argc,
        const char **argv,
        const char **env)

The function's first two arguments identify the app image (file) that you want to launch—we'll return to this in a moment. Having located the file, the function creates a new team, spawns a main thread in that team, and returns the thread_id of the thread to you. The thread isn't running: To make it run you pass the thread_id to [`resume_thread()`](TheKernelKit_ThreadsAndTeams.md#resume_thread "resume_thread\(\)") or [`wait_for_thread()`](TheKernelKit_ThreadsAndTeams.md#wait_for_thread "wait_for_thread\(\)") (as explained in "[Threads And Teams](TheKernelKit_ThreadsAndTeams_Overview.md "Threads And Teams")").

The `argc`/`argv` argument pair is copied and forwarded to the new thread's `main()` function:

  * The first string in the `argv` array must be the name of the image file that you want to launch; [`load_image()`](TheKernelKit_Images.md#load_image "load_image\(\)") uses this string to find the file. You then install any other arguments you want in the array, and terminate the array with a `NULL` entry. `argc` is set to the number of entries in the `argv` array (not counting the terminating `NULL`). It's the caller's responsibility to free the `argv` array after [`load_image()`](TheKernelKit_Images.md#load_image "load_image\(\)") returns (remember—the array is copied before it's passed to the new thread).

  * `envp` is an array of environment variables that are also passed to `main()`. Typically, you use the global environ pointer (which you must declare as an extern—see the example, below). You can, of course, create your own environment variable array: As with the `argv` array, the `envp` array should be terminated with a `NULL` entry, and you must free the array when [`load_image()`](TheKernelKit_Images.md#load_image "load_image\(\)") returns (that is, if you allocated it yourself—don't try to free environ).

The following example demonstrates a typical use of [`load_image()`](TheKernelKit_Images.md#load_image "load_image\(\)"). First, we include the appropriate files and declare the necessary variables:
    
    
    #include <image.h> /* load_executable() */
    #include <OS.h> /* wait_for_thread() */
    #include <stdlib.h> /* malloc() */
    
    char **arg_v; /* choose a name that doesn't collide with argv */
    int32 arg_c; /* same here vis a vis argc */
    thread_id exec_thread;
    int32 return_value;

Install, in the `arg_v` array, the "command line" arguments. Let's pretend we're launching a program found in `/boot/home/apps/adder` that takes two integers, adds them together, and returns the result as `main()`'s exit code. Thus, there are three arguments: The name of the program, and the values of the two addends converted to strings. Since there are three arguments, we allocate `arg_v` to hold four pointers (to accommodate the final `NULL`). Then we allocate and copy the arguments.
    
    
    arg_c = 3;
    arg_v = (char **)malloc(sizeof(char *) * (arg_c + 1));
    
    arg_v[0] = strdup("/boot/home/apps/adder");
    arg_v[1] = strdup("5");
    arg_v[2] = strdup("3");
    arg_v[3] = NULL;

Now that everything is properly set up, we call [`load_image()`](TheKernelKit_Images.md#load_image "load_image\(\)"). After the function returns, it's safe to free the allocated `arg_v` array:
    
    
    exec_thread = load_image(arg_c, arg_v, environ);
    
    while (--arg_c >= 0)
       free(arg_v[arg_c]);
    
    free(arg_v);

At this point, `exec_thread` is suspended (the natural state of a newly-spawned thread). In order to retrieve its return value, we use [`wait_for_thread()`](TheKernelKit_ThreadsAndTeams.md#wait_for_thread "wait_for_thread\(\)") to tell the thread to run:
    
    
    wait_for_thread(exec_thread, &return_value);

After [`wait_for_thread()`](TheKernelKit_ThreadsAndTeams.md#wait_for_thread "wait_for_thread\(\)") returns, the value of `return_value` should be 8 (i.e. 5 + 3).

* * *

### Creating a Shared Library

The primary documentation for creating a shared library is provided by MetroWerks in their CodeWarrior manual. Beyond the information that you find there, you should be aware of the following amendments and caveats:

  * You mustn't export your library's symbols through the `-export` all linker flag. Instead, use the __declspec() directive to export each symbol. The macro is described below. If you're compiling for the PPC, you must also export #pragma symbols; to do this from the BeIDE, go to the Linker/PEF portion of the Settings window and set "Export Symbols" to "Use #pragma".

  * The loader looks for libraries by following the `LIBRARY_PATH` environment variable. The default library path looks like this:
        
        $ echo $LIBRARY_PATH
        %A/lib:/boot/home/config/lib:/boot/beos/system/lib

where "%A" means the directory that contains the app that the user is lauching.

#### Exporting and Importing Symbols

If you're developing a shared library you should explicitly export every global symbol in your library by using the __declspec() macro. To export a symbol, you declare it thus…
    
    
    __declspec(dllexport) type name

…where __declspec(dllexport) is literal, and type and name declare the symbol. Some examples:
    
    
    __declspec(dllexport) char *some_name;
    __declspec(dllexport) void some_func() {...}
    class __declspec(dllexport) MyView {...}

To import these symbols, an app that wants to use your library must "reverse" the declaration by replacing dllexport with dllimport:
    
    
    __declspec(dllimport) char *some_name;
    __declspec(dllimport) void some_func();
    class __declspec(dllimport) MyView;

The trouble with this system is that it implies two sets of headers, one for exporting (for building your library) and another for importing (that the library client would use). The Be libraries use macros, defined in `BeBuild.h`, that throw the import/export switch so the header files can be unified. For example, here's the macro for libbe:
    
    
    #if _BUILDING_be
    #define _IMPEXP_BE   __declspec(dllexport)
    #else
    #define _IMPEXP_BE __declspec(dllimport)
    #endif

When libbe is being built, a private compiler directive defines _BUILDING_be to be non-zero, and _IMPEXP_BE exports symbols. When a developer includes `BeBuild.h`, the _BUILDING_be variable is set to zero, so _IMPEXP_BE is set to import symbols.

You may want to emulate this system by defining macros for your own libraries. This implies that you have to define a compiler switch (analogous to _BUILDING_be) yourself. Set the switch to non-zero when you're building your library, and then set it to zero when you publish your headers for use by library clients.

* * *

### Creating and Using an Add-on Image

An add-on image is indistinguishable from a shared library image. Creating an add-on is exactly like creating a shared library, a topic that we breezed through above, but with a couple of minor tweaks:

  * The loader looks for add-ons by following the paths in the `ADDON_PATH` environment variable. The default `ADDON_PATH` looks like this:
        
        $ echo $ADDON_PATH
        %A/add-ons:/boot/home/config/add-ons:/boot/beos/system/add-ons

  * You have to export your add-on symbols, and you also must extern "C" them. This ensures that the symbol names won't be mangled by the compiler.

#### Exporting Add-on Symbols

To export your add-on's symbols, declare them thus:
    
    
    extern "C" __declspec(dllexport) void some_func();
    extern "C" __declspec(dllexport) int32 some_global_data;

To extern a C++ class takes more work. You can't extern the class directly; typically what you do is create (and extern) a C function that covers the class constructor:
    
    
    extern "C" __declspec(dllexport) MyClass *instantiate_my_class();

`instantiate_my_class()` is implemented to call the `MyClass` constructor:
    
    
    MyClass *instantiate_my_class()
    {
        return new MyClass();
    }

#### Loading an Add-on Image

To load an add-on into your application, you call the [`load_add_on()`](TheKernelKit_Images.md#load_add_on "load_add_on\(\), unload_add_on\(\)") function. The function takes a pathname (absolute or relative to the current working directory) to the add-on file, and returns an image_id number that uniquely identifies the image across the entire system.

For example, let's say you've created an add-on image that's stored in the file `/boot/home/add-ons/adder`. The code that loads the add-on would look like this:
    
    
    /* For brevity, we won't check errors. */
    image_id addon_image;
    
    /* Load the add-on. */
    addon_image = load_add_on("/boot/home/add-ons/adder");

Unlike loading an executable, loading an add-on doesn't create a separate team, nor does it spawn another thread. The whole point of loading an add-on is to bring the image into your application's address space so you can call the functions and fiddle with the variables that the add-on defines.

#### Symbols

After you've loaded an add-on into your application, you'll want to examine the symbols (variables and functions) that it has brought with it. To get information about a symbol, you call the [`get_image_symbol()`](TheKernelKit_Images.md#get_image_symbol "get_image_symbol\(\), get_nth_image_symbol\(\)") function:
    
    
    status_t get_image_symbol(image_id image,
             char *symbol_name,
             int32 symbol_type,
             void **location)

The function's first three arguments identify the symbol that you want to get:

  * The first argument is the image_id of the add-on that owns the symbol.

  * The second argument is the symbol's name. This assumes, of course, that you know the name, and that the add-on has declared the name as extern. In general, using an add-on implies just this sort of cooperation.

  * The third argument is a constant that gives the symbol's symbol type. There are three types, as given below. If the executable format doesn't distinguish between text and data symbols, then you can use any of these three types—they'll all be the same. If the format does distinguish between text and data, then you can either ask for the specific type, or you can ask for `B_SYMBOL_TYPE_ANY`.

Constant| Description  
---|---  
`B_SYMBOL_TYPE_DATA`| Global data (variables)  
`B_SYMBOL_TYPE_TEXT`| Functions  
`B_SYMBOL_TYPE_ANY`| The symbol lives anywhere  
  

The function returns, by reference in its final argument, a pointer to the symbol's address. For example, let's say the adder add-on code looks like this:
    
    
    extern "C" int32 a1 = 0;
    extern "C" int32 a2 = 0;
    extern "C" int32 adder(void);
    
    int32 adder(void)
    {
       return (a1 + a2);
    }

To examine the variables (`a1` and `a2`), you would call [`get_image_symbol()`](TheKernelKit_Images.md#get_image_symbol "get_image_symbol\(\), get_nth_image_symbol\(\)") thus:
    
    
    int32 *var_a1, *var_a2;
    
    get_image_symbol(addon_image, "a1", B_SYMBOL_TYPE_DATA, &var_a1);
    get_image_symbol(addon_image, "a2", B_SYMBOL_TYPE_DATA, &var_a2);

Here we get the symbol for the `adder()` function:
    
    
    int32 (*func_add)();
    get_image_symbol(addon_image, "adder", B_SYMBOL_TYPE_TEXT,
                     &func_add);

Now that we've retrieved all the symbols, we can set the values of the two addends and call the function:
    
    
    *var_a1 = 5;
    *var_a2 = 3;
    int32 return_value = (*func_add)();

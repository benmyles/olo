<!-- Special Topics - Device Drivers - Using Modules -->

## Using Modules

Modules provide a means for multiple drivers to share common functionality; for example, if a variety of types of device might be accessed on the same bus, a module might be created to provide a common interface to the bus.

Your driver can access these modules via the kernel functions [`get_module()`](DeviceDrivers_WritingModules.md#get_module "get_module") and [`put_module()`](DeviceDrivers_WritingModules.md#put_module "put_module"), which obtain and release references to a specified module. When you call [`get_module()`](DeviceDrivers_WritingModules.md#get_module "get_module"), you obtain a structure that provides information about the module, plus pointers to the module's functions. The module is defined in a header file provided by the module's author, similar to this:
    
    
    #define MY_MODULE_NAME "generic/mymodule/v1"
    
    struct my_module_info {
       module_info module;
       int32 (*function1)();
       int32 (*function2)();
       void (*configure)(int32 parameter, int32 value);
    };

When you want to access the module's functions, you call get_module() to get a pointer to this structure from the kernel:
    
    
    struct my_module_info*minfo = NULL;
    
    /* get a pointer to the module */
    
    get_module(MY_MODULE_NAME, (module_info**) &minfo);

Once you've done this, you can call the module's functions through the structure:
    
    
    minfo->configure(0, 10);

When you're done with the module, you should call [`put_module()`](DeviceDrivers_WritingModules.md#put_module "put_module") to release it. The kernel loads and unloads modules as needed, and properly calling [`put_module()`](DeviceDrivers_WritingModules.md#put_module "put_module") lets the kernel do its job.
    
    
    put_module(MY_MODULE_NAME);

If you want a better understanding of how modules work, see the "[Writing Modules](DeviceDrivers_WritingModules.md "Writing Modules")" section.

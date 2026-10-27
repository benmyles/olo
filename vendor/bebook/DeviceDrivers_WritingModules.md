<!-- Special Topics - Device Drivers - Writing Modules -->

## Writing Modules

Modules provide services that can be used by other modules, by device drivers, and by the kernel itself. They can be dynamically loaded and unloaded by the kernel, as needed. If a client can't find a module it needs, it will still load, which gives it the opportunity to find another way to perform the desired tasks, or to disable those features of itself.

Modules, like drivers, export an API through a structure that provides pointers to the functions provided by the module, along with other information about the module. You do this by expanding upon the basic module definition in `drivers/module.h`. For example, you might define your module information structure like this:
    
    
    #define MY_MODULE_NAME "generic/mymodule/v1"
    
    struct my_module_info {
       module_info module;
       int32 (*function1)();
       int32 (*function2)();
       void (*configure)(int32 parameter, int32 value);
    };

Note that the first field in your module information structure is a module_info, which looks like this:
    
    
    struct module_info {
       const char*name;
       uint32 flags;
       status_t (*std_ops);
    };

The `name` field should be a pointer to the driver's name as indicated in your module's header file (in this example, `MY_MODULE_NAME`).

The `flags` field specifies which flags should be in effect for your module. Currently, the `B_KEEP_LOADED` flag is the only one available; as expected, it tells the kernel not to unload your module when nobody is using it; normally, the first time your module is requested by someone calling [`get_module()`](DeviceDrivers_WritingModules.md#get_module "get_module"), the kernel loads it. With each subsequent call to [`get_module()`](DeviceDrivers_WritingModules.md#get_module "get_module"), a reference count is incremented. Every time [`put_module()`](DeviceDrivers_WritingModules.md#put_module "put_module") is called to release the module, the reference count is decremented. When the counter reaches zero, the module is unloaded. `B_KEEP_LOADED` prevents unloading from taking place.

`std_ops` is a pointer to a function that your module must provide. This function is called to handle standard module operations. Currently, there are only two of these operations (initialization and uninitialization). Your module's `std_ops()` function will probably look something like this:
    
    
    static status_t std_ops(int32 op, ...) {
       switch(op) {
          case B_MODULE_INIT:
             /* do whatever you need to do */
             break;
          case B_MODULE_UNINIT:
             /* do whatever you need to do */
             break;
          default:
             return B_ERROR;   /* necessary, for future expansion */
       }
       return B_OK;
    }

It's important to return `B_ERROR` for any unknown operations, in case future versions of the kernel define additional operations.

Exporting your module to the outside world is similar to publishing device driver hooks, but since you define the hooks yourself, it's slightly more involved. Your module needs to have a filled-out version of your module's information structure, like this:
    
    
    static struct my_module_info my_module {
       {
          MY_MODULE_NAME,   /* module name */
          0,   /* flags */
          std_ops
       },
       function1,
       function2,
       configure
    };

When loading your module, the kernel looks for a symbol called "modules" that contains a list of pointers to the modules you export, terminated by a `NULL`:
    
    
    _EXPORT module_info*modules[] = {
       (module_info*) &my_module,
       NULL
    };

This is how the kernel finds out what modules are available for use by drivers (or by other modules). See the "[Using Modules](DeviceDrivers_UsingModules.md "Using Modules")" section for details on how modules are accessed by other drivers or modules.

* * *

### Module Functions

#### get_module

`status_t get_module(const char* path,  
module_info** info);`

Declared in: `drivers/module.h`

The `get_module()` function obtains infomation about the module at the `path` specified and increases the modules internal reference count.

#### put_module

`status_t put_module(const char* path);`

Declared in: `drivers/module.h`

This function decreases the modules internal reference count and if the count is zero the module is unloaded from memory

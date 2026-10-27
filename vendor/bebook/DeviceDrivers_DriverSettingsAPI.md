<!-- Special Topics - Device Drivers - Driver Settings API -->

## Driver Settings API  
  
Declared in: `drivers/driver_settings.h`

If your driver is loaded before the file system for the disk on which your settings file resides, your driver might not be able to load its settings using Posix calls. Also, a robust method for reading settings files—even if they might have become corrupted—can help the system be more stable; if your driver crashes trying to read its settings, the entire system is in jeopardy.

The driver settings API provides easy, safe access to boolean and string settings, and is available to all drivers and modules. If your driver has more complex settings, the [`get_driver_settings()`](DeviceDrivers_DriverSettingsAPI.md#get_driver_settings "get_driver_settings\(\)") function is available to retrieve all your settings in a hierarchical tree.

The boot loader reads the settings files from the boot volume and passes them to the kernel for distribution to the drivers upon request. The boot loader also lets the user add to these settings at boot time; a line of the form "filename:parameters" in the advanced safe mode menu will add "parameters" to the end of the specified settings file. This can be used to change debugging information and to test different options while developing your driver.

* * *

### Using the Driver Settings API

Using the API is very simple. Just follow these basic steps:

  * Call [`load_driver_settings()`](DeviceDrivers_DriverSettingsAPI.md#load_driver_settings "load_driver_settings\(\), unload_driver_settings\(\)") to load the settings data.

  * Use [`get_driver_settings()`](DeviceDrivers_DriverSettingsAPI.md#get_driver_settings "get_driver_settings\(\)") or [`get_driver_parameter()`](DeviceDrivers_DriverSettingsAPI.md#get_driver_parameter "get_driver_parameter\(\)") and [`get_driver_boolean_parameter()`](DeviceDrivers_DriverSettingsAPI.md#get_driver_boolean_parameter "get_driver_boolean_parameter\(\)") to read the settings.

  * Call [`unload_driver_settings()`](DeviceDrivers_DriverSettingsAPI.md#unload_driver_settings) when you're done.

#### The Settings File

Driver settings files are kept in `~/config/settings/kernel/drivers`.

The settings file is formatted like this:

  * Words beginning with "#" indicate that the rest of the line should be treated as a comment.

  * Parameters can have values and subparameters. A parameter has the following form in the settings file:
        
        name [value]* [{
        [parameter]*
        }] ['n',',']

Where [ … ] indicates an optional part, and [ … ]* indicates an optional repeated part.

  * Names and values may not contain spaces unless the spaces are preceded by a backslash ('\') or the words are enclosed in quotes.

Here's an example settings file:
    
    
    device 0 {
       attribute1 value
       attribute2 value
    }
    device 1 {
       attribute1 value
    }
    

For this settings file, [`get_driver_settings()`](DeviceDrivers_DriverSettingsAPI.md#get_driver_settings "get_driver_settings\(\)") will return a pointer to the following tree:
    
    
    driver_settings = {
       parameter_count = 2
       parameters = {
          name = "device"
          value_count = 1
          values = { "0" }
          parameter_count = 2
          parameters = {
             name = "attributes1"
             value_count = 1
             values = "value"
             parameter_count = 0
             parameters = NULL
          },
          {
             name = "attribute2"
             value_count = 1
             values = "value"
             parameter_count = 0
             parameters = NULL
          }
       },
       {
          name = "device"
          value_count = 1
          values = { "1" }
          parameter_count = 1
          parameters = {
             name = "attribute1"
             value_count = 1
             values = "value"
             parameter_count = 0
             parameters = NULL
          }
       }
    }
    

#### Loading the Settings

To load the driver's settings, you need to call [`load_driver_settings()`](DeviceDrivers_DriverSettingsAPI.md#load_driver_settings "load_driver_settings\(\), unload_driver_settings\(\)"). For example, if your driver's name is "xr_joystick", you might do this:
    
    
    void*handle = load_driver_settings("xr_joystick");

The `handle` is then used when calling the other driver settings functions, to indicate which driver's settings you want to reference. This opaque reference protects you against any future changes in the kernel.

#### Reading the Settings

There are three functions you can use to read driver settings:

  * [`get_driver_boolean_parameter()`](DeviceDrivers_DriverSettingsAPI.md#get_driver_boolean_parameter "get_driver_boolean_parameter\(\)") returns a boolean parameter's value.

  * [`get_driver_parameter()`](DeviceDrivers_DriverSettingsAPI.md#get_driver_parameter "get_driver_parameter\(\)") returns a string parameter's value.

  * [`get_driver_settings()`](DeviceDrivers_DriverSettingsAPI.md#get_driver_settings "get_driver_settings\(\)") returns all the settings at once, encapsulated in a hierarchical format.

##### Reading a Boolean Parameter

Let's look at a simple driver that has one boolean parameter, "debug", that enables a special debug mode. The value of this parameter is represented in the settings file by a line "debug value", where value is either "true" or "false". By default, if there's no setting for the debug parameter, false should be assumed. If the parameter is specified but no value is included, we want to assume that the user means true.

Our code to read this setting looks like this:
    
    
    void*handle = load_driver_settings("xr_joystick");
    bool debug = get_driver_boolean_parameter(handle, "debug",
                                              false, true);
    unload_driver_settings(handle);

If there's no settings file, [`load_driver_settings()`](DeviceDrivers_DriverSettingsAPI.md#load_driver_settings "load_driver_settings\(\), unload_driver_settings\(\)") will return `NULL`. In this case, [`get_driver_boolean_parameter()`](DeviceDrivers_DriverSettingsAPI.md#get_driver_boolean_parameter "get_driver_boolean_parameter\(\)") will return `false` (the value we're passing as the `unknownValue` argument).

If there's a settings file, but the debug entry isn't found, the `unknownValue` argument is returned. Even though the handle is valid, the function can't find a value for that argument, so it uses this as the default.

If the file contains a line starting with "debug", the second word on the line is used as the value. If no value is specified, `true` is returned (the value of the `noArgValue` argument to [`get_driver_boolean_parameter()`](DeviceDrivers_DriverSettingsAPI.md#get_driver_boolean_parameter "get_driver_boolean_parameter\(\)")). Otherwise the following is done:

  * If the value is "1", "true", "yes", "on", "enable", or "enabled", `true` is returned.

  * If the value is "0", "false", "no", "off", "disable", or "disabled", `false` is returned.

  * If the value matches none of these strings, it's treated as if no entry were found, and `unknownValue` is returned.

If more than one line containing the word "debug" is found, the last one in the file is used. This lets the user override, at boot time, the value previously specified in the settings file.

##### Reading a String Parameter

Reading string parameters works in much the same way, using the [`get_driver_parameter()`](DeviceDrivers_DriverSettingsAPI.md#get_driver_parameter "get_driver_parameter\(\)") function. The only difference is that the string returned will be `NULL` if the parameter is missing, or the file doesn't exist.

##### Reading All Parameters

If your driver has more complex parameters (such as parameters with multiple values, or with subparameters), you can read the entire settings tree using the [`get_driver_settings()`](DeviceDrivers_DriverSettingsAPI.md#get_driver_settings "get_driver_settings\(\)") function.

The driver_settings structure contains the root of the settings tree:
    
    
    typedef struct driver_settings {
        int parameter_count;
        struct driver_parameter*parameters;
    };

Each parameter is described by the driver_parameter structure:
    
    
    typedef struct driver_parameter {
        char*name;
        int value_count;
        char**values;
        int parameter_count;
        struct driver_parameter*parameters;
    };

* * *

### C Functions

#### get_driver_boolean_parameter()
    
    
    bool get_driver_boolean_parameter(void*handle,
        const char*keyName,
        bool unknownValue,
        bool noArgValue)

Returns the value of a given boolean parameter. The driver settings file is specified by the handle, as returned by [`load_driver_settings()`](DeviceDrivers_DriverSettingsAPI.md#load_driver_settings "load_driver_settings\(\), unload_driver_settings\(\)"). The parameter's name is given by `keyName`. If the parameter isn't found, `unknownValue` is returned. If the parameter exists but has no value, `noArgValue` is returned. This lets you easily deal with these two conditions, providing appropriate default values without additional code to check for error conditions.

If the `handle` is `NULL`, `unknownValue` is returned.

#### get_driver_parameter()
    
    
    const char*get_driver_parameter(void*handle,
        const char*keyName,
        const char*unknownValue,
        const char*noArgValue)

Returns the value of a given string parameter. The driver settings file is specified by the `handle`, as returned by [`load_driver_settings()`](DeviceDrivers_DriverSettingsAPI.md#load_driver_settings "load_driver_settings\(\), unload_driver_settings\(\)"). The parameter's name is given by `keyName`. If the parameter isn't found, `unknownValue` is returned. If the parameter exists but has no value, `noArgValue` is returned. This lets you easily deal with these two conditions, providing appropriate default values without additional code to check for error conditions.

The special `keyName` value `B_SAFEMODE_SAFE_MODE` can be used if you want to find out whether or not BeOS was booted in safe mode; the value will be `true` if BeOS is running in safe mode, or false if a normal boot was performed.

If the `handle` is `NULL`, `unknownValue` is returned.

#### get_driver_settings()
    
    
    const driver_settings*get_driver_settings(void*handle)

Returns the values of all parameters in encapsulated form.

####  load_driver_settings(), unload_driver_settings()
    
    
    void*load_driver_settings(const char*driverName)
    status_t unload_driver_settings(void*handle)

`load_driver_settings()` loads the settings for the driver specified by `driverName`, and returns a handle that should be used for calls to other driver settings functions. If you want to access the safe mode settings, pass `B_SAFEMODE_DRIVER_SETTINGS` Returns `NULL` if no settings are available for the driver.

`unload_driver_settings()` unloads the settings for the driver whose settings file is specified by `handle`. You should always call this function when you're done reading the settings.

* * *

### Defined Types

#### driver_parameter
    
    
    typedef struct driver_parameter {
        char*name;
        int value_count;
        char**values;
        int parameter_count;
        struct driver_parameter*parameters;
    };

Describes a subtree of parameters.

#### driver_settings
    
    
    typedef struct driver_settings {
        int parameter_count;
        struct driver_parameter*parameters;
    };

Encapsulates all the settings for a driver.

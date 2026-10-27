<!-- System Overview - The Input Server - BInputServerFilter -->

## BInputServerFilter

[`BInputServerFilter`](BInputServerFilter.md "BInputServerFilter") is a base class for input filters; these are instances of [`BInputServerFilter`](BInputServerFilter.md "BInputServerFilter") that modify, generate, or eat input events. An input filter add-on is privy to all the events that pass through the Input Server's event stream. A filter is similar to the Interface Kit's [`BMessageFilter`](BMessageFilter.md "BMessageFilter"), but at a much lower level. The [`BInputServerFilter`](BInputServerFilter.md "BInputServerFilter") also sees all events, while a [`BMessageFilter`](BMessageFilter.md "BMessageFilter") only sees the events targeted at its [`BLooper`](BLooper.md "BLooper"). [`BMessageFilter`](BMessageFilter.md "BMessageFilter")s can also generate additional events in place of, or in addition to, the original input event.

[`BInputServerFilter`](BInputServerFilter.md "BInputServerFilter") objects are created and deleted by the Input Server only—you never create or delete these objects in your code.

* * *

### Creating

To create a new input filter, you must:

  * create a subclass of [`BInputServerFilter`](BInputServerFilter.md "BInputServerFilter")

  * implement the [`instantiate_input_filter()`](TheInputServer_Functions.md#instantiate_input_filter "instantiate_input_filter\(\)") C function to create an instance of your [`BInputServerFilter`](BInputServerFilter.md "BInputServerFilter") subclass

  * compile the class and function as an add-on

  * install the add-on in one of the input filter directories

At boot time (or whenever the Input Server is restarted; see "[Loading](TheInputServer_Introduction.md#TheInputServer_Introduction_Loading "Loading")" in The Input Server), the Input Server loads the add-ons it finds in the input filter directories. For each add-on it finds, the Server invokes [`instantiate_input_filter()`](TheInputServer_Functions.md#instantiate_input_filter "instantiate_input_filter\(\)") to get a pointer to the add-ons's [`BInputServerFilter`](BInputServerFilter.md "BInputServerFilter") object. After constructing the object, the Server calls [`InitCheck()`](BInputServerFilter.md#BInputServerFilter_InitCheck "InitCheck\(\)") to give the add-on a chance to bail out if the constructor failed.

* * *

### Installing an Input Filter

The input server looks for input filters in the `input_server/filters` subdirectories of `B_BEOS_ADDONS_DIRECTORY`, `B_COMMON_ADDONS_DIRECTORY`, and `B_USER_ADDONS_DIRECTORY`.

  * You can install your input devices in the latter two directories—i.e. those under `B_COMMON_ADDONS_DIRECTORY`, and `B_USER_ADDONS_DIRECTORY`.

  * The `B_BEOS_ADDONS_DIRECTORY` is reserved for add-ons that are supplied with BeOS.

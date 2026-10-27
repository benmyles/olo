<!-- System Overview - The Input Server - BInputDevice -->

## BInputDevice

A [`BInputDevice`](BInputDevice.md "BInputDevice") object is a "downstream" representation of an Input Server device, such as a mouse or a keyboard, within a "regular" application. The [`BInputDevice`](BInputDevice.md "BInputDevice") can [`Start()`](BInputDevice.md#BInputDevice_Start "Start\(\), Stop\(\), IsRunning\(\)") and [`Stop()`](BInputDevice.md#BInputDevice_Stop) the device it represents, and can send it input device control messages through its [`Control()`](BInputDevice.md#BInputDevice_Control "Control\(\)") function.

You never create [`BInputDevice`](BInputDevice.md "BInputDevice") objects yourself; instead, you ask the system to return one or more instances to you through the [`find_input_device()`](BInputDevice.md#find_input_device "find_input_device\(\), get_input_devices\(\)") or [`get_input_devices()`](BInputDevice.md#get_input_devices) functions. Alternatively, you can work without an object by invoking the static versions of [`Start()`](BInputDevice.md#BInputDevice_Start "Start\(\), Stop\(\), IsRunning\(\)"), [`Stop()`](BInputDevice.md#BInputDevice_Stop), and [`Control()`](BInputDevice.md#BInputDevice_Control "Control\(\)"). Note, however, that the static functions control all devices of a given type, whereas a [`BInputDevice`](BInputDevice.md "BInputDevice") instance can talk to a specific device.

[`BInputDevice`](BInputDevice.md "BInputDevice") objects don't live in the Input Server—they're used in "normal" applications as a means to control an Input Server device add-on.

The [`BInputDevice`](BInputDevice.md "BInputDevice") object is provided, primarily, to let an application talk to a custom input device.

You never subclass [`BInputDevice`](BInputDevice.md "BInputDevice").

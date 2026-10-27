<!-- System Overview - The Media Kit - BParameterWeb -->

## BParameterWeb

[`BParameterWeb`](BParameterWeb.md "BParameterWeb") serves as a container for information describing the relationships among the configurable parameters of a [`BControllable`](BControllable.md "BControllable") node. A [`BControllable`](BControllable.md "BControllable") subclass should, when constructed, create a [`BParameterWeb`](BParameterWeb.md "BParameterWeb") and populate it with controls through one or more [`BParameterGroup`](BParameterGroup.md "BParameterGroup")s. This web of parameters describes the signal path within the node, and the points at which the node's manipulation of the data can be controlled.

The Audio control panel is actually derived from the [`BParameterWeb`](BParameterWeb.md "BParameterWeb") of the node currently selected as the default audio input node.

The [`BParameterWeb`](BParameterWeb.md "BParameterWeb") lets client applications query a node to determine how it can be configured, so the application can then create and display a user interface permitting the user to configure the node using standard user interface objects (like sliders, checkboxes, lines and arrows indicating the data path, and so forth).

By using the parameter web, device-independence is maintained without sacrificing the ability to create generic code to configure devices.

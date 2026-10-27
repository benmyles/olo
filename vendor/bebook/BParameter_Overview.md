<!-- System Overview - The Media Kit - BParameter -->

## BParameter

The [`BParameter`](BParameter.md "BParameter") class is the superclass from which all controls representing configurable parameters within a [`BControllable`](BControllable.md "BControllable") node are derived.

Note

![Note](./images/admonitions/Info_32.png)

The word "parameter," in terms of the Media Kit, refers to a point at which the data manipulation performed by a [`BControllable`](BControllable.md "BControllable") node can be configured, and doesn't actually refer to a user interface object. [`BParameter`](BParameter.md "BParameter")-derived objects don't implement a user interface. See the [`BMediaTheme`](BMediaTheme.md "BMediaTheme") class for information on how to create a user interface for a [`BControllable`](BControllable.md "BControllable") node.

The [`BParameter`](BParameter.md "BParameter") class is an abstract class; you'll normally use the [`MakeNullParameter()`](BParameterGroup.md#BParameterGroup_MakeNullParameter "MakeNullParameter\(\)"), [`MakeDiscreteParameter()`](BParameterGroup.md#BParameterGroup_MakeDiscreteParameter "MakeDiscreteParameter\(\)"), and [`MakeContinuousParameter()`](BParameterGroup.md#BParameterGroup_MakeContinuousParameter "MakeContinuousParameter\(\)") functions in the [`BParameterGroup`](BParameterGroup.md "BParameterGroup") class to actually create parameters and add them to a [`BParameterWeb`](BParameterWeb.md "BParameterWeb"). For more detailed information on parameters and how to create a [`BParameterWeb`](BParameterWeb.md "BParameterWeb"), see the [`BParameterGroup`](BParameterGroup.md "BParameterGroup") and [`BParameterWeb`](BParameterWeb.md "BParameterWeb") classes.

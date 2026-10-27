<!-- System Overview - The Media Kit - BParameterGroup -->

## BParameterGroup

A [`BParameterGroup`](BParameterGroup.md "BParameterGroup") object serves as a logical grouping of [`BParameter`](BParameter.md "BParameter")s. Each [`BParameter`](BParameter.md "BParameter") belongs to exactly one [`BParameterGroup`](BParameterGroup.md "BParameterGroup"), and is in fact created by the [`BParameterGroup`](BParameterGroup.md "BParameterGroup") object to which it belongs, via the [`BParameterWeb::MakeGroup()`](BParameterWeb.md#BParameterWeb_MakeGroup "MakeGroup\(\)") function.

[`BParameterGroup`](BParameterGroup.md "BParameterGroup")s can be nested one inside another.

A [`BParameterGroup`](BParameterGroup.md "BParameterGroup") object that's owned by a [`BParameterWeb`](BParameterWeb.md "BParameterWeb") is a logical candidate for a pane in a multi-pane control panel such as a [`BTabView`](BTabView.md "BTabView"), wherein each tab causes another [`BParameterGroup`](BParameterGroup.md "BParameterGroup")'s [`BParameter`](BParameter.md "BParameter")s to be displayed.

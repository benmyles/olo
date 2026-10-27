<!-- System Overview - The Storage Kit - BRefFilter -->

## BRefFilter

The `BRefFilter` class lets you filter the items that a file panel is about to display. This filtering is performed by the class' only function, [`Filter()`](BRefFilter.md#BRefFilter_Filter "Filter\(\)"). [`Filter()`](BRefFilter.md#BRefFilter_Filter "Filter\(\)") is a hook function; to use a `BRefFilter`, you have to create a derived class and implement the [`Filter()`](BRefFilter.md#BRefFilter_Filter "Filter\(\)") function.

To assign your `BRefFilter` object to a file panel, you invoke [`BFilePanel`](BFilePanel.md "BFilePanel")'s [`SetRefFilter()`](BFilePanel.md#BFilePanel_SetRefFilter "SetRefFilter\(\), RefFilter\(\)") function. (The [`BFilePanel`](BFilePanel.md "BFilePanel") constructor also lets you set the filter.) If you don't specifically assign a filter, the file panel will not have one—there is no "default" ref filter object. You maintain ownership of the `BRefFilter` that you assign to a file panel; the file panel doesn't delete or otherwise change your object.

You can assign the same filter to more than file panel. However, the [`Filter()`](BRefFilter.md#BRefFilter_Filter "Filter\(\)") function isn't told which panel it's being invoked for.

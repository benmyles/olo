<!-- System Overview - The Interface Kit - BControl -->

## BControl

[`BControl`](BControl.md "BControl") is an abstract class for views that draw control devices on the screen. Objects that inherit from [`BControl`](BControl.md "BControl") emulate, in software, real-world control devices—like the switches and levers on a machine, the check lists and blank lines on a form to fill out, or the dials and knobs on a home appliance.

Controls translate the messages that report generic mouse and keyboard events into other messages with more specific instructions for the application. A [`BControl`](BControl.md "BControl") object can be customized by setting the message it posts when invoked and the target object that should handle the message.

Controls also register a current value, stored as an int32 integer that's typically set to `B_CONTROL_ON` or `B_CONTROL_OFF`. The value is changed only by calling [`SetValue()`](BControl.md#BControl_SetValue "SetValue\(\), Value\(\)"), a virtual function that derived classes can implement to be notified of the change.

* * *

### Derived Classes

The Interface Kit currently includes six classes derived from [`BControl`](BControl.md "BControl") — [`BButton`](BButton.md "BButton"), [`BPictureButton`](BPictureButton.md "BPictureButton"), [`BRadioButton`](BRadioButton.md "BRadioButton"), [`BCheckBox`](BCheckBox.md "BCheckBox"), [`BColorControl`](BColorControl.md "BColorControl"), and [`BTextControl`](BTextControl.md "BTextControl"). In addition, it has two classes— [`BListView`](BListView.md "BListView") and [`BMenuItem`](BMenuItem.md "BMenuItem") —that implement control devices but are not derived from this class. [`BListView`](BListView.md "BListView") and its subclass, [`BOutlineListView`](BOutlineListView.md "BOutlineListView"), share an interface with the [`BList`](BList.md "BList") class (of the Support Kit) and [`BMenuItem`](BMenuItem.md "BMenuItem") is designed to work with the other classes in the menu system. Like [`BControl`](BControl.md "BControl"), [`BListView`](BListView.md "BListView") and [`BMenuItem`](BMenuItem.md "BMenuItem") inherit from the Application Kit's [`BInvoker`](BInvoker.md "BInvoker") class.

As [`BListView`](BListView.md "BListView") and [`BMenuItem`](BMenuItem.md "BMenuItem") demonstrate, it's possible to implement a control device that's not a [`BControl`](BControl.md "BControl"). However, it's simpler to take advantage of the code that's already provided by the [`BControl`](BControl.md "BControl") class. That way you can keep a simple programming interface and avoid reimplementing functions that [`BControl`](BControl.md "BControl") has defined for you. If your application defines its own control devices—dials, sliders, selection lists, and the like—they should be derived from [`BControl`](BControl.md "BControl").

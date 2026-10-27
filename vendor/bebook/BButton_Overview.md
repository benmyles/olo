<!-- System Overview - The Interface Kit - BButton -->

## BButton  
  
A [`BButton`](BButton.md "BButton") object draws a labeled button on-screen and responds when the button is clicked or when it's operated from the keyboard. If the [`BButton`](BButton.md "BButton") is the default button for its window and the window is the active window, the user can operate it by pressing the Enter key.

[`BButton`](BButton.md "BButton")s have a single state. Unlike check boxes and radio buttons, the user can't toggle a button on and off. However, the button's value changes while it's being operated. During a click (while the user holds the mouse button down and the cursor points to the button on-screen), the [`BButton`](BButton.md "BButton")'s value is set to 1 (`B_CONTROL_ON`). Otherwise, the value is 0 (`B_CONTROL_OFF`).

This class depends on the control framework defined in the [`BControl`](BControl.md "BControl") class. In particular, it calls these [`BControl`](BControl.md "BControl") functions:

[`SetValue()`](BControl.md#BControl_SetValue "SetValue\(\), Value\(\)"). To make each change in the [`BControl`](BControl.md "BControl")'s value.
    

This is a hook function that you can override to take collateral action when the value changes.

[`Invoke()`](BControl.md#BControl_Invoke "Invoke\(\)"). To post a message each time the button is clicked or operated from the keyboard.
    

You can designate the object that should handle the message by calling [`BControl`](BControl.md "BControl")'s [`SetTarget()`](BInvoker.md#BInvoker_SetTarget "SetTarget\(\), Target\(\), IsTargetLocal\(\), Messenger\(\)") function inherited from [`BInvoker`](BInvoker.md "BInvoker"). A model for the message is set by the [`BButton`](BButton.md "BButton") constructor (or by [`BControl`](BControl.md "BControl")'s [`SetMessage()`](BInvoker.md#BInvoker_SetMessage "SetMessage\(\), Message\(\), Command\(\)") function inherited from [`BInvoker`](BInvoker.md "BInvoker")).

[`IsEnabled()`](BControl.md#BControl_IsEnabled). To determine how the button should be drawn and whether it's enabled to post a message.
    

You can call [`BControl`](BControl.md "BControl")'s [`SetEnabled()`](BControl.md#BControl_SetEnabled "SetEnabled\(\), IsEnabled\(\)") to enable and disable the button.

A [`BButton`](BButton.md "BButton") is an appropriate control device for initiating an action. Use a [`BCheckBox`](BCheckBox.md "BCheckBox"), a [`BPictureButton`](BPictureButton.md "BPictureButton"), or [`BRadioButton`](BRadioButton.md "BRadioButton")s to set a state.

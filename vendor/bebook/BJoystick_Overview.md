<!-- System Overview - The Device Kit - BJoystick -->

## BJoystick

A [`BJoystick`](BJoystick.md "BJoystick") object provides an interface to a joystick (or other game controller) connected to the computer. The BeOS supports joysticks on the BeBox and Intel platforms.

The BeBox supports up to four analog joysticks, each of which can have up to two axes and two buttons; digital joysticks aren't supported by the built-in game ports. You can install a card that provides additional game ports (such as the Sound Blaster AWE64), and use digital joysticks on those ports.

BeOS supports joysticks through game ports on cards, as well as some built-in game ports.

Unlike the event and message-driven interface to the mouse and keyboard, the interface to a joystick is strictly demand-driven. An application must repeatedly poll the state of the joystick by calling the [`BJoystick`](BJoystick.md "BJoystick") object's [`Update()`](BJoystick.md#BJoystick_Update "Update\(\)") function. [`Update()`](BJoystick.md#BJoystick_Update "Update\(\)") queries the port and updates the object's data members to reflect the current state of the joystick.

There are two modes available. Standard mode supports only two axes per joystick, and two buttons per joystick. This mode has been available since the early days of the BeOS. You read the joystick in standard mode by looking at the [`BJoystick`](BJoystick.md "BJoystick") member variables `vertical` and `horizontal` to determine the joystick's axis values, and `button1` and `button2` to determine the state of the buttons.

Enhanced mode supports up to 32 buttons per joystick, and an indefinite number of axes and hats (thumb controls, usually located at the top of a stick). It also supports multiple joysticks chained to a single game port. Instead of reading variables to determine the state of the joystick, there are several functions provided to let you do this.

In addition, enhanced mode provides a mechanism for determining what joysticks are available and what types of (and how many) controls are available on the joysticks. There's also a preference application

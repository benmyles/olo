<!-- System Overview - The Interface Kit - BPictureButton -->

## BPictureButton

A [`BPictureButton`](BPictureButton.md "BPictureButton") draws a button that displays an image (a [`BPicture`](BPicture.md "BPicture") object) rather than a text label. A [`BPictureButton`](BPictureButton.md "BPictureButton") can be either a one-state or a two-state control :

One-state.| Acts like a normal [`BButton`](BButton.md "BButton"): The value is `B_CONTROL_ON` while the object is being pressed and `B_CONTROL_OFF` otherwise. The images for these two states should be different; by convention, the "on" image is a highlighted version of the "off" image. If the button can be disabled, it needs an additional "disabled" image.  
---|---  
Two-state.| Acts like a checkbox: Its value switches between `B_CONTROL_ON` and `B_CONTROL_OFF` every time the user presses (and releases) the button. If the button can be disabled, it needs two additional "disabled" images: One for disabled-while-on and the other for disabled-while-off.

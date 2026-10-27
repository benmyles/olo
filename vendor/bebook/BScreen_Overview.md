<!-- System Overview - The Interface Kit - BScreen -->

## BScreen

A [`BScreen`](BScreen.md "BScreen") object represents a single display screen that's connected to the computer. With a [`BScreen`](BScreen.md "BScreen") object you can…

  * Get and set the screen's size and pixel depth.

  * Get the screen's color map.

  * Make a screen shot.

  * Set the desktop color.

  * Synchronize your code with the screen's retrace event.

You can't copy a [`BScreen`](BScreen.md "BScreen") object—the copy constructor and assignment operators are private.

* * *

### Multiple Screens

Although, the BeOS currently only supports a single screen, in the future it will let the user hook up multiple screens. One of the screens, the main screen, will have the origin of the screen coordinate system at its left top corner. Other screens will be located elsewhere in the same coordinate system. If there's just one screen, it's the main screen.

A [`BScreen`](BScreen.md "BScreen") object represents one screen. An application can have more than one object referring to the same screen.

When multiple screens are supported, a screen_id identifier will be assigned to each one. Currently, `B_MAIN_SCREEN_ID` is the only identifier.

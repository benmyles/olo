<!-- Classes And Methods - The Game Kit - Global Functions -->

## Global Functions

* * *

### set_mouse_position()

Declared in: `game/WindowScreen.h`

`void set_mouse_position(int32 x,  
int32 y);`

Moves the cursor hot spot to (`x`, `y`) in the screen coordinate system, where `x` is a left-to-right index to a pixel column and `y` is a top-to-bottom index to a pixel row. The origin of the coordinate system is the left top pixel of the display area of the main screen.

This function should be called only while the application has a direct connection to the frame buffer through a [`BWindowScreen`](BWindowScreen.md "BWindowScreen") object.

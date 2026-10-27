<!-- System Overview - The Game Kit - BWindowScreen -->

## BWindowScreen

A [`BWindowScreen`](BWindowScreen.md "BWindowScreen") object provides exclusive access to the entire screen, bypassing the Application Server's window system. The object has direct access to the graphics card driver: It can set up the graphics environment on the graphics card, call driver-implemented drawing functions, and directly manipulate the frame buffer.

* * *

### Screen Access

Like all windows, a [`BWindowScreen`](BWindowScreen.md "BWindowScreen") is hidden (off-screen) when it's constructed. By calling [`Show()`](BWindowScreen.md#BWindowScreen_Show) to put it on-screen and make it the active window, an application takes over the whole screen. While the [`BWindowScreen`](BWindowScreen.md "BWindowScreen") is active, the Application Server's graphics operations are suspended—this means that you can't use any [`BView`](BView.md "BView") functions, nor any functions in classes derived from [`BView`](BView.md "BView"); you have to draw directly into the screen's frame buffer, and nothing except what the application draws will be visible to the user—no other windows and no desktop. When the [`BWindowScreen`](BWindowScreen.md "BWindowScreen") gives up active status, the Application Server automatically refreshes the screen with its old contents.

Although the [`BWindowScreen`](BWindowScreen.md "BWindowScreen") object provides a connection to the screen, you shouldn't draw from the [`BWindowScreen`](BWindowScreen.md "BWindowScreen")'s thread. Use the thread only to regulate the access of other threads to the frame buffer.

* * *

### Keyboard and Mouse

A [`BWindowScreen`](BWindowScreen.md "BWindowScreen") object remains a window while it has control of the screen; it stays attached to the Application Server and its message loop continues to function. It gets messages reporting the user's actions on the keyboard and mouse, just like any other active window. Because it covers the whole screen, it's notified of all mouse and keyboard events. You can attach filters to the window to get the messages as they arrive. Or you can call the Interface Kit's [`get_key_info()`](TheInputServer_Functions.md#get_key_info "get_key_info\(\)") function to poll the state of the keyboard and construct a nominal [`BView`](BView.md "BView") so that you can call [`GetMouse()`](BView.md#BView_GetMouse "GetMouse\(\)") to poll the mouse.

* * *

### Workspaces

This class respects workspaces. A [`BWindowScreen`](BWindowScreen.md "BWindowScreen") object releases its grip on the screen when the user turns to another workspace and reestablishes its control when the user returns to the workspace in which it's the active window.

* * *

### Debugging

A [`BWindowScreen`](BWindowScreen.md "BWindowScreen") object can be constructed in a debugging mode that lets you switch back and forth between the workspace in which the game is running and a workspace where error messages are printed. See the [constructor](BWindowScreen.md#BWindowScreen_Constructor "BWindowScreen\(\)") and the [`RegisterThread()`](BWindowScreen.md#BWindowScreen_RegisterThread "RegisterThread\(\), Suspend\(\), SuspensionHook\(\)") function for details.

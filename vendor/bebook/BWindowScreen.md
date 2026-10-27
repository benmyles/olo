<!-- Classes And Methods - The Game Kit - BWindowScreen -->

## BWindowScreen  
  
| Derived From:| [BWindow](BWindow.md "BWindow")  
---|---  
Mix-in Classes:| –  
Declared In:| `game/WindowScreen.h`  
Library:| `libgame.so`  
Allocation:| –  
[Class Overview](BWindowScreen_Overview.md "BWindowScreen")

  * [Constructor and Destructor](BWindowScreen.md#BWindowScreen_ConstructorDestructor)
  * [Hook Functions](BWindowScreen.md#BWindowScreen_HookFunctions)
  * [Member Functions](BWindowScreen.md#BWindowScreen_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BWindowScreen()

`BWindowScreen(const char* title,  
uint32 space,  
status_t* error,  
bool debugging = false);`

Initializes the `BWindowScreen` object by assigning the window a `title` and specifying a `space` configuration for the screen. The window won't have a visible border or a tab in which to display the title to the user. However, others—such as the Workspaces application—can use the title to identify the window.

The window is constructed to fill the screen; its frame rectangle contains every screen pixel when the screen is configured according to the `space` argument. That argument describes the pixel dimensions and bits-per-pixel depth of the screen that the `BWindowScreen` object should establish when it obtains direct access to the frame buffer. It should be one of the following constants:
    
    
    B_8_BIT_640x480    B_16_BIT_640x480    B_32_BIT_640x480
    B_8_BIT_800x600    B_16_BIT_800x600    B_32_BIT_800x600
    B_8_BIT_1024x768   B_16_BIT_1024x768   B_32_BIT_1024x768
    B_8_BIT_1152x900   B_16_BIT_1152x900   B_32_BIT_1152x900
    B_8_BIT_1280x1024  B_16_BIT_1280x1024  B_32_BIT_1280x1024
    B_8_BIT_1600x1200  B_16_BIT_1600x1200  B_32_BIT_1600x1200

These are the same constants that can be passed to [`set_screen_space()`](TheInterfaceKit_Functions.md#set_screen_space "set_screen_space\(\)"), the [Interface Kit](TheInterfaceKit.md "The Interface Kit") function that preference applications call to configure the screen.

The `space` configuration applies only while the `BWindowScreen` object is in control of the screen. When it gives up control, the previous configuration is restored.

The constructor assigns the window to the active workspace (`B_CURRENT_WORKSPACE`). It fails if another `BWindowScreen` object in any application is already assigned to the same workspace.

To be sure there wasn't an error in constructing the object, check the `error` argument. If all goes well, the constructor sets the `error` variable to `B_OK`. If not, it sets it to `B_ERROR`. If there's an error, it's likely to occur in this constructor, not the inherited [`BWindow`](BWindow.md "BWindow") constructor. Since the underlying window will probably exist, you'll need to instruct it to quit. For example:
    
    
    status_t error;
    MyWindowScreen* screen =
                new MyWindowScreen("Glacier", B_8_BIT_1024x768, &error);
    
    if ( error != B_OK )
       screen->PostMessage(B_QUIT_REQUESTED, screen);

If the `debugging` flag is `true`, the `BWindowScreen` is constructed in debugging mode. This modifies its behavior and enables three functions, [`RegisterThread()`](BWindowScreen.md#BWindowScreen_RegisterThread "RegisterThread\(\), Suspend\(\), SuspensionHook\(\)"), [`Suspend()`](BWindowScreen.md#BWindowScreen_Suspend), and [`SuspensionHook()`](BWindowScreen.md#BWindowScreen_SuspensionHook). The debugging regime is described under those functions.

See also: the [`BScreen`](BScreen.md "BScreen") class in the [Interface Kit](TheInterfaceKit.md "The Interface Kit")

#### ~BWindowScreen()

`virtual ~BWindowScreen();`

Closes the clone of the graphics card driver (through which the `BWindowScreen` object established its connection to the screen), unloads it from the application, and cleans up after it.

* * *

### Hook Functions

#### ScreenConnected()

`virtual void ScreenConnected(bool connected);`

Implemented by derived classes to take action when the application gains direct access to the screen and when it's about to lose that access.

This function is called with the `connected` flag set to `true` immediately after the `BWindowScreen` object becomes the active window and establishes a direct connection to the graphics card driver for the screen. At that time, the Application Server's connection to the screen is suspended; drawing can only be accomplished through the screen access that the `BWindowScreen` object provides.

`ScreenConnected()` is called with a flag of `false` just before the `BWindowScreen` object is scheduled to lose its control over the screen and the Application Server's control is reasserted. The `BWindowScreen`'s connection to the screen will not be broken until this function returns. It should delay returning until the application has finished all current drawing and no longer needs direct screen access.

Note that whenever `ScreenConnected()` is called, the `BWindowScreen` object is guaranteed to be connected to the screen; if `connected` is `true`, it just became connected, if `connected` is `false`, it's still connected but will be disconnected when the function returns.

Derived classes typically use this function to regulate access to the screen. For example, they may acquire a semaphore when the `connected` flag is `false`, so that application threads won't attempt direct drawing when the connection isn't in place, and release the semaphore for drawing threads to acquire when the flag is `true`. For example:
    
    
    void MyWindowScreen::ScreenConnected(bool connected)
    {
       if ( connected == false )
          acquire_sem(directDrawingSemaphore);
       else
          release_sem(directDrawingSemaphore);
    }

See also: [`Disconnect()`](BWindowScreen.md#BWindowScreen_Disconnect "Disconnect\(\)")

* * *

### Member Functions

#### CanControlFrameBuffer()

`bool CanControlFrameBuffer();`

Returns `true` if the graphics card driver permits applications to control the configuration of the frame buffer, and `false` if not. Control is exercised through these two functions:

  * [`SetFrameBuffer()`](BWindowScreen.md#BWindowScreen_SetFrameBuffer "SetFrameBuffer\(\)")

  * [`MoveDisplayArea()`](BWindowScreen.md#BWindowScreen_MoveDisplayArea "MoveDisplayArea\(\)")

A return of `true` means that these functions can communicate with the graphics card driver and at least the first will do something useful. A return of `false` means that neither of them will work.

#### CardHookAt()

`[graphics_card_hook](TheGameKit_DefinedTypes.md#graphics_card_hook "graphics_card_hook") CardHookAt(int32 index);`

Returns a pointer to the graphics card "hook" function that's located at `index` in its list of hook functions. The function returns `NULL` if the graphics card driver doesn't implement a function at that index or the index is out of range.

The hook functions provide accelerated drawing capabilities. They're documented under "[Graphics Card Hook Functions](TheGameKit_CardHooks.md "Graphics Card Hook Functions")". The first three hook functions (indices 0, 1, and 2) are not available through the Game Kit; if you pass an index of 0, 1, or 2 to `CardHookAt()`, it will return `NULL` even if the function is implemented.

An application can cache the pointers that `CardHookAt()` returns, but it should ask for a new set each time the depth or dimensions of the screen changes and each time the `BWindowScreen` object releases or regains control of the screen. You'd typically call `CardHookAt()` in your implementation of [`ScreenConnected()`](BWindowScreen.md#BWindowScreen_ScreenConnected "ScreenConnected\(\)").

#### CardInfo()

`[graphics_card_info](TheGameKit_DefinedTypes.md#graphics_card_info "graphics_card_info")* CardInfo();`

Returns a description of the current configuration of the graphics card, as kept by the driver for the card. The returned [graphics_card_info](TheGameKit_DefinedTypes.md#graphics_card_info "graphics_card_info") structure is defined in `addons/graphics/GraphicsCard.h`

Note

![Note](./images/admonitions/Info_32.png)

The information returned by this function is only valid when the `BWindowScreen` is connected to the display.

See also: [`FrameBufferInfo()`](BWindowScreen.md#BWindowScreen_FrameBufferInfo "FrameBufferInfo\(\)")

#### Disconnect()

`void Disconnect();`

Forces the `BWindowScreen` object to disconnect itself from the screen—to give up its authority over the graphics card driver, allowing the Application Server to reassert control. Normally, you'd disconnect the `BWindowScreen` only when hiding the game, reducing it to an ordinary window in the background, or quitting. The [`Hide()`](BWindowScreen.md#BWindowScreen_Hide "Hide\(\), Show\(\)") and [`Quit()`](BWindowScreen.md#BWindowScreen_Quit "Quit\(\)") functions automatically disconnect the `BWindowScreen` as part of the process of hiding and quitting. [`Disconnect()`](BWindowScreen.md#BWindowScreen_Disconnect "Disconnect\(\)") allows you to sever the connection before calling those functions.

Before breaking the screen connection, [`Disconnect()`](BWindowScreen.md#BWindowScreen_Disconnect "Disconnect\(\)") causes the `BWindowScreen` object to receive a [`ScreenConnected()`](BWindowScreen.md#BWindowScreen_ScreenConnected "ScreenConnected\(\)") notification with a flag of `false`. It doesn't return until [`ScreenConnected()`](BWindowScreen.md#BWindowScreen_ScreenConnected "ScreenConnected\(\)") returns and the connection is broken. [`Hide()`](BWindowScreen.md#BWindowScreen_Hide "Hide\(\), Show\(\)") and [`Quit()`](BWindowScreen.md#BWindowScreen_Quit "Quit\(\)") share this behavior.

#### FrameBufferInfo()

`[frame_buffer_info](TheGameKit_DefinedTypes.md#frame_buffer_info "frame_buffer_info")* FrameBufferInfo();`

Returns a pointer to the [frame_buffer_info](TheGameKit_DefinedTypes.md#frame_buffer_info "frame_buffer_info") structure that holds the driver's current conception of the frame buffer. The structure is defined in `addons/graphics/GraphicsCard.h`

Note

![Note](./images/admonitions/Info_32.png)

The information returned by this function is only valid if [`SetFrameBuffer()`](BWindowScreen.md#BWindowScreen_SetFrameBuffer "SetFrameBuffer\(\)") has been called.

See also: [`SetSpace()`](BWindowScreen.md#BWindowScreen_SetSpace "SetSpace\(\)"), [`SetFrameBuffer()`](BWindowScreen.md#BWindowScreen_SetFrameBuffer "SetFrameBuffer\(\)"), [`MoveDisplayArea()`](BWindowScreen.md#BWindowScreen_MoveDisplayArea "MoveDisplayArea\(\)"), [`CardInfo()`](BWindowScreen.md#BWindowScreen_CardInfo "CardInfo\(\)")

####  Hide(), Show()

`virtual void Hide();``virtual void Show();`

These functions augment their [`BWindow`](BWindow.md "BWindow") counterparts to make sure that the `BWindowScreen` is disconnected from the screen before it's hidden and that it's ready to establish a connection when it becomes the active window.

`Hide()` calls [`ScreenConnected()`](BWindowScreen.md#BWindowScreen_ScreenConnected "ScreenConnected\(\)") (with an argument of `false`) and breaks the connection to the screen when [`ScreenConnected()`](BWindowScreen.md#BWindowScreen_ScreenConnected "ScreenConnected\(\)") returns. It then hides the window.

`Show()` shows the window on-screen and makes it the active window, which will cause it to establish a direct connection to the graphics card driver for the screen. Unlike `Hide()`, it may return before [`ScreenConnected()`](BWindowScreen.md#BWindowScreen_ScreenConnected "ScreenConnected\(\)") is called (with an argument of `true`).

See also: [`BWindow::Hide()`](BWindow.md#BWindow_Hide)

#### IOBase()

`void* IOBase();`

Returns a pointer to the base address for the input/output registers on the graphics card. Registers are addressed by 16-bit offsets from this base address. (This function may not be supported in future releases.)

#### MoveDisplayArea()

`status_t MoveDisplayArea(int32 x,  
int32 y);`

Relocates the display area, the portion of the frame buffer that's mapped to the screen. This function moves the area's left-top corner to (`x`, `y`); by default, the corner lies at (0, 0). The display area must lie entirely within the frame buffer.

`MoveDisplayArea()` only works if the graphics card driver permits application control over the frame buffer. It must also permit a frame buffer with a total area larger than the display area. If successful in relocating the display area, this function returns `B_OK`; if not, it returns `B_ERROR`.

See also: [`CanControlFrameBuffer()`](BWindowScreen.md#BWindowScreen_CanControlFrameBuffer "CanControlFrameBuffer\(\)")

#### Quit()

`virtual void Quit();`

Augments the [`BWindow`](BWindow.md "BWindow") version of `Quit()` to force the `BWindowScreen` object to disconnect itself from the screen, so that it doesn't quit while in control of the frame buffer.

Although `Quit()` disconnects the object before quitting, this may not be soon enough for your application. For example, if you need to destroy some drawing threads before the `BWindowScreen` object is itself destroyed, you should get rid of them after the screen connection is severed. You can force the object to disconnect itself by calling [`Disconnect()`](BWindowScreen.md#BWindowScreen_Disconnect "Disconnect\(\)"). For example:
    
    
    void MyWindowScreen::Quit()
    {
       Disconnect();
       kill_thread(drawing_thread_a);
       kill_thread(drawing_thread_b);
       BWindowScreen::Quit();
    }

If the screen connection is still in place when `Quit()` is called, it calls [`ScreenConnected()`](BWindowScreen.md#BWindowScreen_ScreenConnected "ScreenConnected\(\)") with a flag of `false`. It doesn't return until [`ScreenConnected()`](BWindowScreen.md#BWindowScreen_ScreenConnected "ScreenConnected\(\)") returns and the connection is broken.

####  RegisterThread(), Suspend(), SuspensionHook()

`void RegisterThread(thread_id thread);``void Suspend(char* label);``virtual void* SuspensionHook(bool suspended);`

These three functions aid in debugging a game application. They have relevance only if the `BWindowScreen` is running in debugging mode. To set up the mode, you must:

  1. Construct the `BWindowScreen` with the `debugging` flag set to `true`. The flag is `false` by default.

  2. Register all drawing threads (all threads that can touch the frame buffer in any way) by passing the thread_id to [`RegisterThread()`](BWindowScreen.md#BWindowScreen_RegisterThread "RegisterThread\(\), Suspend\(\), SuspensionHook\(\)") immediately after the thread is spawned—before `resume_thread()` is called to start the thread's execution. The window thread for the `BWindowScreen` object should not draw and should not be registered.

  3. Launch the application from the command line in a Terminal window. The window will collect debugging output from the application while the `BWindowScreen` runs in a different workspace, generally the one at the immediately preceding index. For example, if the Terminal window is in the fifth workspace (Command+F5), the game will run in the fourth (Command+F4); if the Terminal is in the fourth (Command+F4), the game runs in the third (Command+F3); and so on. However, if the Terminal window is in the first workspace (Command+F1), the game runs in the second (Command+F2).

The Terminal window is the destination for all messages the game writes to the standard error stream or to the standard output—from `printf()`, for example. You can switch back and forth between the game and Terminal workspaces to check the messages and run your application. When you switch from the game workspace to the Terminal workspace, all registered threads are suspended and the graphics context is saved. When you switch back to the game, the graphics context is restored and the threads are resumed.

Calling `Suspend()` switches to the Terminal workspace programmatically, just as pressing the correct Command—function key combination would. Registered threads are suspended, the Terminal workspace is activated, and the `label` passed as an argument is displayed in a message in the Terminal window. You can resume the game by manually switching back to its workspace.

`SuspensionHook()` is called whenever the game is suspended or resumed—whether by the user switching workspaces or by `Suspend()`. It gives you an opportunity to save and restore any state that would otherwise be lost. `SuspensionHook()` is called with a `suspended` flag of `true` just after the application is suspended and with a flag of `false` just before it's about to be resumed.

`ScreenConnected()` is not called when you switch between the Terminal and game workspaces while in debugging mode. However, it is called for all normal game activities—when the `BWindowScreen` is first activated and when it hides or quits, for example.

Debugging mode can also preserve some information in case of a crash. Hold down all the left modifier keys (Shift, Control, Option, Command, Alt, or whatever the keys may happen to be on your keyboard), and press the F12 key. This restarts the screen with a 640 * 480 resolution and displays a debugger window. You should then be able to switch to the Terminal workspace to check the last set of messages before the crash, modify your code, and start again.

#### ScreenChanged()

`virtual void ScreenChanged([`BRect`](BRect.md "BRect") frame,  
color_space mode);`

Overrides the [`BWindow`](BWindow.md "BWindow") version of [`ScreenChanged()`](BWindow.md#BWindow_ScreenChanged "ScreenChanged\(\)") so that it does nothing. This function is called automatically when the screen configuration changes. It's not one that you should call in application code or reimplement for the game.

See also: [`BWindow::ScreenChanged()`](BWindow.md#BWindow_ScreenChanged "ScreenChanged\(\)")

####  SetColorList(), ColorList()

`void SetColorList(rgb_color* colors,  
int32 first = 0,  
int32 last = 255);``rgb_color* ColorList();`

These functions set and return the list of 256 colors that can be displayed when the frame buffer has a depth of 8 bits per pixel (the `B_CMAP8` color space). `SetColorList()` is passed an array of one or more colors to replace the colors currently in the list. The first color in the array replaces the color in the list at the specified `first` index; all colors up through the `last` specified index are modified. It fails if either index is out of range.

`SetColorList()` alters the list of colors kept on the graphics card. If the `BWindowScreen` isn't connected to the screen, the new list takes effect when it becomes connected.

`ColorList()` returns a pointer to the entire list of 256 colors. This is not the list kept by the graphics card driver, but a local copy. It belongs to the `BWindowScreen` object and should be altered only by calling `SetColorList()`.

See also: [`BScreen::ColorMap()`](BScreen.md#BScreen_ColorMap "ColorMap\(\), IndexForColor\(\), ColorForIndex\(\)") in the [Interface Kit](TheInterfaceKit.md "The Interface Kit")

#### SetFrameBuffer()

`status_t SetFrameBuffer(int32 width,  
int32 height);`

Configures the frame buffer on the graphics card so that it's `width` pixel columns wide and `height` pixel rows high. This function works only if the driver for the graphics card allows custom configurations (as reported by [`CanControlFrameBuffer()`](BWindowScreen.md#BWindowScreen_CanControlFrameBuffer "CanControlFrameBuffer\(\)")) and the `BWindowScreen` object is currently connected to the screen.

The new dimensions of the frame buffer must be large enough to hold all the pixels displayed on-screen—that is, they must be at least as large as the dimensions of the display area. If the driver can't accommodate the proposed width and height, [`SetFrameBuffer()`](BWindowScreen.md#BWindowScreen_SetFrameBuffer "SetFrameBuffer\(\)") returns `B_ERROR`. If the change is made, it returns `B_OK`.

This function doesn't alter the depth of the frame buffer or the size or location of the display area.

See also: [`MoveDisplayArea()`](BWindowScreen.md#BWindowScreen_MoveDisplayArea "MoveDisplayArea\(\)"), [`SetSpace()`](BWindowScreen.md#BWindowScreen_MoveDisplayArea "MoveDisplayArea\(\)")

#### SetSpace()

`status_t SetSpace(uint32 space);`

Configures the screen space to one of the standard combinations of width, height, and depth. The configuration is first set by the class constructor—permitted `space` constants are documented there—and it may be altered after construction only by this function.

Setting the screen space sets the dimensions of the frame buffer and display area. For example, if `space` is `B_32_BIT_800x600`, the frame buffer will be 32 bits deep and at least 800 pixel columns wide and 600 pixel rows high. The display area (the area of the frame buffer mapped to the screen) will also be 800 pixels * 600 pixels. After setting the screen space, you can enlarge the frame buffer by calling [`SetFrameBuffer()`](BWindowScreen.md#BWindowScreen_SetFrameBuffer "SetFrameBuffer\(\)") and relocate the display area in the larger buffer by calling [`MoveDisplayArea()`](BWindowScreen.md#BWindowScreen_MoveDisplayArea "MoveDisplayArea\(\)").

If the requested configuration is refused by the graphics card driver, `SetSpace()` returns `B_ERROR`. If all goes well, it returns `B_OK`.

See also: the `BWindowScreen` [constructor](BWindowScreen.md#BWindowScreen_Constructor "BWindowScreen\(\)"), [`SetFrameBuffer()`](BWindowScreen.md#BWindowScreen_SetFrameBuffer "SetFrameBuffer\(\)"), [`MoveDisplayArea()`](BWindowScreen.md#BWindowScreen_MoveDisplayArea "MoveDisplayArea\(\)")

#### WindowActivated()

`virtual void WindowActivated(bool active);`

Overrides the [`BWindow`](BWindow.md "BWindow") version of [`WindowActivated()`](BWindow.md#BWindow_WindowActivated "WindowActivated\(\)") to connect the `BWindowScreen` object to the screen (give it control over the graphics card driver) when the `active` flag is `true`.

This function doesn't disconnect the `BWindowScreen` when the flag is `false`, because there's no way for the window to cease being the active window without the connection already having been lost.

Don't reimplement this function in your application, even if you call the inherited version; rely instead on [`ScreenConnected()`](BWindowScreen.md#BWindowScreen_ScreenConnected "ScreenConnected\(\)") for accurate notifications of when the `BWindowScreen` gains and loses control of the screen.

See also: [`BWindow::WindowActivated()`](BWindow.md#BWindow_WindowActivated "WindowActivated\(\)"), [`ScreenConnected()`](BWindowScreen.md#BWindowScreen_ScreenConnected "ScreenConnected\(\)")

#### WorkspaceActivated()

`virtual void WorkspaceActivated(int32 workspace,  
bool active);`

Overrides the [`BWindow`](BWindow.md "BWindow") version of [`WorkspaceActivated()`](BWindow.md#BWindow_WorkspaceActivated "WorkspaceActivated\(\)") to connect the `BWindowScreen` object to the screen when the `active` flag is `true` and to disconnect it when the flag is `false`. User's typically activate the game by activating the workspace in which it's running, and deactivate it by moving to another workspace.

Don't override this function in your application; implement [`ScreenConnected()`](BWindowScreen.md#BWindowScreen_ScreenConnected "ScreenConnected\(\)") instead.

See also: [`BWindow::WorkspaceActivated()`](BWindow.md#BWindow_WorkspaceActivated "WorkspaceActivated\(\)"), [`ScreenConnected()`](BWindowScreen.md#BWindowScreen_ScreenConnected "ScreenConnected\(\)")

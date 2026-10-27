<!-- Classes And Methods - The Game Kit - BDirectWindow -->

## BDirectWindow  
  
| Derived From:| [BWindow](BWindow.md "BWindow")  
---|---  
Mix-in Classes:| –  
Declared In:| `kit/game/DirectWindow.h`  
Library:| `libgame.so`  
Allocation:| –  
[Class Overview](BDirectWindow_Overview.md "BDirectWindow")

  * [Constructor and Destructor](BDirectWindow.md#BDirectWindow_ConstructorDestructor)
  * [Hook Functions](BDirectWindow.md#BDirectWindow_HookFunctions)
  * [Member Functions](BDirectWindow.md#BDirectWindow_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BDirectWindow()

`BDirectWindow([`BRect`](BRect.md "BRect") frame,  
const char* title,  
window_type type,  
uint32 flags,  
uint32 workspace = B_CURRENT_WORKSPACE);` `BDirectWindow([`BRect`](BRect.md "BRect") frame,  
const char* title,  
window_look look,  
window_feel feel,  
uint32 flags,  
uint32 workspace = B_CURRENT_WORKSPACE);`

Creates and returns a new `BDirectWindow` object. This is functionally equivalent to the [`BWindow`](BWindow.md "BWindow") [constructor](BWindow.md#BWindow_Constructor "BWindow\(\)"), except the resulting `BDirectWindow` supports direct window operations.

You will probably want to set up a flag to keep track of whether or not the direct window's connection to the screen is viable. In the constructor, you should set this flag (let's call it `fConnectionDisabled` ) to `false`, which indicates to both [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") and your drawing thread that the window is not in the process of being deconstructed. The destructor would then set this flag to `true` before terminating the connection to avoid the unlikely possibility of the connection trying to restart while the `BDirectWindow` is being dismantled.

You'll also need other flags or semaphores (or benaphores) to manage the interaction between the `BDirectWindow` and your drawing thread.

See the sample code in "[Using a Direct Window](BDirectWindow_Overview.md#BDirectWindow_Overview_Using_Direct_Window "Using a Direct Window")" for an example.

#### ~BDirectWindow()

`~BDirectWindow();`

Frees all memory the `BDirectWindow` object allocated for itself. You should never delete a `BDirectWindow` object; call its [`Quit()`](BWindow.md#BWindow_Quit "Quit\(\)") function instead (inherited from [`BWindow`](BWindow.md "BWindow")).

Your `BDirectWindow` destructor should begin by setting the `fConnectionDisabled` flag to `true`, to prevent [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") from attempting to reconnect to the direct window while it's being deconstructed.

Then you should call [`Hide()`](BWindow.md#BWindow_Hide) and [`Sync()`](BWindow.md#BWindow_Sync) to force the direct window to disconnect direct access (both inherited from [`BWindow`](BWindow.md "BWindow")):
    
    
    MyDirectWindow::~BDirectWindow
    {
       fConnectionDisabled = true;
       Hide();
       Sync();
       /* complete usual destruction here */
    }

* * *

### Hook Functions

#### DirectConnected()

`virtual void DirectConnected(direct_buffer_info* info);`

This hook function is the core of `BDirectWindow`. Your application should override this function to learn about the state of the graphics display onto which you're drawing, as well as to be informed of any changes that occur.

This function is also called to suspend and resume your direct access privileges.

Your code in this function should be as short as possible, because what your `DirectConnected()` function does can affect the performance of the entire system. `DirectConnected()` should only handle the immediate task of dealing with changes in the direct drawing context, and shouldn't normally do any actual drawing—that's what your drawing thread is for.

If you have drawing that absolutely has to be done before you can safely return control to the application server (see the note below), you may do so, but your code should do the absolute minimum drawing necessary and leave everything else to the drawing thread.

Note

![Note](./images/admonitions/Info_32.png)

`DirectConnected()` should only return when it can guarantee to the application server that the request specified by `info` will be strictly obeyed.

The structure pointed to by `info` goes away after `DirectConnected()` returns, so you should cache the information that interests you.

Warning

![Warning](./images/admonitions/Stop_32.png)

If your `DirectConnected()` implementation doesn't handle a request within three seconds, the Application Server will intentionally crash your application under the assumption that it's deadlocked. Be sure to handle requests as quickly as possible.

See "[Getting Connected (and Staying That Way)](BDirectWindow_Overview.md#BDirectWindow_Overview_GettingConnected "Getting Connected \(and Staying That Way\)")" for more information about the direct_buffer_info structure.

* * *

### Member Functions

#### GetClippingRegion()

`status_t GetClippingRegion([`BRegion`](BRegion.md "BRegion")* region,  
[`BPoint`](BPoint.md "BPoint")* origin = NULL) const;`

Sets the specified `region` to match the current clipping region of the direct window. If `origin` is specified, each point in the region is offset by the `origin`, resulting in a [`BRegion`](BRegion.md "BRegion") that's localized to your application's vision of where in space the origin is (relative to the origin of the screen's frame buffer).

Although the direct_buffer_info structure contains the clipping region of a direct window, it's not in standard [`BRegion`](BRegion.md "BRegion") form. This function is provided so you can obtain a standard [`BRegion`](BRegion.md "BRegion") if you need one.

Warning

![Warning](./images/admonitions/Stop_32.png)

The `GetClippingRegion()` function can only be called from the [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") function; calling it from outside [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") will return invalid results.

If you need to cache the clipping region of your window and need a [`BRegion`](BRegion.md "BRegion") for clipping purposes, you could use the following code inside your [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") function:
    
    
    [BRegion](BRegion.md "BRegion") rgn;
    GetClippingRegion(&rgn);

This serves a double purpose: it obtains the clipping region in [`BRegion`](BRegion.md "BRegion") form, and it returns a copy of the region that you can maintain locally. However, it may be more efficient to copy the clipping region by hand, since the clipping rectangle list used by `BDirectWindow` uses integer numbers, while [`BRegion`](BRegion.md "BRegion") uses floating-point.

Return Code| Description  
---|---  
`B_OK`.| The clipping region was successfully returned.  
`B_ERROR`.| An error occurred while trying to obtain the clipping region.  
  
####  IsFullScreen(), SetFullScreen()

`bool IsFullScreen() const;``status_t SetFullScreen(bool enable);`

`IsFullScreen()` returns `true` if the direct window is in full-screen exclusive mode, or `false` if it's in window mode.

The value returned by `IsFullScreen()` is indeterminate if a call to `SetFullScreen()` is in progress—if this is the case, you shouldn't rely on the resulting value. Instead, it would be safer to maintain a state setting of your own and use that value.

`SetFullScreen()` enables full-screen exclusive mode if the `enable` flag is `true`. To switch to window mode, pass `false`. The [`SupportsWindowMode() `](BDirectWindow.md#BDirectWindow_SupportsWindowMode "SupportsWindowMode\(\)") function can be used to determine whether or not the video card is capable of supporting window mode. See "[Window Mode vs. Full Screen Mode](BDirectWindow_Overview.md#BDirectWindow_Overview_Window_Vs_Fullscreen "Window Mode vs. Full Screen Mode")" for a detailed explanation of the differences between these modes.

When your window is in full screen mode, it will always have the focus, and no other window can come in front of it.

`SetFullScreen()` can return any of the following result codes.

Return Code| Description  
---|---  
`B_OK`.| The mode was successfully changed.  
`B_ERROR`.| An error occurred while trying to switch between full screen and window modes  
(for example, another window may already be in full-screen exclusive mode in the same workspace).  
  
#### SupportsWindowMode()

`static bool SupportsWindowMode(screen_id id = B_MAIN_SCREEN_ID);`

Returns `true` if the specified screen supports window mode; if you require the ability to directly access the frame buffer of a window (rather than occupying the whole screen), you should call this function to be sure that the graphics hardware in the computer running your application supports it. Because this is a static function, you don't have to construct a `BDirectWindow` object to call it:
    
    
    if (BDirectWindow::SupportsWindowMode()) {
       /* do stuff here */
    }

In particular, window mode requires a graphics card with DMA support and a hardware cursor; older video cards may not be capable of supporting window mode.

If window mode isn't supported, but you still select window mode, [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") will never be called (so you'll never be authorized for direct frame buffer access).

Even if window mode isn't supported, you can still use `BDirectWindow` objects for full-screen direct access to the frame buffer, but it's recommended that you avoid direct video DMA or the use of parallel drawing threads that use both direct frame buffer access and [`BView`](BView.md "BView") calls (because it's likely that such a graphics card won't handle the parallel access and freeze the PCI bus—and that would be bad).

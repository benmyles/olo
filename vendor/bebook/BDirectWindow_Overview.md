<!-- System Overview - The Game Kit - BDirectWindow -->

## BDirectWindow

The [`BDirectWindow`](BDirectWindow.md "BDirectWindow") class gives your code direct access to the graphics frame buffer on the video card. Unlike [`BWindowScreen`](BWindowScreen.md "BWindowScreen"), [`BDirectWindow`](BDirectWindow.md "BDirectWindow") can be used in both full-screen and window modes—you can create a [`BDirectWindow`](BDirectWindow.md "BDirectWindow") that looks just like a normal window, but lets your code draw into it by directly accessing the frame buffer.

In addition, [`BDirectWindow`](BDirectWindow.md "BDirectWindow") lets you switch between full-screen exclusive mode and windowed modes without breaking down and rebuilding the object. A simple call to the [`SetFullScreen()`](BDirectWindow.md#BDirectWindow_SetFullScreen) function does the job.

Note

![Note](./images/admonitions/Info_32.png)

if you want your direct window to be full-screen but don't want exclusive mode, just resize the window to fill the entire screen; since full-screen exclusive mode (as set by calling [`SetFullScreen()`](BDirectWindow.md#BDirectWindow_SetFullScreen)) won't let other windows draw in front of your direct window, you can't have menus in a full-screen exclusive mode direct window.

Another difference between [`BDirectWindow`](BDirectWindow.md "BDirectWindow") and [`BWindowScreen`](BWindowScreen.md "BWindowScreen") is that [`BDirectWindow`](BDirectWindow.md "BDirectWindow") lets you access all the [`BWindow`](BWindow.md "BWindow") functions; you can literally treat your [`BDirectWindow`](BDirectWindow.md "BDirectWindow") just like another window. There are two caveats:

Warning

![Warning](./images/admonitions/Stop_32.png)

  1. Don't draw into the direct window from its own thread; you should spawn another thread for drawing into the direct window.

  2. And use the [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") function to synchronize the interaction between [`BDirectWindow`](BDirectWindow.md "BDirectWindow") and your drawing thread. Also, if you choose to use [`BWindow`](BWindow.md "BWindow") or [`BView`](BView.md "BView") API inside a [`BDirectWindow`](BDirectWindow.md "BDirectWindow"), be sure you don't block the [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") function.

Not all video cards support window mode; use the [`SupportsWindowMode()`](BDirectWindow.md#BDirectWindow_SupportsWindowMode "SupportsWindowMode\(\)") function if you need to know whether or not window mode is available.

* * *

### Getting Connected (and Staying That Way)

The key to the [`BDirectWindow`](BDirectWindow.md "BDirectWindow") class is the [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") function, which your code must implement. This function is called whenever a change that your drawing code may need to be aware of occurs.

When your [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") function is called, it's passed a pointer to a direct_buffer_info structure, as follows:
    
    
    typedef struct {
       direct_buffer_state   buffer_state;
       direct_driver_state   driver_state;
       void*                 bits;
       void*                 pci_bits;
       int32                 bytes_per_row;
       uint32                bits_per_pixel;
       color_space           pixel_format;
       buffer_layout         layout;
       buffer_orientation    orientation;
       uint32                _reserved[9];
       uint32                _dd_type_;
       uint32                _dd_token_;
       uint32                clip_list_count;
       clipping_rect         window_bounds;
       clipping_rect         clip_bounds;
       clipping_rect         clip_list[1];
    } direct_buffer_info;

Field| Description  
---|---  
`buffer_state`| Indicates what change is occurring in the direct buffer access privileges. It can have one of the following values:

`B_DIRECT_START`
    You will always receive a `B_DIRECT_START` notification when your [`BDirectWindow`](BDirectWindow.md "BDirectWindow") is first connected to the screen
`B_DIRECT_MODIFY`
    Any number of `B_DIRECT_MODIFY` notifications follow (it's possible you won't receive any at all). When you return from [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") after handling `B_DIRECT_START` or `B_DIRECT_MODIFY`, your application guarantees to the application server that your code will abide by the frame buffer configuration specified by the direct_buffer_info structure until another `B_DIRECT_MODIFY` notification is received (or `B_DIRECT_STOP` occurs).
`B_DIRECT_STOP`
    You'll receive a `B_DIRECT_STOP` notification when the window is closed, hidden, or if moved, or if a resolution or color depth change occurs. Once your [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") function returns from handling this notification, you guarantee to the application server that your code won't touch the frame buffer anymore.
For any of these notifications, your [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") function shouldn't return until you can guarantee to the application server that your code will abide by the frame buffer configuration it received.You can get further information about what changed by testing against other flags in the `buffer_state` field:

`B_BUFFER_MOVED`
    The content of your window has been moved, either by a call to [`MoveTo()`](BWindow.md#BWindow_MoveTo "MoveBy\(\), MoveTo\(\)") or [`MoveBy()`](BWindow.md#BWindow_MoveBy) or by the user manually dragging the window. The contents of the window are always moved relative to the top-left corner of the window.
`B_BUFFER_RESET`
    The entire direct access buffer has been reset. This can happen if the user changes the depth or resolution of the screen, or if the window had previously been hidden and has been made visible again.
`B_BUFFER_RESIZED`
    The content area of your window has been resized.
`B_CLIPPING_MODIFIED`
    The visible region of the content area of your window changed. This doesn't imply anything about the position of the window or the size of the content area of the window—it simply means that the part of the window that's visible has changed shape.  
`driver_state`| Indicates changes in the state of the graphics card on which your direct window is displayed. There are two possible values:

`B_MODE_CHANGED`
    The resolution or depth of the graphics card has changed.
`B_DRIVER_CHANGED`
    The window was moved onto another monitor.  
`bits`| Is a pointer to the frame buffer in your own team's memory space.  
`pci_bits`| Is a pointer to the frame buffer in the PCI memory space; this value is typically needed to control DMA.  
`bytes_per_row`| Is the number of bytes used to represent a single row of pixels in the frame buffer.  
`bits_per_pixel`| Is the number of bits actually used to store a single pixel, including reserved, unused, or alpha channel bits. This value is usually a multiple of eight.  
`pixel_format`| Is the format used to encode a pixel, as defined in the color_space type in `GraphicsDefs.h`.  
`layout`, `orientation`, `_reserved` , `_dd_type_`, and `_dd_token_`| Are all reserved for future use and must not be used.  
`window_bounds`| Is a rectangle that defines the full content area of the window, in screen coordinates. You can convert these coordinates into frame buffer addresses using the values in the `bits` , `bytes_per_row` , and `bits_per_pixel` fields.  
`clip_bounds`| Is the bounding rectangle of the visible part of the content area of the window, in screen coordinates. This rectangle is the smallest rectangle that contains all the rectangles in the `clip_list`, described below.  
`clip_list_count`| Is the number of rectangles in the `clip_list`.  
`clip_list`| Is a list of rectangles that together define the visible region of the content area of the window, in screen coordinates  
  
The clipping_rect structure is:
    
    
    typedef struct {
       int32      left;
       int32      top;
       int32      right;
       int32      bottom;
    } clipping_rect;

Note that, as always, these edges are inclusive; for example, if left is 5 and top is 3, the pixel at (5,3) is included in the rectangle's contents.

The data in the direct_buffer_info structure is only valid until `DirectConnected()` returns, so if you need to reference any of the information later, you should make a copy of the fields you need.

Warning

![Warning](./images/admonitions/Stop_32.png)

If your [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") implementation doesn't handle a request within three seconds, the Application Server will intentionally crash your application under the assumption that it's deadlocked. Be sure to handle requests as quickly as possible.

* * *

### Window Mode vs. Full Screen Mode

There are some differences in how [`BDirectWindow`](BDirectWindow.md "BDirectWindow") behaves depending on whether it's in window mode or full-screen exclusive mode.

In window mode, the [`BDirectWindow`](BDirectWindow.md "BDirectWindow") behaves almost exactly like a [`BWindow`](BWindow.md "BWindow")—so much so that you can use a [`BDirectWindow`](BDirectWindow.md "BDirectWindow") in any situation you'd normally use a [`BWindow`](BWindow.md "BWindow"). The `window_bounds` rectangles are the same size and shape as the window itself, as you'd expect. If exclusive window mode is available ( [`SupportsWindowMode()`](BDirectWindow.md#BDirectWindow_SupportsWindowMode "SupportsWindowMode\(\)") returns `true`), [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") will be called as described above, thereby providing the means to directly access the frame buffer. If the graphics card doesn't support exclusive access to the frame buffer while in window mode, [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") will never be called, and you can only use [`BWindow`](BWindow.md "BWindow") and [`BView`](BView.md "BView") APIs to work in the window.

In full-screen exclusive mode, the `window_bounds` are actually the size and shape of the entire screen, even if the screen isn't the same size as the direct window you created. You have to handle the difference yourself.

Full-screen exclusive mode also guarantees that your window will always be the focus, always be in front, and will always stay full-screen (the application server will resize the window for you if the screen resolution changes). Since no other windows can come in front of a full-screen exclusive direct window, any Interface Kit objects that use a window to display their contents won't work; this includes any type of menu.

If you want your [`BDirectWindow`](BDirectWindow.md "BDirectWindow") to be full-screen, but still compatible with menus or other windows, create it as a non-exclusive window, then use the following code:
    
    
    BScreen screen(this);
    MoveTo(0,0);
    ResizeTo(screen.width, screen.height);

This will make the non-exclusive direct window fill the entire screen. Keep in mind that in this case, other windows may appear in front of yours, and if the screen resolution changes, you will have to resize the window yourself if you want to continue to fill the entire screen.

* * *

### Using a Direct Window

Let's put together a simple class, derived from [`BDirectWindow`](BDirectWindow.md "BDirectWindow"), that demonstrates the basics of drawing into a direct window.
    
    
    class DirectSample : public BDirectWindow {
    public:
                       DirectSample(BRect frame);
                       ~DirectSample();
        virtual bool   QuitRequested();
        virtual void   DirectConnected(direct_buffer_info *info);
    
        uint8*         fBits;
        int32          fRowBytes;
        color_space    fFormat;
        clipping_rect  fBounds;
    
        uint32         fNumClipRects;
        clipping_rect* fClipList;
    
        bool           fDirty;      // needs refresh?
        bool           fConnected;
        bool           fConnectionDisabled;
        BLocker*       locker;
        thread_id      fDrawThreadID;
    };

The `DirectSample` class implements a constructor as well as the [`QuitRequested()`](BApplication.md#BApplication_QuitRequested "QuitRequested\(\)") and [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") functions.

Some variables are added to the class for cacheing information about the frame buffer.

Field| Description  
---|---  
`fBits`| Will contain a pointer to the frame buffer's bitmap.  
`fRowBytes`| Will contain the number of bytes per row of screen data.  
`fFormat`| Will contain the pixel format (such as `B_CMAP8` for 8-bit indexed color graphics mode). Our sample program will only work in this mode.  
`fBounds`| will contain the bounds rectangle for the window.  
`fNumClipRects`| Will contain the number of rectangles in the clip rectangle list.  
`fClipList`| Is the actual list of clip rectangles, and will be allocated on-the-fly as needed.  
`fDirty`| Will be `true` if the window needs to be redrawn.  
`fConnected`| Is `true` if the window is connected to the frame buffer.  
`fConnectionDisabled`| Is `true` if the window is in the process of being closed.  
`locker`| Is a [`BLocker`](BLocker.md "BLocker") that will be used to ensure mutual exclusion when the frame buffer or buffer information data we've cached is being manipulated.  
`fDrawThreadID`| Contains the thread_id of the drawing thread, which is responsible for drawing the contents of the window.  
  
The specifics of what these variables are and why the information contained in them is maintained will be discussed when we get to the [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") and `DrawingThread()` functions.

The constructor for the `DirectSample` class looks like this:
    
    
    DirectSample::DirectSample(BRect frame)
                 : BDirectWindow(frame, "DirectWindow Sample",
                                 B_TITLED_WINDOW,
                                 B_NOT_RESIZABLE|B_NOT_ZOOMABLE)
    {
       fConnected = false;
       fConnectionDisabled = false;
       locker = new BLocker();
       fClipList = NULL;
       fNumClipRects = 0;
    
       AddChild(new SampleView(Bounds()));
    
       if (!SupportsWindowMode()) {
          SetFullScreen(true);
       }
    
       fDirty = true;
       fDrawThreadID = spawn_thread(DrawingThread, "drawing_thread",
                      B_NORMAL_PRIORITY, (void *) this);
       resume_thread(fDrawThreadID);
       Show();
    }

This code establishes the direct window by deferring to [`BDirectWindow`](BDirectWindow.md "BDirectWindow"). Then the `fConnected` and `fConnectionDisabled` flags are initialized to indicate that the window isn't connected yet, but the connection isn't in the process of being torn down by the `DirectSample` destructor. The locker is created, and the clip rectangle list is initialized to a `NULL` pointer, with a count of 0.

Then it adds a child view that occupies the entire window. The primary purpose of this view in this sample is to set the view color to `B_TRANSPARENT_32_BIT`, to prevent the application server from erasing the window with a default color.

If the video card doesn't support window mode, we call [`SetFullScreen()`](BDirectWindow.md#BDirectWindow_SetFullScreen) to switch the direct window into full-screen exclusive mode. This guarantees that you'll get connected with direct screen access (in a window if possible, otherwise in full-screen exclusive mode). If you don't use [`SetFullScreen()`](BDirectWindow.md#BDirectWindow_SetFullScreen), and window mode isn't supported, [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") will never be called, and you won't have direct screen access.

Then the `fDirty` flag is set to `true`, which indicates that the window needs to be updated, and the drawing thread is started; the drawing thread will handle all actual drawing into the window. The argument passed to the drawing thread is a pointer to the `DirectSample` window itself. You should always use a separate thread for drawing into a [`BDirectWindow`](BDirectWindow.md "BDirectWindow").

Finally, [`Show()`](BWindow.md#BWindow_Show "Show\(\), Hide\(\), IsHidden\(\), Minimize\(\), IsMinimized\(\)") is called to make the direct window visible.

The destructor needs to make sure there's no chance someone will try to draw while the window is being destructed:
    
    
    DirectSample::~DirectSample() {
       int32 result;
    
       fConnectionDisabled = true;      // Connection is dying
       Hide();
       Sync();
       wait_for_thread(fDrawThreadID, &result);
       free(fClipList);
       delete locker;
    }

The first thing the destructor does is set the `fConnectionDisabled` flag to `true`, which indicates that the window is in the process of being destroyed, and that future calls to [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") or the drawing thread should be ignored. The window is then hidden by calling [`Hide()`](BWindow.md#BWindow_Hide). Finally, [`Sync()`](BWindow.md#BWindow_Sync) is called to block until the window is actually hidden.

[`wait_for_thread()`](TheKernelKit_ThreadsAndTeams.md#wait_for_thread "wait_for_thread\(\)") waits until the drawing thread terminates. The drawing thread (as we'll see shortly) is designed to terminate when the `fConnectionDisabled` flag is `true`.

Then the clip rectangle list is freed and the locker deleted.

The [`QuitRequested()`](BApplication.md#BApplication_QuitRequested "QuitRequested\(\)") function is implemented as usual.

The [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") function is called whenever a change occurs that affects how your code should access the frame buffer:
    
    
    void DirectSample::DirectConnected(direct_buffer_info *info) {
       if (!fConnected && fConnectionDisabled) {
          return;
       }
       locker->Lock();
    
       switch(info->buffer_state & B_DIRECT_MODE_MASK) {
          case B_DIRECT_START:
             fConnected = true;
    
          case B_DIRECT_MODIFY:
             // Get clipping information
    
             if (fClipList) {
                free(fClipList);
                fClipList = NULL;
             }
    
             fNumClipRects = info->clip_list_count;
             fClipList = (clipping_rect *)
                   malloc(fNumClipRects*sizeof(clipping_rect));
    
             if (fClipList) {
                memcpy(fClipList, info->clip_list,
                      fNumClipRects*sizeof(clipping_rect));
                fBits = (uint8 *) info->bits;
                fRowBytes = info->bytes_per_row;
                fFormat = info->pixel_format;
                fBounds = info->window_bounds;
                fDirty = true;
    
             }
             break;
    
          case B_DIRECT_STOP:
             fConnected = false;
             break;
       }
       locker->Unlock();
    }

[`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") begins by checking the `fConnected` and `fConnectionDisabled` flags; the code in [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") is only run if the connection is opened (`fConnected` is `true`) or if we want to start it again (`fConnectionDisabled` is `false`). This arrangement prevents the [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") function from trying to reconnect if the destructor has started running. Otherwise, the locker is locked, to prevent [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") and the drawing thread from colliding.

If the buffer state is `B_DIRECT_START`, the `fConnected` flag is set to `true`. This keeps track of the fact that the application server has given permission to draw directly into the region of the frame buffer controlled by the direct window.

If the buffer state is `B_DIRECT_START` or `B_DIRECT_MODIFY` (in which case the direct_buffer_info structure describes changes to the frame buffer), any previously-existing clip rectangle list is deleted, then we cache the information that interests us and set the `fDirty` flag to `true` (to indicate that the display needs to be redrawn to reflect the changed graphics settings).

The clip list is also cached by saving the number of rectangles in the list in the `fNumClipRects` field and by making a copy of the clip list into a newly `malloc()`d block of memory.

If the state is `B_DIRECT_STOP`, the `fConnected` flag is set to `false`, to indicate that we shouldn't draw into the frame buffer anymore.

Finally, the locker is unlocked, which lets the drawing thread start running again.

Now let's have a look at `DrawingThread()`; this function serves as the drawing thread, and is a global function:
    
    
    int32 DrawingThread(void* data) {
       DirectSample* w;
    
       w = (DirectSample*) data;
       while (!w->fConnectionDisabled) {
          w->locker->Lock();
          if (w->fConnected) {
             if (w->fFormat == B_CMAP8 && w->fDirty) {
                int32 y;
                int32 width;
                int32 height;
                int32 adder;
                uint8 *p;
                clipping_rect *clip;
                int32 i;
    
                adder = w->fRowBytes;   // Stash locally for this pass
                for (i=0; i<w->fNumClipRects; i++) {
                   clip = &(w->fClipList[i]);
                   width = (clip->right - clip->left)+1;
                   height = (clip->bottom - clip->top)+1;
                   p = w->fBits+(clip->top*w->fRowBytes)+clip->left;
                   y = 0;
                   while (y < height) {
                      memset(p, 0x00, width);
                      y++;
                      p+=adder;
                   }
                }
             }
             w->fDirty = false;
          }
          w->locker->Unlock();
          // Use BWindow or BView APIs here if you want
          snooze(16000);
       }
       return B_OK;
    }

`DrawingThread()` starts by casting the argument, `data`, into a pointer to the `DirectSample` window into which it will be drawing.

The while loop that follows will continue to run as long as the `fConnectionDisabled` flag is `true`—in other words, it will keep looping as long as the connection is enabled.

The drawing loop itself begins by locking the locker to ensure that [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") doesn't change anything while we're working, then checking to be sure the connection is opened (`fConnected` is `true`). If the connection is open, we verify that format of the window is still 8-bit color and that the display needs to be updated. If the display needs updating and the pixel format is still `B_CMAP8`, the drawing code begins.

The `fRowBytes` field of the `DirectSample` window is cached in a local variable called `adder`. Then each rectangle in the clip list is drawn, one at a time, using a for loop.

A pointer to the clip rectangle to be drawn is stored in `clip`, and the width and height of the rectangle are computed. Then `p` is set to be a pointer to the first pixel in the frame buffer that's contained by the clip rectangle. Since 8-bit color pixels each occupy exactly one byte of video memory, this pixel's address can be computed by taking the base `fBits` pointer, adding the number of bytes per row times the number of rows between the top of the screen and the first row in the clip rectangle, then adding the number of bytes between the left edge of the screen and the left edge of the clip rectangle, as seen in the line:
    
    
    p = w->fBits+(clip->top*w->fRowBytes)+clip->left;

Then a while loop is used to iterate over each line in the clip rectangle, by ranging the variable `y` from 0 to the height of the clip rectangle. `memset()` is used to clear the row to black, which is represented by the byte value 0x00. `y` is incremented by one for each pass through the loop, to count the rows being drawn for each iteration, and the pointer `p` is incremented by adder to move down to the beginning of the next row in the clip rectangle.

Once each clip rectangle has been drawn, the for loop exits, and the `fDirty` flag is set to `false` to indicate that the screen is up-to-date. Once that's done, the locker is unlocked, which lets [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") do its thing if it's called. To avoid using an unreasonable amount of processing time, [`snooze()`](TheKernelKit_ThreadsAndTeams.md#snooze "snooze\(\), snooze_until\(\)") is called to give up CPU time to other threads.

If you want to use calls to [`BWindow`](BWindow.md "BWindow") or [`BView`](BView.md "BView") API in your drawing thread, you should do so just after unlocking the window.

When the thread terminates (which will only happen when the connection is disabled), the drawing thread returns `B_OK`.

This drawing function is designed to draw nothing unless it's necessary; [`DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") will set the `fDirty` flag when something happens to cause the screen to need a refresh, and other code elsewhere in the application could also set the `fDirty` flag to indicate that the screen should be redrawn.

Since we're taking over drawing the contents of the window, we need to tell the application server not to draw anything. This is done by adding the following line to the constructor for the SampleView:
    
    
    SetViewColor(B_TRANSPARENT_32_BIT);

This is very important: if you don't remember to do this, you'll have all kinds of synchronization problems when the application server and your drawing code try to draw into the window at the same time.

Note that this sample code doesn't really do anything useful (if all you want to do is have a black window moving around, don't use [`BDirectWindow`](BDirectWindow.md "BDirectWindow")—use a regular [`BWindow`](BWindow.md "BWindow"), throw a [`BView`](BView.md "BView") into it, and use [`SetViewColor()`](BView.md#BView_SetViewColor "SetViewColor\(\), ViewColor\(\)") to make the view black; it'll be faster and more efficient because it will use hardware graphics acceleration if it's available). However, it serves as a simple example of how to establish a connection to let your own drawing code directly access the screen. Just replace the code inside the drawing loop with something more useful (like a nifty real-time animation).

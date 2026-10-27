<!-- Classes And Methods - The OpenGL Kit - BGLView -->

## BGLView

| Derived From:| [`BView`](BView.md "BView")  
---|---  
Mix-in Classes:| None  
Declared In:| `opengl/GLView.h`  
Library:| `libGL.so`  
Allocation:| –  
[Class Overview](BGLView_Overview.md "BGLView")

  * [Constructor and Destructor](BGLView.md#BGLView_ConstructorDestructor)
  * [Member Functions](BGLView.md#BGLView_MemberFunctions)

  
  
* * *

### Constructor and Destructor

####  BGLView()

`BGLView([`BRect`](BRect.md "BRect") frame,  
const char* name,  
int32 resizingMode,  
int32 flags,  
int32 type);`

Initializes the view, then creates a new OpenGL drawing context and attaches it to the view. The `type` argument specifies the OpenGL type specification for the view:

Constant| Description  
---|---  
`BGL_RGB`| Use RGB graphics instead of indexed color (8-bit). This is the default if neither `BGL_RGB` nor `BGL_INDEX` is specified.  
`BGL_INDEX`| Use indexed color (8-bit graphics). Not currently supported.  
`BGL_SINGLE`| Use single-buffering; all rendering is done directly to the display. This is not currently supported by the BeOS implementation of OpenGL. This is the default if neither `BGL_SINGLE` nor `BGL_DOUBLE` is specified.  
`BGL_DOUBLE`| Use double-buffered graphics. All rendering is done to an offscreen buffer and only becomes visible when the [`SwapBuffers()`](BGLView.md#BGLView_SwapBuffers "SwapBuffers\(\)") function is called.  
`BGL_ACCUM`| Requests that the view have an accumulation buffer.  
`BGL_ALPHA`| Requests that the view's color buffer include an alpha component.  
`BGL_DEPTH`| Requests that the view have a depth buffer.  
`BGL_STENCIL`| Requests that the view have a stencil buffer.  
  
#### ~BGLView()

`virtual ~BGLView();`

Disposes of the OpenGL context for the view.

* * *

### Member Functions

#### AttachedToWindow()

`virtual void ~BGLView();`

Calls the inherited version of [`AttachedToWindow()`](BView.md#BView_AttachedToWindow "AttachedToWindow\(\), AllAttached\(\)") and sets the view color to `B_TRANSPARENT_32_BIT` (this improves performance by preventing the Application Server from erasing the view, since OpenGL takes over responsibility for drawing into the view).

####  CopyPixelsIn(), CopyPixelsOut()

`status_t CopyPixelsIn([`BBitmap`](BBitmap.md "BBitmap")* source,  
[`BPoint`](BPoint.md "BPoint") dest);``status_t CopyPixelsOut([`BPoint`](BPoint.md "BPoint") source,  
[`BBitmap`](BBitmap.md "BBitmap")* dest);`

These functions copy pixel data into and out of the OpenGL draw buffer for the context.

`CopyPixelsIn()` copies the entire contents of the `source` [`BBitmap`](BBitmap.md "BBitmap") into the OpenGL context, offset such that the top-left corner of the [`BBitmap`](BBitmap.md "BBitmap") is drawn at the point dest in the OpenGL buffer.

`CopyPixelsOut()` copies from the OpenGL draw buffer into the specified [`BBitmap`](BBitmap.md "BBitmap"). The area copied is the size of the dest bitmap and contains all data from the specified source `point` to the bottom-right corner of the buffer.

Note

![Note](./images/admonitions/Info_32.png)

If the source is larger than the destination, it's clipped at the bottom and right edges to fit; no scaling is performed Also, the OpenGL context and the [`BBitmap`](BBitmap.md "BBitmap") must be in the same color space.

Return Code| Description  
---|---  
`B_OK`| The data was copied without error.  
`B_BAD_VALUE`| The current draw buffer is not valid, or the destination buffer's width or height is less than or equal to zero.  
`B_BAD_TYPE`| The source and destination are in different color spaces.  
  
#### DirectConnected()

`void DirectConnected(direct_buffer_info* directInfo);`

If the `BGLView` is in a [`BDirectWindow`](BDirectWindow.md "BDirectWindow"), you should call this from your [`BDirectWindow::DirectConnected()`](BDirectWindow.md#BDirectWindow_DirectConnected "DirectConnected\(\)") function to let OpenGL update the window properly.

#### Draw()

`virtual void Draw([`BRect`](BRect.md "BRect") updateRect);`

Refreshes the contents of the `BGLView` by copying the frontbuffer to the screen.

If the view's color space is eight bits deep and the `GL_DITHER` OpenGL option is enabled, the display is dithered.

#### EmbeddedView()

`[`BView`](BView.md "BView")* EmbeddedView();`

Returns a pointer to an embedded view that encompasses the current OpenGL drawing buffer, as defined by OpenGL, for the `BGLView`. If the view is single-buffered, this will be the frontbuffer, and if the view is double-buffered, the embedded view will encompass the backbuffer.

Note

![Note](./images/admonitions/Info_32.png)

`EmbeddedView()` returns `NULL` if, for any reason, [`BView`](BView.md "BView") functions can't be used in the GL buffer. Starting with BeOS R4, this function always returns `NULL`, as the new, high-performance implementation of OpenGL does not support tinkering with the view.

#### EnableDirectMode()

`void EnableDirectMode(bool enabled);`

Call this function to tell the `BGLView` whether or not it should render in direct mode. If you're using a [`BDirectWindow`](BDirectWindow.md "BDirectWindow") and want video refreshes to be done through the direct window method, pass `true` for `enabled`. If you don't want to use the direct window method, pass `false`.

#### ErrorCallback()

`virtual void ErrorCallback(GLenum errorCode);`

Called when an OpenGL error occurs. By default, this function invokes the debugger with an error message reading "GL: Error code $xxxx." You can (and probably should) reimplement this function to cope with errors more gracefully.

#### FrameResized()

`virtual void FrameResized(float width,  
float height);`

Calls the inherited version of [`FrameResized()`](BView.md#BView_FrameResized "FrameResized\(\)"), releases tables that need to be recalculated, and resizes the OpenGL buffers.

You can augment this function to perform other necessary tasks, such as adjusting your `BGLView`'s coordinate system.

####  LockGL(), UnlockGL()

`void LockGL();``void UnlockGL();`

These functions lock and unlock the OpenGL context. You must lock the context before issuing any OpenGL commands, and unlock it when you're done—this is how OpenGL knows which context the drawing commands are intended for, since OpenGL itself isn't encapsulated within the `BGLView` class. For example:
    
    
    LockGL();            /* lock the OpenGL context
    glEnable(GL_DITHER); /* turn on dithering support */
    UnlockGL();

Failing to lock and unlock the context appropriately will result in unpredictable behavior and may cause your application to crash.

#### SwapBuffers()

`void SwapBuffers();` `void SwapBuffers(bool vSync);`

Swaps the front buffer and back buffer, then redraws the contents of the `BGLView`.

This function has no effect if the view is single-buffered.

If `vSync` is specified and is `true`, the swap is synchronized with vertical blanking.

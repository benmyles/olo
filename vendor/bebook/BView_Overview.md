<!-- System Overview - The Interface Kit - BView -->

## BView

A [`BView`](BView.md "BView") object represents a rectangular area within a window. The object draws within this rectangle and responds to user events (mouse clicks, key presses) that are directed at the window.

Most of the other classes in the Interface Kit inherit from [`BView`](BView.md "BView"). You can use these objects off-the-shelf, but for most applications, you'll also need to create some [`BView`](BView.md "BView") subclasses, and create instances of these subclasses.

The [`BView`](BView.md "BView") class is one of the largest in the BeOS class library. Because of this, the [`BView`](BView.md "BView") documentation is split across a number of files:

  * [BView General Functions](BView.md#BView_General "General Functions") describes those functions that you use no matter what you're doing with your [`BView`](BView.md "BView") object.

  * [BView Drawing-Related Functions](BView.md#BView_Drawing "Drawing Related Functions"): Anything related to drawing subdivided into Primitive Drawing Functions (to draw lines, circles, polygons, etc.) and Other Drawing Functions (such as picture recording, and bitmap functions).

  * [BView Graphics State Functions](BView.md#BView_Graphics "Graphics State Functions") modify or report on the "graphics state," a set of variables (colors, pen position, scaling, etc.) that control how drawing takes place in the [`BView`](BView.md "BView").

  * [BView Hook Functions](BView.md#BView_Hook "Hook Functions") gives a list of the [`BView`](BView.md "BView") functions that can be implemented in a subclass.

  * [BView View Hierarchy Functions](BView.md#BView_ViewHierarchy "View Hierarchy Functions") relate to the nested tree of views associated with a window.

  * [BView Input-Related Functions](BView.md#BView_Input "Input Related Functions") monitor and process input activity.

  * Scripting and Archival describes the scripting suites and properties, and archived fields supported by BView.

* * *

### Developing a BView Subclass

To define special behavior for a [`BView`](BView.md "BView")—such as the type of behavior that might be needed for the canvas area of a painting program—you define your own subclass of [`BView`](BView.md "BView"), and in that subclass implement the appropriate hook functions of the [`BView`](BView.md "BView") class. A hook function is a function that is called by the BeOS internals in response to certain events, and that is intended specifically for definition in a user-defined subclass. For example, the [`BView::MouseDown()`](BView.md#BView_MouseDown "MouseDown\(\)") hook function is called on a BView instance every time a mouse button is clicked while the mouse is inside the view. The default [`BView::MouseDown()`](BView.md#BView_MouseDown "MouseDown\(\)") function does nothing, but you can override this with your own implementation, to respond to mouse-down events as you wish.

The following are the most commonly used hook functions. Other hook functions provide for changes in the size or structure of views and windows, changes in the input focus, and so forth. For a complete list and reference of hook functions, see [BView Hook Functions](BView.md#BView_Hook "Hook Functions").

[`Draw()`](BView.md#BView_Draw "Draw\(\)")| Called whenever the contents of the view need to be drawn or redrawn.  
---|---  
[`KeyDown()`](BView.md#BView_KeyDown "KeyDown\(\)")| Called when a keyboard key is pressed.  
[`KeyUp()`](BView.md#BView_KeyUp "KeyUp\(\)")| Called when a keyboard key is released.  
[`MouseDown()`](BView.md#BView_MouseDown "MouseDown\(\)")| Called when a mouse button is clicked while the mouse cursor is in the view.  
[`MouseMoved()`](BView.md#BView_MouseMoved "MouseMoved\(\)")| Called when the mouse cursor enters, exits, or moves within the view.  
  
* * *

### The View Structure

A newly created view is an "orphan"—it won't appear onscreen, and can't be used for much, because it isn't associated with an onscreen & quot;parent". To rectify this situation, and give your new view a warm and loving home, invoke the [`AddChild()`](BView.md#BView_AddChild "AddChild\(\), RemoveChild\(\)") method of an existin [`BWindow`](BWindow.md "BWindow") or [`BView`](BView.md "BView") object to add the new view as a child, i.e.
    
    
    existingWindowOrView.AddChild(yourNewView);

Ultimately, all views displayed in a window end up as a tree of views, with the window as the root of the tree.

See the BView View Hierarchy Functions section for BView functions related to this view structuring.

* * *

### Locking the Window

Most [`BView`](BView.md "BView") functions expect the view's BWindow to be locked. To find a view's [`BWindow`](BWindow.md "BWindow") and lock/unlock it, you first call [`BView::Window()`](BView.md#BView_Window "Window\(\)") and then call the [`LockLooper()`](BHandler.md#BHandler_LockLooper "LockLooper\(\), LockLooperWithTimeout\(\), UnlockLooper\(\)") and [`UnlockLooper()`](BHandler.md#BHandler_UnlockLooper) functions (defined by [`BHandler`](BHandler.md "BHandler"), inherited by [`BWindow`](BWindow.md "BWindow")):
    
    
    if (window.LockLooper() ) {
       . . .
       window.UnlockLooper();
    }

[`BView`](BView.md "BView") functions check to ensure sure the caller has the window locked. If the window isn't properly locked, they print warning messages and fail.

Whenever the system calls a [`BView`](BView.md "BView") hook function, it automatically locks the [`BWindow`](BWindow.md "BWindow") for you—you never have to lock the [`BWindow`](BWindow.md "BWindow") in the implementation of [`BView`](BView.md "BView") hook function.

* * *

### Focus

To facilitate keyboard navigation of views, [`BView`](BView.md "BView") provides integral support for the concept of focus. The view that has the focus is the one whose [`KeyDown()`](BView.md#BView_KeyDown "KeyDown\(\)") function is called to process keyboard events. Only one view in a window can have the focus at any given time

From the user's point-of-view, the tab key rotates the focus from one view to the next through the navigation group, cycling back to the first view if the tab key is pressed while the last targetable view in the group has the focus. shift+tab cycles the focus backward. Holding down the control key while cycling groups causes the focus to jump from one navigation group to the next.

When a view has the focus, some sort of indicator should be drawn to inform the user that the view is the focus. Typically, this involves drawing a line under a label in the view, or possibly drawing a box around the view or some portion of it. The global [`keyboard_navigation_color()`](TheInterfaceKit_Functions.md#keyboard_navigation_color "keyboard_navigation_color\(\)") function should be used to obtain the color used to draw the focus indicator.

The view's [`MakeFocus()`](BView.md#BView_MakeFocus "MakeFocus\(\)") function is called to specify whether or not the control has the focus; it's called with an argument of `true` if the control has the focus, and `false` if it's not the focus; [`MakeFocus()`](BView.md#BView_MakeFocus "MakeFocus\(\)") calls the previously-focused view's [`MakeFocus()`](BView.md#BView_MakeFocus "MakeFocus\(\)") function to inform that view that it's not the focus anymore. You can augment [`MakeFocus()`](BView.md#BView_MakeFocus "MakeFocus\(\)") in a subclass if you need to take notice when the view becomes the focus (or loses the focus). For example, you may need to draw or erase the keyboard navigation indicator.

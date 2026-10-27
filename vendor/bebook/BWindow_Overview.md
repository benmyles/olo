<!-- System Overview - The Interface Kit - BWindow -->

## BWindow

A [`BWindow`](BWindow.md "BWindow") object represents a window that can be displayed on the screen, and that can be the target of user events. You almost always create your own [`BWindow`](BWindow.md "BWindow") subclass(es) rather than use direct instances of [`BWindow`](BWindow.md "BWindow").

[`BWindow`](BWindow.md "BWindow") objects draw windows by talking to the App Server. If you want to take over the entire screen or draw directly into the graphics card's frame buffer (by-passing the App Server), you should use a [`BDirectWindow`](BDirectWindow.md "BDirectWindow") or [`BWindowScreen`](BWindowScreen.md "BWindowScreen") object (both classes are defined in the Game Kit).

* * *

### Creating and Using a BWindow

You must create your [`BApplication`](BApplication.md "BApplication") object (`be_app`) before you create any windows. be_app needn't be running to construct—or even show—a window, but it must be running for the window to receive notifications of user events (mouse clicks, key presses, etc.).

Typically, the first thing you do with your [`BWindow`](BWindow.md "BWindow") is add [`BView`](BView.md "BView")s to it, through the [`AddChild()`](BWindow.md#BWindow_AddChild "AddChild\(\), RemoveChild\(\), ChildAt\(\), CountChildren\(\)") function. Again, `be_app` needn't be running at this point, nor must the window be showing.

Even though it inherits from [`BLooper`](BLooper.md "BLooper"), you never invoke [`AddChild()`](BWindow.md#BWindow_AddChild "AddChild\(\), RemoveChild\(\), ChildAt\(\), CountChildren\(\)")Run() on a [`BWindow`](BWindow.md "BWindow"). Instead, you call [`Show()`](BWindow.md#BWindow_Show "Show\(\), Hide\(\), IsHidden\(\), Minimize\(\), IsMinimized\(\)"). In addition to putting the window on-screen, the first (and only the first) [`Show()`](BWindow.md#BWindow_Show "Show\(\), Hide\(\), IsHidden\(\), Minimize\(\), IsMinimized\(\)") invocation starts the [`BWindow`](BWindow.md "BWindow")'s message loop. To remove a window from the screen without interrupting the object's message loop, use [`Hide()`](BWindow.md#BWindow_Hide). Other message loop details (locking and quitting in particular) are handled as described in the [`BLooper`](BLooper.md "BLooper") class.

Warning

![Warning](./images/admonitions/Stop_32.png)

If you create a [`BWindow`](BWindow.md "BWindow")-derived class that uses multiple inheritance, make sure the first class your mixin class inherits from is [`BWindow`](BWindow.md "BWindow"); otherwise, you'll crash when you try to close the window. This happens because of an interaction between the window thread how C++ deletes objects of a multiply-inherited class. In other words:
    
    
    class myClass : public BWindow, public OtherClass

is safe, while
    
    
    class myClass : public OtherClass, public BWindow

is not.

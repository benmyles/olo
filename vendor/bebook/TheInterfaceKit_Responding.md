<!-- System Overview - The Interface Kit - Responding to the User -->

## Responding to the User

When the user fiddles with the mouse or keyboard, the Application Server figures out which window the event is intended for, and then forms an interface message that it sends to the [`BWindow`](BWindow.md "BWindow") object. The [`BWindow`](BWindow.md "BWindow") receives the message in its [`DispatchMessage()`](BWindow.md#BWindow_DispatchMessage "DispatchMessage\(\)") function and invokes an interface hook function declared by [`BWindow`](BWindow.md "BWindow") or [`BView`](BView.md "BView"). If your application wants to respond to an event it simply implements the appropriate hook function. For example, if you want to watch for mouse down events within a view, you implement [`MouseDown()`](BView.md#BView_MouseDown "MouseDown\(\)") in your [`BView`](BView.md "BView") subclass.

You can also find out what's going on with the keyboard and mouse through the (global) [`get_key_info()`](TheInputServer_Functions.md#get_key_info "get_key_info\(\)") function and the [`BView::GetMouse()`](BView.md#BView_GetMouse "GetMouse\(\)") function. You typically use these functions within the implementation of an interface hook function to get more information about the keyboard or mouse.

You shouldn't call [`get_key_info()`](TheInputServer_Functions.md#get_key_info "get_key_info\(\)") or [`BView::GetMouse()`](BView.md#BView_GetMouse "GetMouse\(\)") in a loop to track the keyboard or mouse unless you really know what you're doing (some game developers may need to poll). To track the keyboard, you implement [`BView`](BView.md "BView")'s [`KeyDown()`](BView.md#BView_KeyDown "KeyDown\(\)") and [`KeyUp()`](BView.md#BView_KeyUp "KeyUp\(\)") functions. To track the mouse, implement [`MouseDown()`](BView.md#BView_MouseDown "MouseDown\(\)"), [`MouseMoved()`](BView.md#BView_MouseMoved "MouseMoved\(\)"), and [`MouseUp()`](BView.md#BView_MouseUp "MouseUp\(\)").

* * *

### Interface Messages

The interface messages are reasonably self-explanatory. See the notes below and the documentation of the individual hook functions for more information.

Message| Hook function  
---|---  
`[B_ZOOM](TheInterfaceKit_MessageConstants.md#B_ZOOM "B_ZOOM")`| [`BWindow::Zoom()`](BWindow.md#BWindow_Zoom "Zoom\(\)")  
`[B_MINIMIZE](TheInterfaceKit_MessageConstants.md#B_MINIMIZE "B_MINIMIZE")`| [`BWindow::Minimize()`](BWindow.md#BWindow_Minimize)  
`[B_KEY_DOWN](TheInterfaceKit_MessageConstants.md#B_KEY_DOWN "B_KEY_DOWN, B_KEY_UP, B_UNMAPPED_KEY_DOWN, B_UNMAPPED_KEY_UP")`| [`BView::KeyDown()`](BView.md#BView_KeyDown "KeyDown\(\)")  
`[B_KEY_UP](TheInterfaceKit_MessageConstants.md#B_KEY_UP)`| [`BView::KeyUp()`](BView.md#BView_KeyUp "KeyUp\(\)")  
`[B_MOUSE_DOWN](TheInterfaceKit_MessageConstants.md#B_MOUSE_DOWN "B_MOUSE_DOWN")`| [`BView::MouseDown()`](BView.md#BView_MouseDown "MouseDown\(\)")  
`[B_MOUSE_UP](TheInterfaceKit_MessageConstants.md#B_MOUSE_UP "B_MOUSE_UP")`| [`BView::MouseUp()`](BView.md#BView_MouseUp "MouseUp\(\)")  
`[B_MOUSE_MOVED](TheInterfaceKit_MessageConstants.md#B_MOUSE_MOVED "B_MOUSE_MOVED")`| [`BView::MouseMoved()`](BView.md#BView_MouseMoved "MouseMoved\(\)")  
`[B_VIEW_MOVED](TheInterfaceKit_MessageConstants.md#B_VIEW_MOVED "B_VIEW_MOVED")`| [`BView::FrameMoved()`](BView.md#BView_FrameMoved "FrameMoved\(\)")  
`[B_VIEW_RESIZED](TheInterfaceKit_MessageConstants.md#B_VIEW_RESIZED "B_VIEW_RESIZED")`| [`BView::FrameResized()`](BView.md#BView_FrameResized "FrameResized\(\)")  
`[B_VALUE_CHANGED](TheInterfaceKit_MessageConstants.md#B_VALUE_CHANGED "B_VALUE_CHANGED")`| [`BScrollBar::ValueChanged()`](BScrollBar.md#BScrollBar_ValueChanged "ValueChanged\(\)")  
`[B_WINDOW_ACTIVATED](TheInterfaceKit_MessageConstants.md#B_WINDOW_ACTIVATED "B_WINDOW_ACTIVATED")`| [`BWindow::WindowActivated()`](BWindow.md#BWindow_WindowActivated "WindowActivated\(\)") [`BView::WindowActivated()`](BView.md#BView_WindowActivated "WindowActivated\(\)")  
[`B_QUIT_REQUESTED`](TheApplicationKit_MessageConstants.md#B_QUIT_REQUESTED "B_QUIT_REQUESTED")| [`BLooper::QuitRequested()`](BLooper.md#BLooper_QuitRequested "QuitRequested\(\)")  
`[B_WINDOW_MOVED](TheInterfaceKit_MessageConstants.md#B_WINDOW_MOVED "B_WINDOW_MOVED")`| [`BWindow::FrameMoved()`](BWindow.md#BWindow_FrameMoved "FrameMoved\(\), FrameResized\(\)")  
`[B_WINDOW_RESIZED](TheInterfaceKit_MessageConstants.md#B_WINDOW_RESIZED "B_WINDOW_RESIZED")`| [`BWindow::FrameResized()`](BWindow.md#BWindow_FrameResized)  
`[B_SCREEN_CHANGED](TheInterfaceKit_MessageConstants.md#B_SCREEN_CHANGED "B_SCREEN_CHANGED")`| [`BWindow::ScreenChanged()`](BWindow.md#BWindow_ScreenChanged "ScreenChanged\(\)")  
`[B_WORKSPACE_ACTIVATED](TheInterfaceKit_MessageConstants.md#B_WORKSPACE_ACTIVATED "B_WORKSPACE_ACTIVATED")`| [`BWindow::WorkspaceActivated()`](BWindow.md#BWindow_WorkspaceActivated "WorkspaceActivated\(\)")  
`[B_WORKSPACES_CHANGED](TheInterfaceKit_MessageConstants.md#B_WORKSPACES_CHANGED "B_WORKSPACES_CHANGED")`| [`BWindow::WorkspacesChanged()`](BWindow.md#BWindow_WorkspacesChanged "WorkspacesChanged\(\)")  
`[B_PULSE](TheApplicationKit_MessageConstants.md#B_PULSE "B_PULSE")`| [`BView::Pulse()`](BView.md#BView_Pulse "Pulse\(\)")  
  
Notes:

  * `B_ZOOM` is usually caused by the user operating the zoom button in the window's title tab.

  * `B_MINIMIZE` tells the window to hide/show itself. This message is usually caused by the user double-clicking the window's tab (to hide), and clicking the window token in the application's window list (in the DeskBar).

  * `B_KEY_DOWN` and `B_KEY_UP` report that the user pressed and then release a character key. When the key is held down, repeated `B_KEY_DOWN` messages are sent. If a key is mapped to a string of characters, a `B_KEY_DOWN`/`B_KEY_UP` pair is generated for each character. Pressing a modifier keys on its own doesn't generate key messages.

  * `B_MOUSE_DOWN`, `B_MOUSE_MOVED`, and `B_MOUSE_UP` are delivered (as hook functions) to the [`BView`](BView.md "BView") that the mouse is over at the time, regardless of where it started. For example, if the user drags the mouse through your view, you'll get a series of `B_MOUSE_MOVED` messages without getting a `B_MOUSE_UP` or a `B_MOUSE_DOWN`. The [`BView::MouseMoved()`](BView.md#BView_MouseMoved "MouseMoved\(\)") function has a code that tells you whether the mouse is just entering, inside, or exiting your view. You can tell your [`BView`](BView.md "BView") to track the mouse even when it's outside your view through [`BView::SetMouseEventMask()`](BView.md#BView_SetMouseEventMask).

  * A `B_VALUE_CHANGED` message reports that the Application Server changed a value associated with a [`BScrollBar`](BScrollBar.md "BScrollBar") objects.

  * `B_WORKSPACE_ACTIVATED` reports that the active workspace (the one displayed on-screen) has changed. The message is only sent to windows that live in one of the affected workspaces.

  * `B_WORKSPACES_CHANGED` notifies the window that the set of workspaces in which it can be displayed has changed.

  * A [`BWindow`](BWindow.md "BWindow") reinterprets a [`B_QUIT_REQUESTED`](TheApplicationKit_MessageConstants.md#B_QUIT_REQUESTED "B_QUIT_REQUESTED") message, originally defined for the [`BLooper`](BLooper.md "BLooper") class in the Application Kit, to mean a user request to close the window. However, it doesn't redeclare the [`QuitRequested()`](BLooper.md#BLooper_QuitRequested "QuitRequested\(\)") hook function that it inherits from [`BLooper`](BLooper.md "BLooper").

#### Tracking the Keyboard

The view that's responsible for handling keyboard events is called as the focus view. The focus view is the view within the active window that's displaying the current selection, or the control that's marked to show that it can be operated from the keyboard. Only one view in the window can be in focus at a time.

You promote a view to focus by calling [`BView::MakeFocus()`](BView.md#BView_MakeFocus "MakeFocus\(\)"), and you ask for the current focus through [`BWindow::CurrentFocus()`](BWindow.md#BWindow_CurrentFocus "CurrentFocus\(\)").

#### Focus View Commands

The table below maps a keyboard action to the hook function it invokes or the message it sends. In each of these cases, the function or message is sent to the current focus view (which may change between a key down and a key up)

User action| Function or message  
---|---  
Press a character key| [`BView::KeyDown()`](BView.md#BView_KeyDown "KeyDown\(\)")  
Release a character key| [`BView::KeyUp()`](BView.md#BView_KeyUp "KeyUp\(\)")  
Command+a| `B_SELECT_ALL`  
Command+x| `B_CUT`  
Command+c| `B_COPY`  
Command+v| `B_PASTE`  
  
The focus view needn't respond to all of these functions and commands, but a [`BView`](BView.md "BView") that doesn't respond to any of them should never promote itself to focus.

#### Other Keyboard Commands

There are four other built-in keyboard commands:

User action| Message or Action| Target  
---|---|---  
Command+w| [`B_QUIT_REQUESTED`](TheApplicationKit_MessageConstants.md#B_QUIT_REQUESTED "B_QUIT_REQUESTED")| The active window.  
Command+q| [`B_QUIT_REQUESTED`](TheApplicationKit_MessageConstants.md#B_QUIT_REQUESTED "B_QUIT_REQUESTED")| The active app.  
Option+Tab| Changes focus to the next navigable view.| The active window.  
Enter| Activates the default button.| The active window.  
  
#### Kinds of Keyboard Messages

`B_KEY_UP` events are always assigned to the view that's in focus when the user releases the key—even if the previous `B_KEY_DOWN` message performed a shortcut, forced keyboard navigation, or was assigned to the default button.

#### Message Protocols

The event messages that the Application Server sends to a [`BWindow`](BWindow.md "BWindow") usually have more information in them than is passed to the corresponding hook function. For example, while a `B_MOUSE_DOWN` message knows where, when, and which mouse button was pressed (among other things), only the "where" information is passed to the [`MouseDown()`](BView.md#BView_MouseDown "MouseDown\(\)") function.

You can retrieve the message that initiated a hook function from within the hook function itself by calling [`BLooper::CurrentMessage()`](BLooper.md#BLooper_CurrentMessage "CurrentMessage\(\), DetachCurrentMessage\(\)") function:
    
    
    void MyView::MouseDown(BPoint where)
    {
       BMessage *msg = Window()->CurrentMessage();
       ...

* * *

### The User Interface

The Interface Kit provides interface mechanisms that your classes can participate in, if they coordinate with kit-defined code. Two such mechanisms are described below—keyboard navigation and the drag-and-drop delivery of messages.

#### Keyboard Navigation

Keyboard navigation is a mechanism for allowing users to manipulate views—especially buttons, check boxes, and other control devices—from the keyboard. It gives users the ability to:

  * Move the focus of keyboard actions from view to view within a window by pressing the Tab key.

  * Operate the view that's currently in focus by pressing spacebar and Enter (to invoke it) or the arrow keys (to move around inside it).

The first ability—navigation between views—is implemented by the Interface Kit. The second—navigation within a view—is up to individual applications (although the [`BControl`](BControl.md "BControl") class helps a little), as are most view-specific aspects of the user interface. The only trick, and it's not a difficult one, is to make the two kinds of navigation work together.

To participate in the navigation mechanism, a class derived from [`BView`](BView.md "BView") needs to coordinate three aspects of its code—setting navigation flags, drawing an indication that the [`BView`](BView.md "BView") is in focus, and responding to keyboard events. The following sections discuss each of these elements.

#### Setting Navigation Flags

The `B_NAVIGABLE` flag marks a [`BView`](BView.md "BView") as an eligible target for keyboard navigation. It's one flag in a mask that the [`BView`](BView.md "BView") constructor sets, along with other view attributes. For example:
    
    
    MyView::MyView(BRect frame, const char *name,
                            uint32 resizingMode, uint32 flags)
       : BView(frame, name, resizingMode, flags|B_NAVIGIBLE|B_WILL_DRAW)
    {
       . . .
    }

When the user presses the Tab key, the focus moves from one `B_NAVIGIBLE` target to the next, working first down and then across the view hierarchy. That is, if a [`BView`](BView.md "BView") has both `B_NAVIGIBLE` children and `B_NAVIGIBLE` siblings, the children will be targeted before the siblings.

The flag should be removed from the mask when the view is disabled or cannot become the focus view for any reason, and included again when it's re-enabled. The mask can be altered with the [`SetFlags()`](BView.md#BView_SetFlags "SetFlags\(\) , Flags\(\)") function:
    
    
    if ( /* cannot become the focus view */ )
       SetFlags(Flags() & ~B_NAVIGIBLE);
    else
       SetFlags(Flags() | B_NAVIGIBLE);

Most navigible [`BView`](BView.md "BView")s are control devices and derive from the [`BControl`](BControl.md "BControl") class. All [`BControl`](BControl.md "BControl") are navigible by default and [`BControl`](BControl.md "BControl") has a [`SetEnabled()`](BControl.md#BControl_SetEnabled "SetEnabled\(\), IsEnabled\(\)") function that turns the `B_NAVIGIBLE` flag on and off, so this work is already done for objects that inherit from [`BControl`](BControl.md "BControl").

You may also want to set a view's `B_NAVIGIBLE_JUMP` flag to permit larger jumps between navigible views. Control+Tab moves the focus from one group of views to another, where the groups are (hopefully) obvious to the user from their arrangement in the window.

`B_NAVIGIBLE_JUMP` marks positions in the view hierarchy for these larger jumps. When the user presses Control+Tab, focus jumps to the next `B_NAVIGIBLE_JUMP` view. If a `B_NAVIGABLE_JUMP` view isn't also `B_NAVIGABLE`, focus moves to the next available `B_NAVIGABLE` view. For example, if a `B_NAVIGABLE_JUMP` parent view is not navigible itself but has navigible children, Control+Tab focusses on the first `B_NAVIGABLE` child.

#### Drawing the Focus Indicator

When the user navigates to a view, the [`BView`](BView.md "BView") needs to draw some sort of visual indication that it's the current focus for keyboard actions. Be-defined views underline text (for example, a button label) when the view is in focus, or draw a rectangular outline of the view. The underline and outline are drawn in the color returned by [`keyboard_navigation_color()`](TheInterfaceKit_Functions.md#keyboard_navigation_color "keyboard_navigation_color\(\)"). Using this color lends consistency to the user interface.

A [`BView`](BView.md "BView") learns that the focus has changed when its [`MakeFocus()`](BView.md#BView_MakeFocus "MakeFocus\(\)") hook function is called. It's up to [`MakeFocus()`](BView.md#BView_MakeFocus "MakeFocus\(\)") to ensure that the focus indicator is drawn or erased, depending on the [`BView`](BView.md "BView")'s new status. It's usually simplest for [`MakeFocus()`](BView.md#BView_MakeFocus "MakeFocus\(\)") to call [`Draw()`](BView.md#BView_Draw "Draw\(\)") and have it do the work. For example:
    
    
    void MyView::MakeFocus(bool focused)
    {
       if ( focused != IsFocus() ) {
          baseClass::MakeFocus(focused);
          Draw(Bounds());
          Flush();
          . . .
       }
    }

The [`BControl`](BControl.md "BControl") class has a [`MakeFocus()`](BControl.md#BControl_MakeFocus "MakeFocus\(\)") function that calls [`Draw()`](BView.md#BView_Draw "Draw\(\)"), so if your class derives from [`BControl`](BControl.md "BControl"), all you need to do is implement [`Draw()`](BView.md#BView_Draw "Draw\(\)"). [`Draw()`](BView.md#BView_Draw "Draw\(\)") can call [`IsFocus()`](BView.md#BView_IsFocus "IsFocus\(\)") to test the [`BView`](BView.md "BView")'s current status. Here's a rough example:
    
    
    void MyView::Draw(BRect updateRect)
    {
       rgb_color navigationColor = keyboard_navigation_color();
       BRect r = Bounds()
       r.InsetBy(2.0, 2.0)
       . . .
       rgb_color c = HighColor();
       if ( IsFocus() ) {
          /* draw the indicator */
          SetHighColor(navigationColor);
          StrokeRect(r);
          SetHighColor(c);
       }
       else {
          /* erase the indicator */
          SetHighColor(ViewColor());
          StrokeRect(r);
          SetHighColor(c);
       }
       . . .
    }

#### Handling Keyboard Actions

Finally, your [`BView`](BView.md "BView") may need to override [`KeyDown()`](BView.md#BView_KeyDown "KeyDown\(\)") to handle the keystrokes that are used to operate the view (for view-internal navigation). Always incorporate the inherited version of [`KeyDown()`](BView.md#BView_KeyDown "KeyDown\(\)") so that it can take care of navigation between views. For example:
    
    
    void MyView::KeyDown(const char *bytes, int32 numBytes)
    {
       switch ( bytes[0] ) {
          case B_ENTER:
          case B_SPACE:
                /* take action */
                break;
          case B_UP_ARROW:
          case B_DOWN_ARROW:
          case B_RIGHT_ARROW:
          case B_LEFT_ARROW:
                /* move within the view */
                break;
          default:
                _baseClass_ ::KeyDown(bytes, numBytes);
                break;
       }
    }

Again, the [`BControl`](BControl.md "BControl") class implements a [`KeyDown()`](BControl.md#BControl_KeyDown "KeyDown\(\)") function that invokes the control device when the user presses the space bar or Enter key. If your class derives from [`BControl`](BControl.md "BControl") and it doesn't have to do any other view-internal navigation, the [`BControl`](BControl.md "BControl") function may be adequate for your needs.

#### Drag and Drop

The [`BView`](BView.md "BView") class supports a drag-and-drop user interface. The user can transfer a parcel of information from one place to another by dragging an image from a source view and dropping it on a destination view—perhaps a view in a different window in a different application.

A source [`BView`](BView.md "BView") initiates dragging by calling [`DragMessage()`](BView.md#BView_DragMessage "DragMessage\(\)") from within its [`MouseDown()`](BView.md#BView_MouseDown "MouseDown\(\)") function. The [`BView`](BView.md "BView") bundles all information relevant to the dragging session into a [`BMessage`](BMessage.md "BMessage") object and passes it to [`DragMessage()`](BView.md#BView_DragMessage "DragMessage\(\)"). It also passes an image or a rectangle to represent the data package on-screen. For example:
    
    
    void MyView::MouseDown(BPoint point)
    {
       . . .
       if ( aRect.Contains(point) ) {
          BMessage message(SOME_WORDS_OF_ENCOURAGEMENT);
          message.AddString("words", theEncouragingWords);
          DragMessage(&message, aRect);
       }
       . . .
    }

The Application Server then takes charge of the [`BMessage`](BMessage.md "BMessage") object and animates the image as the user drags it on-screen. As the image moves across the screen, the views it passes over are informed with [`MouseMoved()`](BView.md#BView_MouseMoved "MouseMoved\(\)") function calls. These notifications give views a chance to show the user whether or not they're willing to accept the message being dragged. When the user releases the mouse button, dropping the dragged message, the message is delivered to the [`BWindow`](BWindow.md "BWindow") and targeted to the destination [`BView`](BView.md "BView").

A [`BView`](BView.md "BView") is notified that a message has arrived by a [`MessageReceived()`](BView.md#BView_MessageReceived "MessageReceived\(\)") function call. This is the same function that announces the arrival of other messages. By calling [`WasDropped()`](BMessage.md#BMessage_WasDropped "WasDropped\(\), DropPoint\(\)"), you can ask the message whether it was dropped on the view or delivered in some other way. If it was dropped, you can find out where by calling [`DropPoint()`](BMessage.md#BMessage_DropPoint). For example:
    
    
    void AnotherView::MessageReceived(BMessage *message)
    {
       switch ( message->what ) {
          . . .
          case SOME_WORDS_OF_ENCOURAGEMENT:
          {
                char *words;
                if ( message->FindString("words", &words) != B_OK )
                   return;
                if ( message->WasDropped() ) {
                   BPoint where = DropPoint();
                   ConvertFromScreen(&where);
                   PleaseInsertTheseWords(words, where);
                }
                break;
          }
          . . .
          default:
                _baseClass_ ::MessageReceived(message);
       }
    }

Aside from creating a [`BMessage`](BMessage.md "BMessage") object and passing it to [`DragMessage()`](BView.md#BView_DragMessage "DragMessage\(\)"), or implementing [`MouseMoved()`](BView.md#BView_MouseMoved "MouseMoved\(\)") and [`MessageReceived()`](BView.md#BView_MessageReceived "MessageReceived\(\)") functions to handle any messages that come its way, there's nothing an application needs to do to support a drag-and-drop user interface. The bulk of the work is done by the Application Server and Interface Kit.

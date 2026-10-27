<!-- System Overview - The Interface Kit - BScrollBar -->

## BScrollBar  
  
A [`BScrollBar`](BScrollBar.md "BScrollBar") object displays a vertical or horizontal scroll bar that users can operate to scroll the contents of another view, a target view. Scroll bars usually are grouped as siblings of the target view under a common parent. That way, when the parent is resized, the target and scroll bars can be automatically resized to match. (A companion class, [`BScrollView`](BScrollView.md "BScrollView"), defines just such a container view; a [`BScrollView`](BScrollView.md "BScrollView") object sets up the scroll bars for a target view and makes itself the parent of the target and the scroll bars.)

* * *

### The Update Mechanism

[`BScrollBar`](BScrollBar.md "BScrollBar")s are different from other views in one important respect: All their drawing and event handling is carried out within the Application Server, not in the application. A [`BScrollBar`](BScrollBar.md "BScrollBar") object doesn't receive [`Draw()`](BView.md#BView_Draw "Draw\(\)") or [`MouseDown()`](BView.md#BView_MouseDown "MouseDown\(\)") notifications; the server intercepts updates and interface messages that would otherwise be reported to the [`BScrollBar`](BScrollBar.md "BScrollBar") and handles them itself. As the user moves the knob on a scroll bar or presses a scroll arrow, the Application Server continuously refreshes the scroll bar's image on-screen and informs the application with a steady stream of `B_VALUE_CHANGED` messages.

The window dispatches these messages by calling the [`BScrollBar`](BScrollBar.md "BScrollBar")'s [`ValueChanged()`](BScrollBar.md#BScrollBar_ValueChanged "ValueChanged\(\)") function. Each function call notifies the [`BScrollBar`](BScrollBar.md "BScrollBar") of a change in its value and, consequently, of a need to scroll the target view.

Confining the update mechanism for scroll bars to the Application Server limits the volume of communication between the application and server and enhances the efficiency of scrolling. The application's messages to the server can concentrate on updating the target view as its contents are being scrolled, rather than on updating the scroll bars themselves.

* * *

### Value and Range

A scroll bar's value determines what the target view displays. The assumption is that the left coordinate value of the target view's bounds rectangle should match the value of the horizontal scroll bar, and the top of the target view's bounds rectangle should match the value of the vertical scroll bar. When a [`BScrollBar`](BScrollBar.md "BScrollBar") is notified of a change of value (through [`ValueChanged()`](BScrollBar.md#BScrollBar_ValueChanged "ValueChanged\(\)")), it calls the target view's [`ScrollTo()`](BView.md#BView_ScrollTo) function to put the new value at the left or top of the bounds rectangle.

The value reported in a [`ValueChanged()`](BScrollBar.md#BScrollBar_ValueChanged "ValueChanged\(\)") notification and passed to [`ScrollTo()`](BView.md#BView_ScrollTo) depends on where the user moves the scroll bar's knob and on the range of values the scroll bar represents. The range is first set in the [`BScrollBar`](BScrollBar.md "BScrollBar") constructor and can be modified by the [`SetRange()`](BScrollBar.md#BScrollBar_SetRange "SetRange\(\), GetRange\(\)") function.

The range must be large enough to bring all the coordinate values where the target view can draw into its bounds rectangle. If everything the target view can draw is conceived as being enclosed in a "data rectangle", the range of a horizontal scroll bar must extend from a minimum that makes the left side of the target's bounds rectangle coincide with the left side of its data rectangle, to a maximum that puts the right side of the bounds rectangle at the right side of the data rectangle. This is illustrated in part below:

![Scrolling A View](./images/TheInterfaceKit/scrollbar.png)

As this illustration helps demonstrate, the maximum value of a horizontal scroll bar can be no less than the right coordinate value of the data rectangle minus the width of the bounds rectangle. Similarly, for a vertical scroll bar, the maximum value can be no less than the bottom coordinate of the data rectangle minus the height of the bounds rectangle. The range of a scroll bar subtracts the dimensions of the target's bounds rectangle from its data rectangle. (The minimum values of horizontal and vertical scroll bars can be no greater than the left and top sides of the data rectangle.)

What the target view can draw may change from time to time as the user adds or deletes data. As this happens, the range of the scroll bar should be updated with the [`SetRange()`](BScrollBar.md#BScrollBar_SetRange "SetRange\(\), GetRange\(\)") function. The range may also need to be recalculated when the target view is resized.

* * *

### Coordination

Scroll bars control the target view, but a target can also be scrolled without the intervention of its scroll bars (by calling [`ScrollTo()`](BView.md#BView_ScrollTo) or [`ScrollBy()`](BView.md#BView_ScrollBy "ScrollBy\(\), ScrollTo\(\)") directly). Therefore, not only must a scroll bar know about its target, but a target view must know about its scroll bars. When a [`BScrollBar`](BScrollBar.md "BScrollBar") sets its target, the target [`BView`](BView.md "BView") is notified and records the identity of the [`BScrollBar`](BScrollBar.md "BScrollBar").

The two objects communicate whenever the display changes: When the scroll bar is the instrument that initiates scrolling, [`ValueChanged()`](BScrollBar.md#BScrollBar_ValueChanged "ValueChanged\(\)") calls the target view's [`ScrollTo()`](BView.md#BView_ScrollTo) function. To cover cases of target-initiated scrolling, [`ScrollTo()`](BView.md#BView_ScrollTo) calls the BScrollBar's [`SetValue()`](BScrollBar.md#BScrollBar_SetValue "SetValue\(\), Value\(\)") function so that the scroll bars can be updated on-screen. [`SetValue()`](BScrollBar.md#BScrollBar_SetValue "SetValue\(\), Value\(\)") in turn calls [`ValueChanged()`](BScrollBar.md#BScrollBar_ValueChanged "ValueChanged\(\)"), which makes sure the exchange of function calls doesn't get too circular.

* * *

### Scroll Bar Options

Users have control over some aspects of how scroll bars look and behave. With the ScrollBar preferences application, they can choose:

  * Whether the knob should be a fixed size, or whether it should grow and shrink to proportionally represent how much of a document (how much of the data rectangle) is visible within the target view. A proportional knob is the default.

  * Whether double, bidirectional scroll arrows should appear on each end of the scroll bar, or whether each end should have only a single, unidirectional arrow. Double arrows are the default.

  * Which of three patterns should appear on the knob.

  * What the size of the knob should be—the minimum length of a proportional knob or the fixed length of a knob that's not proportional. The default length is 15 pixels.

When this class constructs a new [`BScrollBar`](BScrollBar.md "BScrollBar"), it conforms the object to the choices the user has made.

See also: [`set_scroll_bar_info()`](TheInterfaceKit_Functions.md#set_scroll_bar_info "set_scroll_bar_info\(\), get_scroll_bar_info\(\)"), [`BView::ScrollBar()`](BView.md#BView_ScrollBar "ScrollBar\(\)"), the [`BScrollView`](BScrollView.md "BScrollView") class

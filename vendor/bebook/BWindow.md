<!-- Classes And Methods - The Interface Kit - BWindow -->

## BWindow

| Derived From:| [BLooper](BLooper.md "BLooper") > [BHandler](BHandler.md "BHandler")  
---|---  
Mix-in Classes:| [BArchivable](BArchivable.md "BArchivable")  
Declared In:| `interface/Window.h`  
Library:| `libbe.so`  
Allocation:| Constructor only  
[Class Overview](BWindow_Overview.md "BWindow")

  * [Constructor and Destructor](BWindow.md#BWindow_ConstructorDestructor)
  * [Hook Functions](BWindow.md#BWindow_HookFunctions)
  * [Member Functions](BWindow.md#BWindow_MemberFunctions)
  * [Static Functions](BWindow.md#BWindow_StaticFunctions)
  * [Constants and Defined Types](BWindow.md#BWindow_ConstantsandDefinedTypes)
  * [Scripting Support](BWindow.md#BWindow_Scripting)
  * [Archived Fields](BWindow.md#BWindow_Archived_Fields)

  
  
* * *

### Constructor and Destructor

#### BWindow()

`BWindow(BRect frame,  
const char* title,  
window_type type,  
uint32 flags,  
uint32 workspaces = B_CURRENT_WORKSPACE);` `BWindow(BRect frame,  
const char* title,  
window_look look,  
window_feel feel,  
uint32 flags,  
uint32 workspaces = B_CURRENT_WORKSPACE);` `BWindow([`BMessage`](BMessage.md "BMessage")* archive);`

Creates a new window:

Field| Description  
---|---  
`frame`| Is the window's content area rectangle given in screen coordinates. The window's border and title tab (if any) are wrapped around the content area. The frame's coordinates are rounded to the nearest whole number. You can further restrict the window's frame through [`SetWindowAlignment()`](BWindow.md#BWindow_SetWindowAlignment "SetWindowAlignment\(\), GetWindowAlignment\(\)") and [`SetSizeLimits()`](BWindow.md#BWindow_SetSizeLimits "SetSizeLimits\(\), GetSizeLimits\(\), SetZoomLimits\(\)").  
`title`| Is the string that's displayed in the window's title tab (if it has one). The string is also used, with the prefix "w>", as the name of the window's looper thread: "w>`title`". You can reset the title through [`SetTitle()`](BWindow.md#BWindow_SetTitle "SetTitle\(\), Title\(\)").  
`look`/`feel` or `type`.| The appearance and behavior of the window is defined by a combination of the `look` and `feel` arguments, or by a single `type`. See the [window_look](BWindow.md#BWindow_window_look "window_look"), [window_feel](BWindow.md#BWindow_window_feel "window_feel"), and [window_type](BWindow.md#BWindow_window_type "window_type") descriptions for a list of values. The look and feel can be reset through [`SetLook()`](BWindow.md#BWindow_SetLook "SetLook\(\), Look\(\)"), [`SetFeel()`](BWindow.md#BWindow_SetFeel "SetFeel\(\), Feel\(\)"), and [`SetType()`](BWindow.md#BWindow_SetType "SetType\(\), Type\(\)").  
`flags`| The `flags` argument is a mask that defines the window's "user attributes"—whether the user can resize or close the window, whether the window avoids front when the user closes other windows, and so on. See "[Window Flags](BWindow.md#BWindow_Window_Flags "Window Flags")" for a list of values. The flags can be reset through [`SetFlags()`](BWindow.md#BWindow_SetFlags "SetFlags\(\), Flags\(\)").  
`workspaces`| Is a mask that tells the window which of the 32 potential workspace(s) it should be displayed in (it can be displayed in more than one workspace at the same time). For example, a workspaces of (1<<2)+(1<<7) tells the window to display itself in workspaces 2 and 7. Instead of passing a bitmask, you can also use the `B_CURRENT_WORKSPACE` or `B_ALL_WORKSPACES` constant. By default, the window appears in the current (active) workspace. You can reset the workspaces through [`SetWorkspaces()`](BWindow.md#BWindow_SetWorkspaces "SetWorkspaces\(\), Workspaces\(\)").  
  
A freshly-created window is hidden and locked. After construction, you add [`BView`](BView.md "BView")s to the `BWindow` through [`AddChild()`](BWindow.md#BWindow_AddChild "AddChild\(\), RemoveChild\(\), ChildAt\(\), CountChildren\(\)"), and then show the window (and start its message loop) by calling [`Show()`](BWindow.md#BWindow_Show "Show\(\), Hide\(\), IsHidden\(\), Minimize\(\), IsMinimized\(\)").

#### ~BWindow()

You never delete your `BWindow`s; call [`Quit()`](BWindow.md#BWindow_Quit "Quit\(\)") instead.

* * *

### Hook Functions

####  FrameMoved(), FrameResized()

`virtual void FrameMoved([`BPoint`](BPoint.md "BPoint")* origin);``virtual void FrameResized(float width,  
float height);`

These hook functions are invoked just after the window's frame is moved or resized, whether by the user or programatically. The arguments give the window's new origin (in screen coordinates) or dimensions. The default implementations do nothing.

See also: `B_WINDOW_MOVED`, `B_WINDOW_RESIZED`.

####  MenusBeginning(), MenusEnded()

`virtual void MenusBeginning();``virtual void MenusEnded();`

The `MenusBeginning()` hook function is called just before menus belonging to the window are about to be shown to the user. `MenusEnded()` is called when the menus have been removed from the screen. The default implementations do nothing. You can implement these functions to make sure the menus' states accurately reflect the state of the window.

Important

![Important](./images/admonitions/Warning_32.png)

These hook functions are not invoked because of messages—don't go looking for a "menus beginning" or "menus ended" message.

#### MessageReceived()

`virtual void MessageReceived([`BMessage`](BMessage.md "BMessage")* message);`

Implementation detail. See [`BHandler::MessageReceived()`](BHandler.md#BHandler_MessageReceived "MessageReceived\(\)").

#### ScreenChanged()

`virtual void ScreenChanged(BRect frame,  
color_space mode);`

Hook function that's called when the screen (on which this window is located) changes size or location in the screen coordinate system, or changes color space (depth). `frame` is the screen's new frame rectangle, and `mode` is its new color space.

See also: [`BScreen::Frame()`](BScreen.md#BScreen_Frame "Frame\(\)")

#### WindowActivated()

`virtual void WindowActivated(bool active);`

`WindowActivated()` is a hook function that's automatically invoked on the `BWindow` (and each of its [`BView`](BView.md "BView") children) when the window is activated or deactivated. If `active` is `true`, the window has just become the active window; if it's `false`, it's about to give up that status. You can implement `WindowActivated()` to do whatever you want; the default implementation does nothing.

#### WorkspaceActivated()

`virtual void WorkspaceActivated(int32 workspace,  
bool active);`

Implemented by derived classes to respond to a notification that the workspace displayed on the screen has changed. All windows in the newly activated workspace as well as those in the one that was just deactivated get this notification.

The `workspace` argument is an index to the workspace in question and the `active` flag conveys its current status. If `active` is `true`, the workspace has just become the active workspace. If `active` is `false`, it has just stopped being the active workspace.

The default (`BWindow`) version of this function is empty.

See also: "`B_WORKSPACE_ACTIVATED`" in the Message Protocols appendix, [`activate_workspace()`](TheInterfaceKit_Functions.md#activate_workspace "activate_workspace\(\), current_workspace\(\)")

#### WorkspacesChanged()

`virtual void WorkspacesChanged(uint32 oldWorkspaces,  
uint32 newWorkspaces);`

Implemented by derived classes to respond to a notification the the window has just changed the set of workspaces in which it can be displayed from `oldWorkspaces` to `newWorkspaces`. This typically happens when the user moves a window from one workspace to another, but it may also happen when a programmatic change is made to the set of permitted workspaces. Each workspace is represented by a corresponding bit in the `oldWorkspaces` and `newWorkspaces` masks.

The default (`BWindow`) version of this function is empty.

See also: `B_WORKSPACES_CHANGED` in the Message Protocols appendix, [`SetWorkspaces()`](BWindow.md#BWindow_SetWorkspaces "SetWorkspaces\(\), Workspaces\(\)")

#### Zoom()

`void Zoom();` `virtual void Zoom([`BPoint`](BPoint.md "BPoint") origin,  
float width,  
float height);`

The non-virtual `Zoom()` (which is called when the user clicks the zoom button, but can also be called programatically) figures out a new size and location for the window (as described below), and then passes these dimensions to the virtual `Zoom()` hook function. It's hook `Zoom()`'s responsibility to actually resize and move the window; the default version applies the arguments explicitly by calling [`MoveTo()`](BWindow.md#BWindow_MoveTo "MoveBy\(\), MoveTo\(\)") and [`ResizeTo()`](BWindow.md#BWindow_ResizeTo). You can re-implement hook `Zoom()` to create a new zooming algorithm that incorporates or ignores the arguments as you see fit.

The dimensions that non-virtual `Zoom()` passes to hook `Zoom()` are deduced from the smallest of three rectangles: 1) the screen rectangle, 2) the rectangle defined by [`SetZoomLimits()`](BWindow.md#BWindow_SetZoomLimits), 3) the rectangle defined by [`SetSizeLimits()`](BWindow.md#BWindow_SetSizeLimits "SetSizeLimits\(\), GetSizeLimits\(\), SetZoomLimits\(\)"). However, if the window's rectangle already matches these "zoom" dimensions (give or take a few pixels), `Zoom()` passes the window's previous ("non-zoomed") size and location.

You can effectively call `Zoom()` even if the window is `B_NOT_ZOOMABLE`.

`Zoom()` may both move and resize the window, resulting in [`FrameMoved()`](BWindow.md#BWindow_FrameMoved "FrameMoved\(\), FrameResized\(\)") and [`FrameResized()`](BWindow.md#BWindow_FrameResized) notifications.

* * *

### Member Functions

####  Activate(), IsActive()

`void Activate(bool flag = true);``bool IsActive() const;`

`Activate()` makes the `BWindow` the active window (if `flag` is `true`), or causes it to relinquish that status (if `flag` is `false`). The active window is made frontmost, its title tab is highlighted, and it becomes the target of keyboard events. The [`Show()`](BWindow.md#BWindow_Show "Show\(\), Hide\(\), IsHidden\(\), Minimize\(\), IsMinimized\(\)") function automatically activates the window.

Important

![Important](./images/admonitions/Warning_32.png)

You can't activate a hidden window.

Note

![Note](./images/admonitions/Info_32.png)

You rarely call `Activate()` yourself. Deciding which window to make active is the user's choice.

`IsActive()` returns `true` if the window is currently the active window, and `false` if it's not.

See also: [`BView::WindowActivated()`](BView.md#BView_WindowActivated "WindowActivated\(\)")

####  AddChild(), RemoveChild(), ChildAt(), CountChildren()

`void AddChild([`BView`](BView.md "BView")* aView,  
[`BView`](BView.md "BView")* sibling = NULL);``bool RemoveChild([`BView`](BView.md "BView")* aView);``[`BView`](BView.md "BView")* ChildAt(int32 index) const;``int32 CountChildren() const;`

`AddChild()` places `aView` (and its child views) in the window, adds it to the window's view list, and adds it to the window's list of handlers:

  * Graphically, the view is placed in the window's coordinate system at the location defined by the view's frame rectangle.

  * In the window's view list, `aView` is inserted before `sibling`. if `sibling` is `NULL`, `aView` is added at the end of the list. Note, however, that window list order is of little significance; for example, it doesn't affect the order in which sibling views are drawn.

  * `aView` and its children are added to the window's handler list; `aView`'s next handler is set to this `BWindow`.

Each [`BView`](BView.md "BView") in aView's hierarchy is sent an [`AttachedToWindow()`](BView.md#BView_AttachedToWindow "AttachedToWindow\(\), AllAttached\(\)") call. When they've all had a chance to respond, they're each sent an [`AllAttached()`](BView.md#BView_AllAttached) call.

If `aView` has already been added to a view hierarchy, or if `sibling` isn't in the window's view list, `AddChild()` fails.

`RemoveChild()` removes `aView` (and its children) from the window's display, view list, and handler list, and sets `aView`'s next handler to `NULL`.

[`DetachedFromWindow()`](BView.md#BView_DetachedFromWindow "DetachedFromWindow\(\), AllDetached\(\)") and [`AllDetached()`](BView.md#BView_AllDetached) are invoked on `aView` and each of its children. If `aView` isn't in the window's view list, the function fails and returns `false`; it returns `true` upon success.

`ChildAt()` returns the `index`'th view in the window's view list, or `NULL` if `index` is out of bounds (you've reached the end of the list). The view list doesn't recurse; to build a full map of a window's view hierarchy, you call [`BView::ChildAt()`](BView.md#BView_ChildAt) iteratively on each of the window's views (and each of their children, etc.).

See also: [`BView::Parent()`](BView.md#BView_Parent "Parent\(\), NextSibling\(\), PreviousSibling\(\), ChildAt\(\), CountChildren\(\)")

####  AddShortcut(), RemoveShortcut()

`void AddShortcut(uint32 key,  
uint32 modifiers,  
[`BMessage`](BMessage.md "BMessage")* message);` `void AddShortcut(uint32 key,  
uint32 modifiers,  
[`BMessage`](BMessage.md "BMessage")* message,  
[`BHandler`](BHandler.md "BHandler")* handler);`

`void RemoveShortcut(uint32 key,  
uint32 modifiers);`

`AddShortcut()` creates a keyboard shortcut: When the user types Command+`modifiers`+`key`, `message` is sent to `handler`. If a shortcut already exists for `modifiers`+`key`, it's removed before the new shortcut is added.

`RemoveShortcut()` removes the shortcut for `modifiers`+`key`.

Warning

![Warning](./images/admonitions/Stop_32.png)

Don't use these functions to create and remove menu shortcuts; use [`BMenuItem`](BMenuItem.md "BMenuItem") objects instead.

Notes on the arguments:

  * `key` is a case-insensitive character value. If you want to map to an uppercase character, you have to include `B_SHIFT_KEY` in the `modifiers` mask.

  * `modifiers` is an OR'd list of modifier key numbers. `B_COMMAND_KEY`, which is always assumed, needn't be added to the mask. See [modifiers()](TheInputServer_Functions.md#modifiers "modifiers\(\)") for a list of modifier keys.

  * `message` is a model of the [`BMessage`](BMessage.md "BMessage") you want sent when the user types the shortcut. The `BWindow` takes ownership of the `message` object and adds a `when` field to it:

Field name| Type code| Description  
---|---|---  
`when`| `B_INT64_TYPE`| The time of the key-down, in microseconds since 01/01/70.  
  
  * `handler` must be in the window's handler list (or the message won't be sent). If you exclude the argument, this `BWindow` handles the message. If handler is (literally) `NULL`, the message is sent to the `BWindow`'s focus view (or to the `BWindow` if no view is in focus).

As with all Command events, a `B_KEY_DOWN` message isn't sent when the user invokes a keyboard shortcut, but the subsequent `B_KEY_UP` is.

Every `BWindow` has five built-in shortcuts:

Shortcut| Message| Handler  
---|---|---  
Command+x| `B_CUT`| the focus view  
Command+c| `B_COPY`| the focus view  
Command+v| `B_PASTE`| the focus view  
Command+a| `B_SELECT_ALL`| the focus view  
Command+w (closable windows only)| [`B_QUIT_REQUESTED`](TheApplicationKit_MessageConstants.md#B_QUIT_REQUESTED "B_QUIT_REQUESTED")| the `BWindow`  
  
In addition, `BWindow`s respond to Command+q by posting [`B_QUIT_REQUESTED`](TheApplicationKit_MessageConstants.md#B_QUIT_REQUESTED "B_QUIT_REQUESTED") to `be_app`.

####  AddToSubset(), RemoveFromSubset()

`status_t AddToSubset([`BWindow`](BWindow.md "BWindow")* window);``status_t RemoveFromSubset([`BWindow`](BWindow.md "BWindow")* window);`

Adds windows to and removes them from this window's subset. This affects modal and floating windows with a subset feel only (i.e. `B_MODAL_SUBSET_WINDOW_FEEL` or `B_FLOATING_SUBSET_WINDOW_FEEL`). A subset feel window blocks or floats above only those windows in its subset. To set the window's feel, use [`SetFeel()`](BWindow.md#BWindow_SetFeel "SetFeel\(\), Feel\(\)").

#### Archive()

`virtual status_t Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Archives the `BWindow` by recording its frame rectangle, title, type, and flags in the [`BMessage`](BMessage.md "BMessage") `archive`. If the `deep` flag is `true`, this function also archives all the views in the window's view hierarchy. If the flag is `false`, only the `BWindow` is archived.

See also: [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)"), [`Instantiate()`](BWindow.md#BWindow_Instantiate "Instantiate\(\)") static function

####  Bounds(), Frame()

`[`BRect`](BRect.md "BRect") Bounds() const;``[`BRect`](BRect.md "BRect") Frame() const;`

These functions return the rectangle that encloses the window's content area. The bounds rectangle (`Bounds()`) is expressed in the window's coordinate system; the frame rectangle (`Frame()`) is expressed in the screen's coordinate system.

The rectangles are cached by the `BWindow` itself—calling these functions doesn't incur a trip to the App Server.

####  ConvertToScreen(), ConvertFromScreen()

`[`BPoint`](BPoint.md "BPoint") ConvertToScreen([`BPoint`](BPoint.md "BPoint") windowPoint) const;` `void ConvertToScreen([`BPoint`](BPoint.md "BPoint")* windowPoint) const;` `[`BRect`](BRect.md "BRect") ConvertToScreen([`BRect`](BRect.md "BRect") windowRect) const;` `void ConvertToScreen([`BRect`](BRect.md "BRect")* windowRect) const;`

`[`BPoint`](BPoint.md "BPoint") ConvertFromScreen([`BPoint`](BPoint.md "BPoint") windowPoint) const;` `void ConvertFromScreen([`BPoint`](BPoint.md "BPoint")* windowPoint) const;` `[`BRect`](BRect.md "BRect") ConvertFromScreen([`BRect`](BRect.md "BRect") windowRect) const;` `void ConvertFromScreen([`BRect`](BRect.md "BRect")* windowRect) const;`

Converts the argument from window coordinates to screen coordinates or vice versa. The point or rect needn't fall within this `BWindow`'s bounds.

If the argument is passed by value, the function returns the converted value; if it's by pointer, the conversion is done in-place.

The `BWindow` must be locked.

#### CurrentFocus()

`[`BView`](BView.md "BView")* CurrentFocus() const;`

returns the current focus view for the `BWindow`, or `NULL` if no view is currently in focus. The focus view is the [`BView`](BView.md "BView") that's responsible for showing the current selection and is the target for keyboard messages directed at this `BWindow`. The focus view is set through [`BView::MakeFocus()`](BView.md#BView_MakeFocus "MakeFocus\(\)").

The `BWindow` sets its preferred handler to be the focus view, so the inherited [`PreferredHandler()`](BLooper.md#BLooper_PreferredHandler) function will return this same object (but as a [`BHandler`](BHandler.md "BHandler")).

####  DisableUpdates(), EnableUpdates()

`void DisableUpdates();``void EnableUpdates();`

These functions disable automatic updating within the window, and re-enable it again. Any drawing that's done while updates are disabled is suppressed until updates are re-enabled. If you're doing a lot of drawing within the window, and you want the results of the drawing to appear all at once, you should disable updates, draw, and then re-enable updates.

See also: [`BView::Invalidate()`](BView.md#BView_Invalidate "Invalidate\(\)"), [`UpdateIfNeeded()`](BWindow.md#BWindow_UpdateIfNeeded "UpdateIfNeeded\(\)")

#### DispatchMessage()

Implementation detail; see [`BLooper::DispatchMessage()`](BLooper.md#BLooper_DispatchMessage "DispatchMessage\(\)").

Warning

![Warning](./images/admonitions/Stop_32.png)

You shouldn't override this function in a `BWindow` subclass; if you want to augment the window's message-dispatching mechanism, override [`MessageReceived()`](BWindow.md#BWindow_MessageReceived "MessageReceived\(\)").

#### FindView()

`[`BView`](BView.md "BView")* FindView([`BPoint`](BPoint.md "BPoint")* point) const;` `[`BView`](BView.md "BView")* FindView(const char* name) const;`

Returns the view located at `point` within the window, or the view tagged with `name`. The functions returns `NULL` if no view is found.

####  Flush(), Sync()

`void Flush() const;``void Sync() const;`

Both of these functions cause this window's App Server-bound messages to be sent immediately. `Flush()` sends the messages and returns immediately; `Sync()` send the messages and waits for the App Server to respond. In other words, when `Sync()` returns you're guaranteed that all of the flushed messages have been processed.

See also: [`BView::Flush()`](BView.md#BView_Flush "Flush\(\), Sync\(\)")

#### GetSupportedSuites()

`virtual status_t GetSupportedSuites([`BMessage`](BMessage.md "BMessage")* message);`

Adds the name "suite/vnd.Be-window" to the message.

See also: [`BHandler::GetSupportedSuites()`](BHandler.md#BHandler_GetSupportedSuites "GetSupportedSuites\(\)")

####  IsFront(), IsFloating(), IsModal()

`bool IsFront() const;``bool IsFloating() const;``bool IsModal() const;`

These functions return `true` if the window is frontmost on screen, if it has a floating window feel, and if it has a modal window feel, respectively.

Note

![Note](./images/admonitions/Info_32.png)

A floating window can never be the frontmost window.

#### LastMouseMovedView()

`[`BView`](BView.md "BView")* LastMouseMovedView();`

Returns a pointer to the view in this window that most recently received a `B_MOUSE_MOVED` message.

####  MoveBy(), MoveTo()

`void MoveBy(float horizontal,  
float vertical);`

`void MoveTo([`BPoint`](BPoint.md "BPoint") point);` `void MoveTo(float x,  
float y);`

These functions move the window without resizing it. `MoveBy()` adds `horizontal` coordinate units to the left and right components of the window's frame rectangle and `vertical` units to the frame's top and bottom. if `horizontal` and `vertical` are negative, the window moves upward and to the left. if they're positive, it moves downward and to the right. `MoveTo()` moves the left top corner of the window's content area to `point`—or (`x`, `y`) – in the screen coordinate system; it adjusts all coordinates in the frame rectangle accordingly.

None of the values passed to these functions should specify fractional coordinates; a window must be aligned on screen pixels. Fractional values will be rounded to the closest whole number.

Neither function alters the `BWindow`'s coordinate system or bounds rectangle.

When these functions move a window, a window-moved event is reported to the window. This results in the `BWindow`'s [`FrameMoved()`](BWindow.md#BWindow_FrameMoved "FrameMoved\(\), FrameResized\(\)") function being called.

#### NeedsUpdate()

`bool NeedsUpdate() const;`

Returns `true` if any of the views within the window need to be updated, and `false` if they're all up-to-date.

####  OpenViewTransaction(), CommitViewTransaction()

`virtual void OpenViewTransaction();``virtual void CommitViewTransaction();`

These two functions bracket a series of "batched" drawing instructions. After you call `OpenViewTransaction()`, everything you draw in the window is bundled up and then sent to the app server (and rendered) when you call `CommitViewTransaction()`. You can only perform one transaction (per window) at a time. The `BWindow` must be locked when you call `OpenViewTransaction()`, and must remain locked until after you call `CommitViewTransaction()`. Invocations of [`Flush()`](BWindow.md#BWindow_Flush "Flush\(\), Sync\(\)") are ignored while a transaction is open (and locked).

#### Quit()

`virtual void Quit();`

`Quit()` removes the window from the screen, deletes all the [`BView`](BView.md "BView")s in its view hierarchy, destroys the window thread, removes the window's connection to the Application Server, and deletes the `BWindow` object.

Use this function, rather than the `delete` operator, to destroy a window.

`BWindow`'s `Quit()` works much like the [`BLooper`](BLooper.md "BLooper") function it overrides. When called from the `BWindow`'s thread, it doesn't return. When called from another thread, it returns after all previously posted messages have been processed and the `BWindow` and its thread have been destroyed.

Note

![Note](./images/admonitions/Info_32.png)

The window must be locked when you call `Quit()`.

See also: [`BLooper::QuitRequested()`](BLooper.md#BLooper_QuitRequested "QuitRequested\(\)"), [`BLooper::Quit()`](BLooper.md#BLooper_Quit "Quit\(\)"), [`BApplication::QuitRequested()`](BApplication.md#BApplication_QuitRequested "QuitRequested\(\)")

####  ResizeBy(), ResizeTo()

`void ResizeBy(float horizontal,  
float vertical);``void ResizeTo(float width,  
float height);`

These functions resize the window, while keeping its left top corner constant. `ResizeBy()` adds `horizontal` pixels to the width of the window's frame and `vertical` pixels to its height. `ResizeTo()` sets the frame absolutely to [`width`, `height`] pixels. Fractional components are rounded to the nearest whole number.

The [`FrameResized()`](BWindow.md#BWindow_FrameResized) hook function is called after the frame has been resized.

#### ResolveSpecifier()

`virtual [`BHandler`](BHandler.md "BHandler")* ResolveSpecifier([`BMessage`](BMessage.md "BMessage")* message,  
int32 index,  
[`BMessage`](BMessage.md "BMessage")* specifier,  
int32 command,  
const char* property);`

Resolves specifiers for the "Frame", "Title", and "View" properties. See "[Scripting Support](BWindow.md#BWindow_Scripting "Scripting Support")" in the class overview for more information.

See also: [`BHandler::ResolveSpecifier()`](BHandler.md#BHandler_ResolveSpecifier "ResolveSpecifier\(\)")

#### SendBehind()

`status_t SendBehind(const [`BWindow`](BWindow.md "BWindow")* window);`

Relayers the windows on the screen so this window is behind `window`.

####  SetDefaultButton(), DefaultButton()

`void SetDefaultButton([`BButton`](BButton.md "BButton")* button);``[`BButton`](BButton.md "BButton")* DefaultButton() const;`

Set and return the window's default button. This is the button that's mapped to the Enter key. The user can activate the default button at any time—even if another [`BView`](BView.md "BView") is the focus view (the focus view will not receive a `B_KEY_DOWN` message). To remove the current default (without promoting another button) call SetDefaultButton(NULL). There can only be one default button at a time; `SetDefaultButton()` demotes the previous default.

When you promote or demote a default button, it's automatically redisplayed and receives a [`BButton::MakeDefault()`](BButton.md#BButton_MakeDefault "MakeDefault\(\)") call.

####  SetFeel(), Feel()

`status_t SetFeel(window_feel feel);``window_feel Feel() const;`

`SetFeel()` changes the window's feel to the specified value.

`Feel()` returns the current feel of the window.

See the `BWindow` [constructor](BWindow.md#BWindow_Constructor "BWindow\(\)") for a list of window_feel constants.

####  SetFlags(), Flags()

`status_t SetFlags(uint32 flags);``uint32 Flags() const;`

`SetFlags()` set the window's flags (or "user attributes") to the specified combination. `Flags()` returns the current flags.

See "[Window Flags](BWindow.md#BWindow_Window_Flags "Window Flags")". for a list of the flag values.

####  SetKeyMenuBar(), KeyMenuBar()

`void SetKeyMenuBar([`BMenuBar`](BMenuBar.md "BMenuBar")* menuBar);``[`BMenuBar`](BMenuBar.md "BMenuBar")* KeyMenuBar() const;`

`SetKeyMenuBar()` makes the specified [`BMenuBar`](BMenuBar.md "BMenuBar") object the "key" menu bar for the window—the object that's at the root of the menu hierarchy that users can navigate using the keyboard. `KeyMenuBar()` returns the object with key status, or `NULL` if the window doesn't have a [`BMenuBar`](BMenuBar.md "BMenuBar") object in its view hierarchy.

If a window contains only one [`BMenuBar`](BMenuBar.md "BMenuBar") view, it's automatically designated the key menu bar. If there's more than one [`BMenuBar`](BMenuBar.md "BMenuBar") in the window, the last one added to the window's view hierarchy is considered to be the key one.

If there's a "true" menu bar displayed along the top of the window, its menu hierarchy is the one that users should be able to navigate with the keyboard. `SetKeyMenuBar()` can be called to make sure that the [`BMenuBar`](BMenuBar.md "BMenuBar") object at the root of that hierarchy is the "key" menu bar.

####  SetLook(), Look()

`status_t SetLook(window_look look);``window_look Look() const;`

`SetLook()` changes the window's look to the specified value.

`Look()` returns the current look of the window.

See the `BWindow` [constructor](BWindow.md#BWindow_Constructor "BWindow\(\)") for a list of window_look constants.

####  SetPulseRate(), PulseRate()

`void SetPulseRate(bigtime_t microseconds);``bigtime_t PulseRate();`

These functions set and return how often [`BView::Pulse()`](BView.md#BView_Pulse "Pulse\(\)") is called for the `BWindow`'s views (how often `B_PULSE` messages are posted to the window). All [`BView`](BView.md "BView")s attached to the same window share the same pulse rate.

By turning on the `B_PULSE_NEEDED` flag, a [`BView`](BView.md "BView") can request periodic [`BView::Pulse()`](BView.md#BView_Pulse "Pulse\(\)") notifications. By default, `B_PULSE` messages are posted every 500,000 microseconds, as long as no other messages are pending. Each message causes [`BView::Pulse()`](BView.md#BView_Pulse "Pulse\(\)") to be called once for every [`BView`](BView.md "BView") that requested the notification. There are no pulses if no [`BView`](BView.md "BView")s request them.

`SetPulseRate()` permits you to set a different interval. The interval set should not be less than 100,000 microseconds; differences less than 50,000 microseconds may not be noticeable. A finer granularity can't be guaranteed.

Setting the pulse rate to 0 disables pulsing for all views in the window.

See also: The `BView` [constructor](BView.md#BView_Constructor "BView\(\)")

####  SetSizeLimits(), GetSizeLimits(), SetZoomLimits()

`void SetSizeLimits(float minWidth,  
float maxWidth,  
float minHeight,  
float maxHeight);``void GetSizeLimits(float* minWidth,  
float* maxWidth,  
float* minHeight,  
float* maxHeight);``void SetZoomLimits(float maxWidth,  
float maxHeight);`

These functions set and report limits on the size of the window. The user won't be able to resize the window beyond the limits set by `SetSizeLimits()`–to make it have a width less than `minWidth` or greater than `maxWidth`, nor a height less than `minHeight` or greater than `maxHeight`. By default, the minimums are sufficiently small and the maximums sufficiently large to accommodate any window within reason.

`SetSizeLimits()` constrains the user, not the programmer. It's legal for an application to set a window size that falls outside the permitted range. The limits are imposed only when the user attempts to resize the window; at that time, the window will jump to a size that's within range.

`GetSizeLimits()` writes the current limits to the variables provided.

`SetZoomLimits()` sets the maximum size that the window will zoom to (when the [`Zoom()`](BWindow.md#BWindow_Zoom "Zoom\(\)") function is called). The maximums set by `SetSizeLimits()` also apply to zooming; the window will zoom to the screen size or to the smaller of the maximums set by these two functions.

Since the sides of a window must line up on screen pixels, the values passed to both `SetSizeLimits()` and `SetZoomLimits()` should be whole numbers.

####  SetTitle(), Title()

`void SetTitle(const char* newTitle);``const char* Title() const;`

These functions set and return the window's title. `SetTitle()` replaces the current title with `newTitle`. It also renames the window thread in the following format:
    
    
    "w>newTitle"
    

where as many characters of the `newTitle` are included in the thread name as will fit.

`Title()` returns a pointer to the current title. The returned string is null-terminated. It belongs to the `BWindow` object, which may alter the string or free the memory where it resides without notice. Applications should ask for the title each time it's needed and make a copy for their own purposes.

A window's title and thread name are originally set by an argument passed to the `BWindow` [constructor](BWindow.md#BWindow_Constructor "BWindow\(\)").

####  SetType(), Type()

`status_t SetType(window_type type);``window_type Type() const;`

`SetType()` changes the type of the window to the specified value. `Type()` returns the type. You normally set the window's type when it's constructed.

The type is set at construction (or by `SetType()`) as one of the following constants (full descriptions can be found in the discussion of the `BWindow` [constructor](BWindow.md#BWindow_Constructor "BWindow\(\)")):

  * `B_UNTYPED_WINDOW`
  * `B_MODAL_WINDOW`
  * `B_BORDERED_WINDOW`
  * `B_TITLED_WINDOW`
  * `B_DOCUMENT_WINDOW`
  * `B_FLOATING_WINDOW`

####  SetWindowAlignment(), GetWindowAlignment()

`status_t SetWindowAlignment(window_alignment mode,  
int32 h,  
int32 hOffset = 0,  
int32 width = 0,  
int32 widthOffset = 0,  
int32 v = 0,  
int32 vOffset = 0,  
int32 height = 0,  
int32 heightOffset = 0);``status_t GetWindowAlignment(window_alignment* mode = NULL,  
int32* h = NULL,  
int32* hOffset = NULL,  
int32* width = NULL,  
int32* widthOffset = NULL,  
int32* v = NULL,  
int32* vOffset = NULL,  
int32* height = NULL,  
int32* heightOffset = NULL) const;`

`SetWindowAlignment()` sets the current alignment of the window content on the screen. `mode` is either `B_PIXEL_ALIGNMENT` or `B_BYTE_ALIGNMENT`.

If `mode` is `B_PIXEL_ALIGNMENT`, `SetWindowAlignment()` aligns the window in pixel coordinates. `h` and `hOffset` together determine the horizontal alignment: `h` gives the horizontal origin step while `hOffset` is the horizontal offset. `hOffset` must be between 1 and `h` (as a convenience, 0 is taken to mean 1). For example, if `h` is 4 and `hOffset` is 1, valid horizontal origins would be …, -7, -3, 1, 5, 9, … Similarly, `width`/`widthOffset`, `v`/`vOffset`, `height`/`heightOffset` give you control over the other window parameters.

If `mode` is `B_BYTE_ALIGNMENT`, then the alignment is given in terms of frame buffer offsets. However, the setting only affects the horizontal origin and width. You can't align the right and bottom edges in `B_BYTE_ALIGNMENT` mode.

`GetWindowAlignment()` returns the current window alignment.

Both methods return `B_NO_ERROR` on success and `B_ERROR` otherwise.

####  SetWorkspaces(), Workspaces()

`void SetWorkspaces(uint32 workspaces);``uint32 Workspaces() const;`

These functions set and return the set of workspaces where the window can be displayed. The workspaces argument passed to `SetWorkspaces()` and the value returned by `Workspaces()` is a bitfield with one bit set for each workspace in which the window can appear. Usually a window appears in just one workspace.

`SetWorkspaces()` can associate a window with workspaces that don't exist yet. The window will appear in those workspaces if and when the user creates them.

You can pass `B_CURRENT_WORKSPACE` as the `workspaces` argument to place the window in the workspace that's currently displayed (the active workspace) and remove it from all others, or `B_ALL_WORKSPACES` to make sure the window shows up in all workspaces, including any new ones that the user might create. `Workspaces()` may return `B_ALL_WORKSPACES`, but will identify the current workspace rather than return `B_CURRENT_WORKSPACE`.

Changing a `BWindow`'s set of workspaces causes it to be notified with a [`WorkspacesChanged()`](BWindow.md#BWindow_WorkspacesChanged "WorkspacesChanged\(\)") function call.

See also: the `BWindow` [constructor](BWindow.md#BWindow_Constructor "BWindow\(\)")

####  Show(), Hide(), IsHidden(), Minimize(), IsMinimized()

`virtual void Show();``virtual void Hide();``bool IsHidden() const;``virtual void Minimize(bool minimize);``bool IsMinimized() const;`

These functions hide and show the window.

`Show()` places the window frontmost on the screen (but behind any applicable floating or modal windows), places it on Deskbar's window list, and makes it the active window. If this is the `BWindow`'s first `Show()`, the object's message loop is started, and the object is unlocked.

`Hide()` removes the window from the screen, removes it from Deskbar's window list, and passes active status to some other window (if this is the active window). If `Hide()` is called more than once, you'll need to call `Show()` an equal number of times for the window to become visible again.

`Minimize()` hides and shows the window (and passes active status), as `minimize` is `true` or `false`. The difference between this function and `Hide()`/`Show()` is that `Minimize()` dims (and undims) the window's entry in Deskbar's window list, but doesn't remove the entry altogether. Also, a single Minimize(false) "undoes" any number of Minimize(true) calls.

`Minimize()` also acts as a hook that's invoked when the user double-clicks the window's title tab or selects the window from DeskBar's window list. If `minimize` is `true`, the window is about to be hidden; if `false`, it's about to be shown. The `Minimize()` function itself does the hiding and showing–if you override `Minimize()` and you want to inherit the `BWindow` behaviour, you must call `BWindow::Minimize()` in your implementation.

`IsHidden()` returns `true` if the window is currently hidden (i.e. through `Hide()`). `IsMinimized()` returns `true` if the window is minimized. Hiding takes precendence over minimization. For example, in both of these sequences…
    
    
    window->Hide();
    window->Minimize(true);
    /* or */
    window->Minimize(true);
    window->Hide();

…the window is hidden but not minimized.

See also: `B_MINIMIZE`

#### UpdateIfNeeded()

`void UpdateIfNeeded();`

Immediately and synchronously invokes [`Draw()`](BView.md#BView_Draw "Draw\(\)") on each child view that needs updating. This function is ignored if its called from any thread other than the `BWindow`'s message loop. You call it as part of the implementation of a user interface hook function ( [`MouseMoved()`](BView.md#BView_MouseMoved "MouseMoved\(\)"), [`KeyDown()`](BView.md#BView_KeyDown "KeyDown\(\)")(), et. al.) to force invalid views to be immediately redrawn without having to wait for the hook function to finish. See [ "Forcing an Update while Responding to an Event"](TheInterfaceKit_Drawing.md#TheInterfaceKit_Drawing_Forcing_An_Update "Forcing an Update") for details and an example.

* * *

### Static Functions

#### Instantiate()

See [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)")

* * *

### Constants and Defined Types

#### window_look

The window_look constants define the appearance of the window–the look of its title tab, border, and the type of "resize control" (in the bottom right corner of the window). The look is set in the constructor or in the [`SetLook()`](BWindow.md#BWindow_SetLook "SetLook\(\), Look\(\)") function. The table below lists and briefly describes the window_look values.

Constant| Description  
---|---  
`B_DOCUMENT_WINDOW_LOOK`| Large title bar, thick border, draggable "resizebox".  
`B_TITLED_WINDOW_LOOK`| Same as the document window, but with a less substantial "resize corner".  
`B_FLOATING_WINDOW_LOOK`| Small title bar, thin border, resize corner.  
`B_MODAL_WINDOW_LOOK`| No title bar, thick border, no resize control (by convention; see the `B_NOT_RESIZABLE` window flag).  
`B_BORDERED_WINDOW_LOOK`| No title bar, line border, no resize control.  
`B_NO_BORDER_WINDOW_LOOK`| A borderless white rectangle. The user can't move or close this window.  
  
#### window_feel

The window_feel constants govern a window's behavior in relation to other windows. The feel is set in the `BWindow` constructor or in [`SetFeel()`](BWindow.md#BWindow_SetFeel "SetFeel\(\), Feel\(\)"). The table below briefly describes the window feels.

  * To set a window's subset, use [`AddToSubset()`](BWindow.md#BWindow_AddToSubset "AddToSubset\(\), RemoveFromSubset\(\)").

  * Modal windows are drawn in front of floating windows.

Constant| Description  
---|---  
`B_NORMAL_WINDOW_FEEL`| Behaves like a normal window (non-modal, non-floating).  
`B_MODAL_SUBSET_WINDOW_FEEL`, `B_MODAL_APP_WINDOW_FEEL`, `B_MODAL_ALL_WINDOW_FEEL`| When displayed, blocks all windows in its subset1, app, or across the entire system. A modal subset/app window is visible only if a window in its subset/app is visible. Modal all windows are always visible in all workspaces  
`B_FLOATING_SUBSET_WINDOW_FEEL`, `B_FLOATING_APP_WINDOW_FEEL`, `B_FLOATING_ALL_WINDOW_FEEL`| Floats above all windows in its subset1, app, or across the entire system. A floating subset/app window is visible only if a window in its subset/app is frontmost. Floating all windows are always visible in all workspaces.  
  
#### window_type

The window_type constants are pre-defined combinations of looks and feels. You set the type through the `BWindow` [constructor](BWindow.md#BWindow_Constructor "BWindow\(\)") or [`SetType()`](BWindow.md#BWindow_SetType "SetType\(\), Type\(\)").

Window Type| Look and Feel  
---|---  
`B_TITLED_WINDOW`| `B_TITLED_WINDOW_LOOK` and `B_NORMAL_WINDOW_FEEL`.  
`B_DOCUMENT_WINDOW`| `B_DOCUMENT_WINDOW_LOOK` and `B_NORMAL_WINDOW_FEEL`.  
`B_MODAL_WINDOW`| `B_MODAL_WINDOW_LOOK` and `B_MODAL_APP_WINDOW_FEEL`.  
`B_FLOATING_WINDOW`| `B_FLOATING_WINDOW_LOOK` and `B_FLOATING_APP_WINDOW_FEEL`.  
`B_BORDERED_WINDOW`| `B_BORDERED_WINDOW_LOOK` and `B_NORMAL_WINDOW_FEEL`.  
`B_UNTYPED_WINDOW`| A window of unknown or undefined type.  
  
#### Window Flags

The window flags (or "user attributes") define miscellaneous aspects of the window's interface, such as whether it can be moved or closed by the user. You combine the flags that you want and pass them to the `BWindow` [constructor](BWindow.md#BWindow_Constructor "BWindow\(\)") or [`SetFlags()`](BWindow.md#BWindow_SetFlags "SetFlags\(\), Flags\(\)").

The default behavior is the inverse of all these flags–i.e. a window is normally movable, closable, resizable, and so on. However, some window looks and feels cause imply some of these flags. For example, a modal feel window can't be minimized (hidden)

Window Flag| Description  
---|---  
`B_NOT_MOVABLE`| Prevents the user from being able to move the window.  
`B_NOT_CLOSABLE`| Prevents the user from closing the window.  
`B_NOT_ZOOMABLE`| Prevents the user from zooming the window.  
`B_NOT_MINIMIZABLE`| Prevents the user from hiding the window by double-clicking the title tab.  
`B_NOT_H_RESIZABLE` `B_NOT_V_RESIZABLE` `B_NOT_RESIZABLE`| Prevents the user from resizing the window horizontally and/or vertically (`B_NOT_RESIZABLE` is the same as the sum of the other two constants).  
`B_OUTLINE_RESIZE`| The window draws only the outline of its new dimensions as it's being resized, and doesn't refresh its contents.  
`B_WILL_ACCEPT_FIRST_CLICK`| Tells a non-active window to process an activating mouse click (the "first" mouse click) as if it were already active. The [`BView`](BView.md "BView") that responds to the mouse-down message must activate the window. By default, the first mouse click in a non-active window activates the window, and then the mouse click event is thrown away.   
`B_AVOID_FRONT`| Prevents the window from becoming the frontmost window.  
`B_AVOID_FOCUS`| Prevents the window from being the target of keyboard events.  
`B_NO_WORKSPACE_ACTIVATION`| When a window is first shown, the workspace normally switches to the one in which the window is displayed. Setting this flag keeps this from happening.  
`B_NOT_ANCHORED_ON_ACTIVATE`| Tells the system to bring the window to the current workspace when the window is selected from Deskbar's window list. Normally, selecting a window from the list activates the workspace that the window is currently in.  
`B_ASYNCHRONOUS_CONTROLS`| Tells the window to allow controls to run asynchronously. All windows that contain controls should include this flag (it's off by default because of backwards compatibility).  
`B_QUIT_ON_WINDOW_CLOSE`| Currently has no effect.  
  
#### Window Workspaces

You tell a window which workspaces to appear through the constructor, or through the [`SetWorkspaces()`](BWindow.md#BWindow_SetWorkspaces "SetWorkspaces\(\), Workspaces\(\)") function. In practice, the only two realistic settings are represented by these constants.

Workspace Constant| Description  
---|---  
`B_CURRENT_WORKSPACE`| Appear in the current workspace.  
`B_ALL_WORKSPACES`| Appear in all workspaces.  
  
* * *

### Scripting Support

#### Suite: "suite/vnd.Be-window"

##### B_WINDOW_MOVE_BY, B_WINDOW_MOVE_TO

Message| Description  
---|---  
`B_WINDOW_MOVE_BY`| Moves the window by the distance specified by the [`B_POINT_TYPE`](TheSupportKit_Constants.md#B_POINT_TYPE) field `data`.  
`B_WINDOW_MOVE_TO`| Moves the window to the position specified by the [`B_POINT_TYPE`](TheSupportKit_Constants.md#B_POINT_TYPE) field `data`.  
  
##### Feel

The `Feel` property represents the workspaces in which the window resides. The messages are equivalent to manipulating the window with the [`Feel()`](BWindow.md#BWindow_Feel) and [`SetFeel()`](BWindow.md#BWindow_SetFeel "SetFeel\(\), Feel\(\)") methods.

Message| Specifiers| Description  
---|---|---  
`B_GET_PROPERTY`| `B_DIRECT_SPECIFIER`| Returns the current feel of the window.  
`B_SET_PROPERTY`| `B_DIRECT_SPECIFIER`| Sets the feel of the window.  
  
##### Flags

The `Flags` property represents the window flags. The messages are equivalent to manipulating the window with the [`Flags()`](BWindow.md#BWindow_Flags) and [`SetFlags()`](BWindow.md#BWindow_SetFlags "SetFlags\(\), Flags\(\)") methods.

Message| Specifiers| Description  
---|---|---  
`B_GET_PROPERTY`| `B_DIRECT_SPECIFIER`| Returns the current flags of the window.  
`B_SET_PROPERTY`| `B_DIRECT_SPECIFIER`| Sets the window flags.  
  
##### Frame

The `Frame` property represents the frame rectangle of the window. The frame is passed as a [`BRect`](BRect.md "BRect") (`B_RECT_TYPE`).

Message| Specifiers| Description  
---|---|---  
`B_GET_PROPERTY`| `B_DIRECT_SPECIFIER`| Returns the window's frame rectangle.  
`B_SET_PROPERTY`| `B_DIRECT_SPECIFIER`| Sets the window's frame rectangle.  
  
##### Hidden

The "Hidden" property determines the visibility of the window. The messages are equivalent to manipulating the window with the [`IsHidden()`](BWindow.md#BWindow_IsHidden), [`Hide()`](BWindow.md#BWindow_Hide) and [`Show()`](BWindow.md#BWindow_Show "Show\(\), Hide\(\), IsHidden\(\), Minimize\(\), IsMinimized\(\)") methods with one caveat: nested [`Hide()`](BWindow.md#BWindow_Hide) and [`Show()`](BWindow.md#BWindow_Show "Show\(\), Hide\(\), IsHidden\(\), Minimize\(\), IsMinimized\(\)") calls are disabled so that multiple scripted "hides" are undone with a single scripted "show".

Message| Specifiers| Description  
---|---|---  
`B_GET_PROPERTY`| `B_DIRECT_SPECIFIER`| Returns `true` if the window is hidden; `false` otherwise.  
`B_SET_PROPERTY`| `B_DIRECT_SPECIFIER`| Hides or shows the window.  
  
##### Look

The "Look" property represents the constant that indicates how the window appears. The messages are equivalent to manipulating the window with the [`Look()`](BWindow.md#BWindow_Look) and [`SetLook()`](BWindow.md#BWindow_SetLook "SetLook\(\), Look\(\)") methods.

Message| Specifiers| Description  
---|---|---  
`B_GET_PROPERTY`| `B_DIRECT_SPECIFIER`| Returns the current look of the window.  
`B_SET_PROPERTY`| `B_DIRECT_SPECIFIER`| Sets the look of the window.  
  
##### MenuBar

The "MenuBar" property pops the current specifier off the specifier stack and then passes the scripting message to the key menu bar. If no such menu b/r is present, then an error is returned.

Message| Specifiers| Description  
---|---|---  
any| `B_DIRECT_SPECIFIER`| Directs the scripting message to the key menu bar.  
  
##### Minimize

The "Minimize" property controls the whether the window is minimized or not. The message is equivalent to manipulating the window with the [`Minimize()`](BWindow.md#BWindow_Minimize) method.

Message| Specifiers| Description  
---|---|---  
`B_SET_PROPERTY`| `B_DIRECT_SPECIFIER`| Minimizes the window if "data" is `true`; restores otherwise.  
  
##### Title

The "Title" property represents the title of the window. The messages are equivalent to manipulating the window with the [`Title()`](BWindow.md#BWindow_Title) and [`SetTitle()`](BWindow.md#BWindow_SetTitle "SetTitle\(\), Title\(\)") methods.

Message| Specifiers| Description  
---|---|---  
`B_DIRECT_SPECIFIER`| Returns a string containing the window title.|   
`B_SET_PROPERTY`| `B_DIRECT_SPECIFIER`| Sets the window title.  
  
##### View

The "View" property redirects all requests to the window's top view without popping the specifier from the stack.

Message| Specifiers| Description  
---|---|---  
any| any| Directs the scripting message to the top view without popping the current specifier.  
  
##### Workspaces

The "Workspaces" property represents the workspaces in which the window resides. The messages are equivalent to manipulating the window with the [`Workspaces()`](BWindow.md#BWindow_Workspaces) and [`SetWorkspaces()`](BWindow.md#BWindow_SetWorkspaces "SetWorkspaces\(\), Workspaces\(\)") methods.

Message| Specifiers| Description  
---|---|---  
`B_GET_PROPERTY`| `B_DIRECT_SPECIFIER`| Returns int32 bitfield of the workspaces in which the window appears.  
`B_SET_PROPERTY`| `B_DIRECT_SPECIFIER`| Sets the workspaces in which the window appears.  
  
* * *

### Archived Fields

Field| Type Code| Description  
---|---|---  
`_frame`| `B_RECT_TYPE`| The frame rectangle.  
`_title`| `B_STRING_TYPE`| The title of the `BWindow`.  
`_wlook`| `B_INT32_TYPE`| The `BWindow` look.  
`_wfeel`| `B_INT32_TYPE`| The `BWindow` feel.  
`_type`| `B_INT32_TYPE`| The `BWindow` type (if one exists).  
`_flags`| `B_INT32_TYPE`| The `BWindow` flags.  
`_wspace`| `B_INT32_TYPE`| The workspaces in which the `BWindow` appears.  
`_zoom` (array)| `B_FLOAT_TYPE`| The horizontal and vertical zoom limits, if any exist.  
`_sizel` (array)| `B_FLOAT_TYPE`| The minimum horizontal and vertical and maximum horizontal and vertical size limits, if any exist.  
`_pulse`| `B_INT64_TYPE`| The pulse rate, if not the default.  
`_views` (array)| `B_MESSAGE_TYPE`| Child [`BView`](BView.md "BView")s. See [Archived Fields](BView.md#BView_ArchivedFields "Archived Fields") in the BView class for the [`BMessage`](BMessage.md "BMessage") format. (Deep archive only.)

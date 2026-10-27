<!-- Classes And Methods - The Interface Kit - BAlert -->

## BAlert

| Derived From:| [BWindow](BWindow.md "BWindow") > [BLooper](BLooper.md "BLooper") > [BHandler](BHandler.md "BHandler")  
---|---  
Mix-in Classes:| [BArchivable](BArchivable.md "BArchivable")  
Declared In:| `interface/Alert.h`  
Library:| `libbe.so`  
Allocation:| Constructor only  
[Class Overview](BAlert_Overview.md "BAlert")

  * [Constructor and Destructor](BAlert.md#BAlert_ConstructorDestructor)
  * [Hook Functions](BAlert.md#BAlert_HookFunctions)
  * [Member Functions](BAlert.md#BAlert_MemberFunctions)
  * [Constants and Defined Types](BAlert.md#BAlert_ConstantsDefinedTypes)
  * [Archived Fields](BAlert.md#BAlert_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BAlert()

`BAlert(const char* title,  
const char* text,  
const char* button0Label,  
const char* button1Label = NULL,  
const char* button2Label = NULL,  
button_width widthStyle = B_WIDTH_AS_USUAL,  
alert_type type = B_INFO_ALERT);` `BAlert(const char* title,  
const char* text,  
const char* button0Label,  
const char* button1Label,  
const char* button2Label,  
button_width widthStyle,  
button_spacing spacing,  
alert_type type = B_INFO_ALERT);` `BAlert([`BMessage`](BMessage.md "BMessage")* archive);`

Creates an alert panel that's ready to be displayed. The arguments are:

`title`| Is the name of the panel. Since an alert panel doesn't have a title tab, the title isn't actually displayed in the panel. Nonetheless, it can be useful in debugging: The `title` is used to name the window thread ("w>`title`").  
---|---  
`text`| Is displayed in the body of the panel; it should describe the reason the panel is being shown ("Are you sure you want to quit?", "Invalid name.", etc.).  
`button0Label`, `button1Label`, `button2Label`| Label the panel's buttons. An alert panel can have one, two, or three buttons—however many labels you supply is the number of buttons that are displayed. The buttons are arranged at the bottom of the panel as shown in the illustration above, and each is assigned an index (0, 1, or 2, left to right) that's used to identify the button that the user clicked.  
`widthStyle`| Is a constant that describes how the buttons should be sized, one of `B_WIDTH_AS_USUAL`, `B_WIDTH_FROM_WIDEST`, or `B_WIDTH_FROM_LABEL`. See [button_width](TheInterfaceKit_DefinedTypes.md#button_width "button_width") for details.  
`spacing`| Is a constant that describes how the buttons are spaced, either `B_EVEN_SPACING`, or `B_OFFSET_SPACING`. The constructor that lacks the spacing argument uses `B_EVEN_SPACING`. See [button_spacing](BAlert.md#button_spacing "button_spacing") for details.  
`type`| Is a constant that determines which of the five alert icons is displayed in the panel: `B_EMPTY_ALERT`, `B_INFO_ALERT`, `B_IDEA_ALERT`, `B_WARNING_ALERT`, or `B_STOP_ALERT`. See [alert_type](BAlert.md#alert_type "alert_type") for pictures.  
  
After the `BAlert` is constructed, [`Go()`](BAlert.md#BAlert_Go "Go\(\)") must be called to display it. The panel is removed and the object is deleted after the user clicks a button.

#### ~BAlert()

`~BAlert();`

A standard destructor

* * *

### Hook Functions

#### FrameResized()

See [`BWindow::FrameResized()`](BWindow.md#BWindow_FrameResized)

#### MessageReceived()

See [`BWindow::MessageReceived()`](BWindow.md#BWindow_MessageReceived "MessageReceived\(\)")

* * *

### Member Functions

#### AlertPosition()

`[`BPoint`](BPoint.md "BPoint") AlertPosition(float width,  
float height) const;`

Computes the "best" frame for an alert of the given `width` and `height`, and returns the top left corner of the computed frame in screen coordinates. This function is called automatically when you construct your `BAlert`; you never have to invoke it yourself.

#### ButtonAt()

`[`BButton`](BButton.md "BButton")* ButtonAt(int32 index) const;`

Returns a pointer to the [`BButton`](BButton.md "BButton") object identified by `index`. Indices begin at 0 and count buttons from left to right. The [`BButton`](BButton.md "BButton") belongs to the `BAlert` object and should not be freed.

#### Go()

`int32 Go();` `status_t Go([`BInvoker`](BInvoker.md "BInvoker")* invoker);`

Displays the alert panel. `Go()` can operate synchronously or asynchronously:

  * The no-argument version is synchronous: The function doesn't return until the user has clicked a button and the panel has been removed from the screen.. The value it returns is the index of the clicked button (0,1, or 2, left-to-right)..

  * The [`BInvoker`](BInvoker.md "BInvoker") argument version is asynchronous: The function returns immediately (with `B_OK`) and the button index is delivered as the int32 "which" field of the [`BMessage`](BMessage.md "BMessage") that's sent to the [`BInvoker`](BInvoker.md "BInvoker")'s target.

If you call `Go()` with a (literal) `NULL` argument…
    
    
    alert->Go(NULL);

…the asynchronous version is used, but the [`BMessage`](BMessage.md "BMessage") isn't sent.

The synchronous version deletes the object before it returns; the asynchronous version deletes it after the message is sent. In either case, you should consider the `BAlert` object to be invalid after you call `Go()`.

If the `BAlert` is sent a [`B_QUIT_REQUESTED`](TheApplicationKit_MessageConstants.md#B_QUIT_REQUESTED "B_QUIT_REQUESTED") message while the panel is still on-screen, the synchronous version of `Go()` returns -1; the asynchronous version suppresses the index-reporting message.

####  SetShortcut(), Shortcut()

`void SetShortcut(int32 index,  
char shortcut);``char Shortcut(int32 index) const;`

These functions set and return the shortcut character that's mapped to the button at `index`. A given button can have only one shortcut except for the rightmost button, which, in addition to the shortcut that you give it here, is always mapped to `B_ENTER.`

If you create a "Cancel" button, you should give it a shortcut of `B_ESCAPE`.

#### TextView()

`[`BTextView`](BTextView.md "BTextView")* TextView() const;`

Returns a pointer to the [`BTextView`](BTextView.md "BTextView") object that contains the textual information that's displayed in the panel. You can fiddle with this object but you mustn't delete it.

* * *

### Constants and Defined Types

####  alert_type

Declared in: `interface/Alert.h`
    
    
    enum alert_type
    {
        B_EMPTY_ALERT,
        B_INFO_ALERT,
        B_IDEA_ALERT,
        B_WARNING_ALERT,
        B_STOP_ALERT
    };

The alert_type constants represent the five alert icons:

`B_EMPTY_ALERT`| `B_INFO_ALERT`| `B_IDEA_ALERT`| `B_WARNING_ALERT`| `B_STOP_ALERT`  
---|---|---|---|---  
(none) |  ![Info Icon](./images/TheInterfaceKit/alert_info.png) |  ![Idea Icon](./images/TheInterfaceKit/alert_idea.png) |  ![Warning Icon](./images/TheInterfaceKit/alert_warning.png) |  ![Stop Icon](./images/TheInterfaceKit/alert_stop.png)  
  
####  button_spacing
    
    
    enum button_spacing
    {
        B_EVEN_SPACING,
        B_OFFSET_SPACING
    };

Used to set the spacing of the alert's buttons:

`B_EVEN_SPACING`| The buttons are evenly spaced. (This is used by the constructor without the spacing argument.)  
---|---  
`B_OFFSET_SPACING`| If the alert has more than one button, the leftmost button is shifted left to separate it from its neighbor(s). You should use this configuration for an alert that has a (leftmost) "Cancel" button, as shown in the illustration at the beginning of this document.  
  
* * *

### Archived Fields

Field| Type code| Description  
---|---|---  
`_text`| `B_STRING_TYPE`| The alert's descriptive text.  
`_atype`| `B_INT32_TYPE`| The alert_type value.  
`_but_width`| `B_INT32_TYPE`| The button_width value.  
`_but_key` (array)| `B_INT32_TYPE`| Keyboard shortcut values (one field per button).  
  
In a deep copy, the following views appear in the "_views" field:

View name| Level| Description  
---|---|---  
`_master_`| 0| Background view.  
`_tv_`| 1| [`BTextView`](BTextView.md "BTextView") for the alert panel's text.  
`_b0_`| 1| [`BButton`](BButton.md "BButton") with index 0.  
`_b1_`| 1| [`BButton`](BButton.md "BButton") with index 1.  
`_b2_`| 1| [`BButton`](BButton.md "BButton") with index 2.

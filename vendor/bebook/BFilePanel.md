<!-- Classes And Methods - The Storage Kit - BFilePanel -->

## BFilePanel

| Derived From:| None  
---|---  
Mix-in Classes:| None  
Declared In:| `storage/FilePanel.h`  
Library:| `libtracker.so`  
Allocation:| –  
[Class Overview](BFilePanel_Overview.md "BFilePanel")

  * [Constructor and Destructor](BFilePanel.md#BFilePanel_ConstructorDestructor)
  * [Hook Functions](BFilePanel.md#BFilePanel_HookFunctions)
  * [Member Functions](BFilePanel.md#BFilePanel_Member_Functions)

  
  
* * *

### Constructor and Destructor

#### BFilePanel()

`BFilePanel(file_panel_mode mode = B_OPEN_PANEL,  
[`BMessenger`](BMessenger.md "BMessenger")* target = NULL,  
entry_ref* panel_directory = NULL,  
uint32 node_flavors = 0,  
bool allow_multiple_selection = true,  
[`BMessage`](BMessage.md "BMessage")* message = NULL,  
[`BRefFilter`](BRefFilter.md "BRefFilter")* filter = NULL,  
bool modal = false,  
bool hide_when_done = true);`

The constructor creates a new `BFilePanel` object and initializes it according to the arguments. The panel isn't displayed until you invoke [`Show()`](BFilePanel.md#BFilePanel_Show "Show\(\), Hide\(\), IsShowing\(\)"). The arguments are thoroughly described in "[Constructing and Fine-tuning the Panel](BFilePanel_Overview.md#BFilePanel_Constructing_Fine_Tuning "Constructing and Fine-tuning the Panel")."

Note

![Note](./images/admonitions/Info_32.png)

You may notice that some of the default arguments shown here don't jibe with the defaults listed in the section "[Constructing and Fine-tuning the Panel](BFilePanel_Overview.md#BFilePanel_Constructing_Fine_Tuning "Constructing and Fine-tuning the Panel")". In particular, the `node_flavors` argument was described as defaulting to `B_FILE_NODE`, but is shown here as 0. The "Constructing…" descriptions are correct: The default values shown here are caught and converted by the `BFilePanel` constructor.

#### ~BFilePanel()

`virtual ~BFilePanel();`

Destroys the `BFilePanel`. The object's target and [`BRefFilter`](BRefFilter.md "BRefFilter") are not touched by this destruction. If the object is currently displaying a file panel, the panel is closed.

* * *

### Hook Functions

#### SelectionChanged()

`virtual void SelectionChanged();`

This hook function is invoked whenever the user changes the set of selected files. Within your implementation of this function, you iterate over [`GetNextSelectedRef()`](BFilePanel.md#BFilePanel_GetNextSelectedRef "GetNextSelectedRef\(\), Rewind\(\)") to retrieve refs to the currently selected files.

#### WasHidden()

`virtual void WasHidden();`

`WasHidden()` is a hook function that's invoked whenever the user's actions causes the file panel to be hidden. If you call [`Hide()`](BFilePanel.md#BFilePanel_Hide) yourself, `WasHidden()` is not invoked.

* * *

### Member Functions

####  GetNextSelectedRef(), Rewind()

`status_t GetNextSelectedRef(entry_ref* ref);``void Rewind();`

`GetNextSelectedRef()` initializes its arguments to point to the "next" ref in the file panel's set of currently selected items. The function returns `B_ENTRY_NOT_FOUND` when it reaches the end of the list. `Rewind()` gets you back to the top of the list.

Although you can call these functions anytime you want, they're intended to be used in implementations of the [`SelectionChanged()`](BFilePanel.md#BFilePanel_SelectionChanged "SelectionChanged\(\)") hook function.

#### PanelMode()

`file_panel_mode PanelMode() const;`

Returns the object's mode, either `B_OPEN_PANEL` or `B_SAVE_PANEL`. The mode is set in the constructor and can't be changed thereafter.

#### Refresh()

`void Refresh();`

`Refresh()` tells the file panel to re-read the contents of the panel directory, which causes the directory's entries to be re-run through the ref filter.

You don't have to call `Refresh()` in order to keep the panel in sync with the directory's contents—the directory and file panel are kept in sync automatically.

#### SendMessage()

`virtual void SendMessage([`BMessenger`](BMessenger.md "BMessenger")* messenger,  
[`BMessage`](BMessage.md "BMessage")* message);`

Sends [`BMessage`](BMessage.md "BMessage") `message` to the [`BHandler`](BHandler.md "BHandler") targeted by `messenger`.

See Also: [`BMessenger::SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)")

####  SetButtonLabel(), SetSaveText()

`void SetButtonLabel(file_panel_button which_button,  
const char* label);``void SetSaveText(const char* label);`

`SetButtonLabel()` lets you set the label that's displayed in the panel's buttons. The button that a specific invocation affects depends on the value of `which_button`:

Constant| Description  
---|---  
`B_DEFAULT_BUTTON`| Is the Open button for an Open Panel and the Save button for a Save Panel.  
`B_CANCEL_BUTTON`| Is the "Cancel" button.  
  
`SetSaveText()` sets the text that's displayed in the Save Panel's text view (the area in which the user types and confirms a file name).

####  SetHideWhenDone(), HidesWhenDone()

`void SetHideWhenDone(bool hide_when_done);``bool HidesWhenDone() const;`

By default, a file panel is hidden when the user confirms or Cancels. You can control this behavior using the `SetHideWhenDone()` function. If you set `hide_when_done` to `false`, the panel remains on the screen; if you specify `true`, the panel hides when the user confirms or Cancels. Clicking the panel's close box always hides the panel.

`HidesWhenDone()` returns the current setting of this option: `true` if the panel hides when the user is done with it or `false` if it remains on the screen.

#### SetMessage()

`void SetMessage([`BMessage`](BMessage.md "BMessage")* message);`

`SetMessage()` allows you to set the format of the file panel's notification messages. The message can also be set through the constructor. See "[The Target and the Messages it Sees](BFilePanel_Overview.md#BFilePanel_The_Target_And_The_Messages "The Target and the Messages it Sees")" for more information.

A copy is made of the [`BMessage`](BMessage.md "BMessage"), so it is your responsibility to delete `msg` when you no longer need it.

####  SetPanelDirectory(), GetPanelDirectory()

`void SetPanelDirectory([`BEntry`](BEntry.md "BEntry")* dirEntry);` `void SetPanelDirectory([`BDirectory`](BDirectory.md "BDirectory")* dirObj);` `void SetPanelDirectory(entry_ref* dirRef);` `void SetPanelDirectory(const char* dirStr);`

`void GetPanelDirectory(entry_ref* ref) const;`

The `SetPanelDirectory()` function sets the panel's "panel directory." This is the directory whose contents are displayed in the panel's file list. You can also set the panel directory through the constructor. If you don't supply a directory, the current working directory is used.

`GetPanelDirectory()` initializes `ref` to point to the current panel directory. The argument must be allocated.

####  SetRefFilter(), RefFilter()

`void SetRefFilter([`BRefFilter`](BRefFilter.md "BRefFilter")* filter);``[`BRefFilter`](BRefFilter.md "BRefFilter")* RefFilter() const;`

Whenever the file panel's panel directory is changed or refreshed ([`Refresh()`](BFilePanel.md#BFilePanel_Refresh "Refresh\(\)")), or when a new entry is added to the current panel directory, the "new" entries are run through the panel's "ref filter." The [`BRefFilter`](BRefFilter.md "BRefFilter") class defines a single boolean hook function called [`Filter()`](BRefFilter.md#BRefFilter_Filter "Filter\(\)"). The function receives the entries, one-by-one, and can reject specific entries (because they're the wrong file type, for example). Rejected entries are not shown in the panel's file list.

The `SetRefFilter()` function sets the panel's ref filter. You can also set it through the constructor. Ownership of the filter is _not_ handed to the panel. You mustn't delete the ref filter while the panel is still extant.

`RefFilter()` returns a pointer to the panel's ref filter.

####  SetTarget(), Messenger()

`void SetTarget([`BMessenger`](BMessenger.md "BMessenger") bellhop);``[`BMessenger`](BMessenger.md "BMessenger") Messenger() const;`

`SetTarget()` sets the target of the file panel's notification messages. The target can also be set through the constructor. If you don't set a target, `be_app_messenger` is used. See the [`BInvoker`](BInvoker.md "BInvoker") class (in the Application Kit) for an explanation of how a [`BMessenger`](BMessenger.md "BMessenger") can be used as a target.

A copy is made of the [`BMessenger`](BMessenger.md "BMessenger"), so it is your responsibility to delete `bellhop` when you no longer need it.

`Messenger()` returns (a copy of) the messenger that's used as the file panel's target.

####  Show(), Hide(), IsShowing()

`void Show();``void Hide();``bool IsShowing() const;`

These functions show and hide the file panel, and tell if you the panel is currently showing.

#### Window()

`BWindow* Window() const;`

Returns a pointer to the file panel's window. If you want to mess around with the window's views, see "[Modifying the Look of the File Panel](BFilePanel_Overview.md#BFilePanel_Modifying_The_Look "Modifying the Look of the File Panel")."

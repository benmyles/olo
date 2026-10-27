<!-- Classes And Methods - The Interface Kit - BPopUpMenu -->

## BPopUpMenu

| Derived From:| [`BMenu`](BMenu.md "BMenu")  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/PopUpMenu.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BPopUpMenu_Overview.md "BPopUpMenu")

  * [Constructor and Destructor](BPopUpMenu.md#BPopUpMenu_ConstructorDestructor)
  * [Member Functions](BPopUpMenu.md#BPopUpMenu_MemberFunctions)
  * [Static Functions](BPopUpMenu.md#BPopUpMenu_StaticFunctions)

  
  
* * *

### Constructor and Destructor

#### BPopUpMenu()

`BPopUpMenu(const char* name,  
bool radioMode = true,  
bool labelFromMarked = true,  
menu_layout layout = B_ITEMS_IN_COLUMN);` `BPopUpMenu([`BMessage`](BMessage.md "BMessage")* archive);`

Initializes the `BPopUpMenu` object. If the object is added to a [`BMenuBar`](BMenuBar.md "BMenuBar"), its `name` also becomes the initial label of its controlling item (just as for other [`BMenu`](BMenu.md "BMenu")s).

If the `labelFromMarked` flag is `true` (as it is by default), the label of the controlling item will change to reflect the label of the item that the user last selected. In addition, the menu will operate in radio mode (regardless of the value passed as the `radioMode` flag). When the menu pops up, it will position itself so that the marked item appears directly over the controlling item in the [`BMenuBar`](BMenuBar.md "BMenuBar").

If `labelFromMarked` is `false`, the menu pops up so that its first item is over the controlling item.

If the `radioMode` flag is `true` (as it is by default), the last item selected by the user will always be marked. In this mode, one and only one item within the menu can be marked at a time. If `radioMode` is `false`, items aren't automatically marked or unmarked.

However, the `radioMode` flag has no effect unless the `labelFromMarked` flag is `false`. As long as `labelFromMarked` is `true`, radio mode will also be `true`.

The layout of the items in a `BPopUpMenu` can be either `B_ITEMS_IN_ROW` or the default `B_ITEMS_IN_COLUMN`. It should never be `B_ITEMS_IN_MATRIX`. The menu is resized so that it exactly fits the items that are added to it.

The new `BPopUpMenu` is empty; you add items to it by calling [`BMenu`](BMenu.md "BMenu")'s [`AddItem()`](BMenu.md#BMenu_AddItem "AddItem\(\)") function.

See also: [`BMenu::SetRadioMode()`](BMenu.md#BMenu_SetRadioMode "SetRadioMode\(\), IsRadioMode\(\)"), [`BMenu::SetLabelFromMarked()`](BMenu.md#BMenu_SetLabelFromMarked "SetLabelFromMarked\(\) , IsLabelFromMarked\(\)")

#### ~BPopUpMenu()

`virtual ~BPopUpMenu();`

Does nothing. The [`BMenu`](BMenu.md "BMenu") destructor is sufficient to clean up after a `BPopUpMenu`.

* * *

### Member Functions

#### Archive()

`virtual status_t Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Calls the inherited version of [`Archive()`](BMenu.md#BMenu_Archive "Archive\(\)") and stores the `BPopUpMenu` in the [`BMessage`](BMessage.md "BMessage") archive.

See also: [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)"), [`Instantiate()`](BPopUpMenu.md#BPopUpMenu_Instantiate "Instantiate\(\)") static function

#### Go()

`[`BMenuItem`](BMenuItem.md "BMenuItem")* Go([`BPoint`](BPoint.md "BPoint") screenPoint,  
bool deliversMessage = false,  
bool openAnyway = false,  
bool asynchronous = false);` `[`BMenuItem`](BMenuItem.md "BMenuItem")* Go([`BPoint`](BPoint.md "BPoint") screenPoint,  
bool deliversMessage,  
bool openAnyway,  
[`BRect`](BRect.md "BRect") clickToOpenRect,  
bool asynchronous = false);`

Places the pop-up menu on-screen so that its left top corner is located at `screenPoint` in the screen coordinate system. If the `asynchronous` flag is `true`, `Go()` returns right away; the return value is `NULL`. Otherwise, it doesn't return until the user dismisses the menu from the screen. If the user invoked an item in the menu, it returns a pointer to the item. If no item was invoked, it returns `NULL`.

`Go()` is typically called from within the [`MouseDown()`](BView.md#BView_MouseDown "MouseDown\(\)") function of a [`BView`](BView.md "BView"). For example:
    
    
    void MyView::MouseDown([BPoint](BPoint.md "BPoint") point)
    {
       [BMenuItem](BMenuItem.md "BMenuItem")* selected;
       [BMessage](BMessage.md "BMessage")* copy;
       . . .
       ConvertToScreen(&point);
       selected = myPopUp->Go(point);
       . . .
       if ( selected ) {
          BLooper *looper;
          BHandler *target = selected->Target(&looper);
          looper->PostMessage(selected->Message(), target);
       }
       . . .
    }

`Go()` operates in two modes:

  * If the `deliversMessage` flag is `true`, the `BPopUpMenu` works just like a menu that's controlled by a [`BMenuBar`](BMenuBar.md "BMenuBar"). When the user invokes an item in the menu, the item posts a message to its target.

  * If the `deliversMessage` flag is `false`, a message is not posted. Invoking an item doesn't automatically accomplish anything. It's up to the application to look at the returned [`BMenuItem`](BMenuItem.md "BMenuItem") and decide what to do. It can mimic the behavior of other menus and post the message—as shown in the example above—or it can take some other course of action.

`Go()` always puts the pop-up menu on-screen, but ordinarily keeps it there only as long as the user holds a mouse button down. When the user releases the button, the menu is hidden and `Go()` returns. However, the `openAnyway` flag and the `clickToOpenRect` arguments can alter this behavior so that the menu will stay open even when the user releases the mouse button (or even if a mouse button was never down). It will take another user action—such as invoking an item in the menu or clicking elsewhere—to dismiss the menu.

If the `openAnyway` flag is `true`, `Go()` keeps the menu on-screen even if no mouse buttons are held down. This permits a user to open and operate a pop-up menu from the keyboard. If `openAnyway` is `false`, mouse actions determine whether the menu stays on-screen.

If the user has the click-to-open menu preference turned on and releases the mouse button while the cursor lies inside the `clickToOpenRect` rectangle, `Go()` interprets the action as clicking to open the menu and keeps it on-screen. If the cursor is outside the rectangle when the mouse button goes up, the menu is removed from the screen and `Go()` returns. The rectangle should be stated in the screen coordinate system.

Once `Go()` returns, your application should delete the `BPopUpMenu` object.

See also: [`BInvoker::SetMessage()`](BInvoker.md#BInvoker_SetMessage "SetMessage\(\), Message\(\), Command\(\)")

#### ScreenLocation()

##### protected

`virtual [`BPoint`](BPoint.md "BPoint") ScreenLocation();`

Determines where the pop-up menu should appear on-screen (when it's being run automatically, not by `Go()`). As explained in the description of the class constructor, this largely depends on whether the label of the superitem changes to reflect the item that's currently marked in the menu. The point returned is stated in the screen coordinate system.

This function is called only for `BPopUpMenu`s that have been added to a menu hierarchy (a [`BMenuBar`](BMenuBar.md "BMenuBar")). You should not call it to determine the point to pass to `Go()`. However, you can override it to change where a customized pop-up menu defined in a derived class appears on-screen when it's controlled by a [`BMenuBar`](BMenuBar.md "BMenuBar").

See also: [`BMenu::SetLabelFromMarked()`](BMenu.md#BMenu_SetLabelFromMarked "SetLabelFromMarked\(\) , IsLabelFromMarked\(\)"), [`BMenu::ScreenLocation()`](BMenu.md#BMenu_ScreenLocation "ScreenLocation\(\)"), the `BPopUpMenu` constructor

####  SetAsyncAutoDestruct(), AsyncAutoDestruct()

`void SetAsyncAutoDestruct(bool state);``bool AsyncAutoDestruct() const;`

`SetAsyncAutoDestruct()` lets you specify whether you want `BPopUpMenu` to automatically delete its [`BMenu`](BMenu.md "BMenu") when the pop-up menu closes. If you want the [`BMenu`](BMenu.md "BMenu") to be deleted, specify `true` for `state`.

`AsyncAutoDestruct()` returns the current state of the flag.

* * *

### Static Functions

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Returns a new `BPopUpMenu` object, allocated by new and created with the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive. However, if the archive message doesn't contain data for a `BPopUpMenu`, this function returns `NULL`.

See also: [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"), [`Archive()`](BMenu.md#BMenu_Archive "Archive\(\)")

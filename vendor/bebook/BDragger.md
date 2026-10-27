<!-- Classes And Methods - The Interface Kit - BDragger -->

## BDragger

| Derived From:| [`BView`](BView.md "BView")  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/Dragger.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BDragger_Overview.md "BDragger")

  * [Constructor and Destructor](BDragger.md#BDragger_ConstructorDestructor)
  * [Hook Functions](BDragger.md#BDragger_HookFunctions)
  * [Member Functions](BDragger.md#BDragger_MemberFunctions)
  * [Static Functions](BDragger.md#BDragger_StaticFunctions)
  * [Archived Fields](BDragger.md#BDragger_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BDragger()

`BDragger([`BRect`](BRect.md "BRect") frame,  
[`BView`](BView.md "BView")* target,  
uint32 resizingMode = B_FOLLOW_NONE,  
uint32 flags = B_WILL_DRAW);` `BDragger([`BMessage`](BMessage.md "BMessage")* archive);`

Creates a new `BDragger` and sets its target view. The `BDragger` and the target [`BView`](BView.md "BView") must be directly related in the view hierarchy (as parent-child or as siblings); but, note well, the constructor doesn't establish this relationship for you. After you construct you `BDragger`, you have to do one of three things:

  1. add the target as a child of the dragger,

  2. add the dragger as a child of the target,

  3. or add the dragger as a sibling of the target.

If you add the target as a child of `BDragger`, it should be the only child that the `BDragger` has.

A `BDragger` draws in the right bottom corner of its frame rectangle. If the `target` view is a parent or a sibling of the `BDragger`, that rectangle needs to be no larger than the image the `BDragger` draws (the handle). However, if the target is the `BDragger`'s child, the dragger's frame rectangle must enclose the target's frame (so that the dragger doesn't clip the target).

A `BDragger` is fully functional once it has been constructed and attached to the view hierarchy of its target. You don't need to call any other functions. However, the whole endeavor fails if the target [`BView`](BView.md "BView") can't be archived.

#### ~BDragger()

`virtual ~BDragger();`

Frees all memory the `BDragger` allocated (principally for the bitmap image it draws).

* * *

### Hook Functions

####  AttachedToWindow(), DetachedFromWindow()

`virtual void AttachedToWindow();``virtual void DetachedFromWindow();`

`AttachedToWindow()` makes sure that the `BDragger` is under the control of the [`HideAllDraggers()`](BDragger.md#BDragger_HideAllDraggers "HideAllDraggers\(\), ShowAllDraggers\(\), AreDraggersDrawn\(\)") and [`ShowAllDraggers()`](BDragger.md#BDragger_ShowAllDraggers) functions, makes its low and background view colors match the view color of its parent, and determines the `BDragger`'s precise relationship to its target view. To make this determination, the target must be in the view hierarchy; it can't be added to the window after the `BDragger` is. For example, if the target is the `BDragger`'s child, it should be added to the `BDragger` and then the `BDragger` added to the window.

`DetachedFromWindow()` removes the `BDragger` from the control of the [`HideAllDraggers()`](BDragger.md#BDragger_HideAllDraggers "HideAllDraggers\(\), ShowAllDraggers\(\), AreDraggersDrawn\(\)") and [`ShowAllDraggers()`](BDragger.md#BDragger_ShowAllDraggers) functions.

#### Draw()

`virtual void Draw([`BRect`](BRect.md "BRect") updateRect);`

Draws the handle—or fails to draw it and has the parent view draw in that area instead, if all `BDragger`s are hidden.

#### MessageReceived()

`virtual void MessageReceived([`BMessage`](BMessage.md "BMessage")* message);`

Responds to messages that regulate the visibility of the `BDragger` handle.

#### MouseDown()

`virtual void MouseDown([`BPoint`](BPoint.md "BPoint") where);`

Responds to a `B_MOUSE_DOWN` message by archiving the target view (and the `BDragger`) and initiating a drag-and-drop operation, or by taking other appropriate action.

* * *

### Member Functions

#### Archive()

`virtual status_t Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Records the `BDragger`'s hierarchical relationship to the target view and then calls [`BView::Archive()`](BView.md#BView_Archive "Archive\(\)") . The `deep` flag has no significance for `BDragger` itself, but note that the flag is passed on to the [`BView`](BView.md "BView") version.

#### IsVisibilityChanging()

##### protected

`bool IsVisibilityChanging() const;`

Returns `true` if two things are true:

  1. The `BDragger` is the parent of its target.

  2. The `BDragger` handle was visible but now should not be, or it wasn't visible and now should be.

Otherwise, this function returns `false`.

What's this function for? It's in the API so derived classes can implement their own versions of [`Draw()`](BDragger.md#BDragger_Draw "Draw\(\)"). If the `BDragger` isn't the parent of its target, the visibility of the `BDragger` view can be controlled by the [`Hide()`](BView.md#BView_Hide "Hide\(\), Show\(\)") and [`Show()`](BView.md#BView_Show) functions rather than [`Draw()`](BDragger.md#BDragger_Draw "Draw\(\)").

####  PopUp(), SetPopUp()

`[`BPopUpMenu`](BPopUpMenu.md "BPopUpMenu")* PopUp() const;``status_t SetPopUp([`BPopUpMenu`](BPopUpMenu.md "BPopUpMenu")* context_menu);`

Returns and sets the [`BPopUpMenu`](BPopUpMenu.md "BPopUpMenu") displayed when the user right clicks on the `BDragger` view after it has been attached to a [`BShelf`](BShelf.md "BShelf").

* * *

### Static Functions

####  HideAllDraggers(), ShowAllDraggers(), AreDraggersDrawn()

`static status_t HideAllDraggers();``static status_t ShowAllDraggers();``static bool AreDraggersDrawn();`

These functions communicate with all `BDragger` objects in all applications (provided they're attached to windows). `HideAllDraggers()` hides the `BDragger` objects so that they're not visible on-screen. `ShowAllDraggers()` undoes the effect of `HideAllDraggers()` and causes all `BDragger` objects to draw their handles. The Show Replicants / Hide Replicants menu item does its work through these functions.

`HideAllDraggers()` may or may not hide the `BDragger` view in the way that [`BView`'s](BView.md "BView") [`Hide()`](BView.md#BView_Hide "Hide\(\), Show\(\)") function does. The `BDragger` may still be visible, although it won't draw anything until `ShowAllDraggers()` is called. Therefore, if the target [`BView`](BView.md "BView") is the `BDragger`'s child, it will not be hidden when `HideAllDraggers()` erases its parent.

`AreDraggersDrawn()` returns `true` when the `BDragger`s are shown and `false` when they're hidden.

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Returns a new `BDragger` object, allocated by new and created with the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive. If the archive message doesn't contain an archived `BDragger`, `Instantiate()` returns `NULL`.

* * *

### Archived Fields

The `Archive()` function adds the following fields to its [`BMessage`](BMessage.md "BMessage") argument:

Field| Type code| Description  
---|---|---  
`_popup`| `B_MESSAGE_TYPE`| Archive of the pop-up menu. Field present only if this menu was set with [`SetPopUp()`](BDragger.md#BDragger_SetPopUp).  
`_rel`| `B_INT32_TYPE`| Relationship to the target view; one of: `TARGET_IS_PARENT`, `TARGET_IS_CHILD`, or `TARGET_IS_SIBLING`.

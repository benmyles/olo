<!-- System Overview - The Interface Kit - BPopUpMenu -->

## BPopUpMenu

A [`BPopUpMenu`](BPopUpMenu.md "BPopUpMenu") is a menu that's typically used in isolation, rather than as part of an extensive menu hierarchy. By default, it operates in radio mode—the last item selected by the user, and only that item, is marked in the menu.

A menu of this kind can be used to choose one from a set of mutually exclusive states—to pick a paper size or paragraph style, for example. It shouldn't be used to group different kinds of choices (as other menus may), nor should it include items that initiate actions rather than set states, except in certain well-defined cases.

A pop-up menu can be used in any of three ways:

  1. It can be controlled by a [`BMenuBar`](BMenuBar.md "BMenuBar") object, often one that contains just a single item. The [`BMenuBar`](BMenuBar.md "BMenuBar"), in effect, functions as a button that pops up a list. The label of the marked item in the list can be displayed as the label of the controlling item in the [`BMenuBar`](BMenuBar.md "BMenuBar"). In this way, the [`BMenuBar`](BMenuBar.md "BMenuBar") is able to show the current state of the hidden menu. When this is the case, the menu pops up so its marked item is directly over the controlling item.

  2. A [`BPopUpMenu`](BPopUpMenu.md "BPopUpMenu") can also be controlled by a view other than a [`BMenuBar`](BMenuBar.md "BMenuBar"). It might be associated with a particular image the view displays, for example, and appear over the image when the user moves the cursor there and presses the mouse button. Or it might be associated with the view as a whole and come up under the cursor wherever the cursor happens to be. When the view is notified of a mouse-down event, it calls [`BPopUpMenu`](BPopUpMenu.md "BPopUpMenu")'s [`Go()`](BPopUpMenu.md#BPopUpMenu_Go "Go\(\)") function to show the menu on-screen.

  3. The [`BPopUpMenu`](BPopUpMenu.md "BPopUpMenu") might also be controlled by a particular mouse button, typically the secondary mouse button. When the user presses the button, the menu appears at the location of the cursor. Instead of passing responsibility for the mouse-down event to a [`BView`](BView.md "BView"), the [`BWindow`](BWindow.md "BWindow") would intercept it and place the menu on-screen.

Other than [`Go()`](BPopUpMenu.md#BPopUpMenu_Go "Go\(\)") (and the [constructor](BPopUpMenu.md#BPopUpMenu_Constructor "BPopUpMenu\(\)")), this class implements no functions that you ever need to call from application code. In all other respects, a [`BPopUpMenu`](BPopUpMenu.md "BPopUpMenu") can be treated like any other [`BMenu`](BMenu.md "BMenu").

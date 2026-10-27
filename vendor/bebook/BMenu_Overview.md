<!-- System Overview - The Interface Kit - BMenu -->

## BMenu

A [`BMenu`](BMenu.md "BMenu") object displays a pull-down or pop-up list of menu items. A menu can contain simple menu items ( [`BMenuItem`](BMenuItem.md "BMenuItem") objects), or other menus (other [`BMenu`](BMenu.md "BMenu")s). To add an item to a menu, call [`AddItem()`](BMenu.md#BMenu_AddItem "AddItem\(\)").

* * *

### Menu Hierarchy

Menus are hierarchically arranged; an item in one menu can control another menu. The controlled menu is a submenu; the menu that contains the item that controls it is its supermenu. A submenu remains hidden until the user operates the item that controls it; it becomes hidden again when the user is finished with it.

The menu at the root of the hierarchy is displayed in a window as a list - perhaps a list of just one item. Since it, unlike other menus, doesn't have a controlling item, it must remain visible. A root menu is therefore a special kind of menu in that it behaves more like an ordinary view than do other menus, which stay hidden. Root menus should belong to the [`BMenuBar`](BMenuBar.md "BMenuBar") class, which is derived from [`BMenu`](BMenu.md "BMenu"). The typical root menu is a menu bar displayed across the top of a window.

* * *

### Menu Items

Each item in a menu is a kind of [`BMenuItem`](BMenuItem.md "BMenuItem") object. An item can be marked (displayed with a check mark to its left), assigned a keyboard shortcut, enabled and disabled, and given a "trigger" character that the user can type to invoke the item when its menu is open on-screen.

Every item has a particular job to do. If an item controls a submenu, its job is to show the submenu on-screen and hide it again. All other items give instructions to the application. When invoked by the user, they deliver a [`BMessage`](BMessage.md "BMessage") to a target [`BHandler`](BHandler.md "BHandler"). What the item does depends on the content of the [`BMessage`](BMessage.md "BMessage") and the [`BHandler`](BHandler.md "BHandler")'s response to it.

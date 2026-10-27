<!-- System Overview - The Interface Kit - BMenuBar -->

## BMenuBar  
  
A [`BMenuBar`](BMenuBar.md "BMenuBar") is a menu that can stand at the root of a menu hierarchy. Rather than appear on-screen when commanded to do so by a user action, a [`BMenuBar`](BMenuBar.md "BMenuBar") object has a settled location in a window's view hierarchy, just like other views. Typically, the root menu is the menu bar that's drawn across the top of the window. It's from this use that the class gets its name.

However, instances of this class can also be used in other ways. A [`BMenuBar`](BMenuBar.md "BMenuBar") might simply display a list of items arranged in a column somewhere in a window. Or it might contain just one item, where that item controls a pop-up menu (a [`BPopUpMenu`](BPopUpMenu.md "BPopUpMenu") object). Rather than look like a "menu bar," the [`BMenuBar`](BMenuBar.md "BMenuBar") object would look something like a button.

* * *

### The Key Menu Bar

The "real" menu bar at the top of the window usually represents an extensive menu hierarchy; each of its items typically controls a submenu.

The user should be able to operate this menu bar from the keyboard (using the arrow keys and Enter). There are two ways that the user can put the [`BMenuBar`](BMenuBar.md "BMenuBar") and its hierarchy in focus for keyboard events:

  * Clicking an item in the menu bar. If the "click to open" preference is not turned off, this opens the submenu the item controls so that it stays visible on-screen and puts the submenu in focus.

  * Pressing the Menu key or Command+Escape. This puts the [`BMenuBar`](BMenuBar.md "BMenuBar") in focus and selects its first item.

Either method opens the entire menu hierarchy to keyboard navigation.

If a window's view hierarchy includes more than one [`BMenuBar`](BMenuBar.md "BMenuBar") object, the Menu key (or Command+Escape) must choose one of them to put in focus. By default, it picks the last one that was attached to the window. However, the [`SetKeyMenuBar()`](BWindow.md#BWindow_SetKeyMenuBar) function defined in the [`BWindow`](BWindow.md "BWindow") class can be called to designate a different [`BMenuBar`](BMenuBar.md "BMenuBar") object as the "key" menu bar for the window.

* * *

### A Kind of BMenu

[`BMenuBar`](BMenuBar.md "BMenuBar") inherits most of its functions from the [`BMenu`](BMenu.md "BMenu") class. It reimplements the [`AttachedToWindow()`](BMenuBar.md#BMenuBar_AttachedToWindow "AttachedToWindow\(\)"), [`Draw()`](BMenuBar.md#BMenuBar_Draw "Draw\(\)"), and [`MouseDown()`](BMenuBar.md#BMenuBar_MouseDown "MouseDown\(\)"), functions that set up the object and respond to messages, but these aren't functions that you'd call from application code; they're called for you.

The only real function (other than the constructor) that the [`BMenuBar`](BMenuBar.md "BMenuBar") class adds to those it inherits is [`SetBorder()`](BMenuBar.md#BMenuBar_SetBorder "SetBorder\(\), Border\(\)"), which determines how the list of items is bordered.

Therefore, for most [`BMenuBar`](BMenuBar.md "BMenuBar") operations—adding submenus, finding items, temporarily disabling the menu bar, and so on—you must call inherited functions and treat the object like the [`BMenu`](BMenu.md "BMenu") that it is.

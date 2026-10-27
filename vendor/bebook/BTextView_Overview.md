<!-- System Overview - The Interface Kit - BTextView -->

## BTextView

A [`BTextView`](BTextView.md "BTextView") object displays text on-screen, and provides these text manipulating features:

  * It lets the user enter, select, and edit text from the keyboard and mouse.

  * It supports standard Cut, Copy, Paste, Delete, and Select All editing commands

  * It provides an Undo mechanism.

By default, a [`BTextView`](BTextView.md "BTextView") displays all its text in a single font and color. The [`SetStylable()`](BTextView.md#BTextView_SetStylable "SetStylable\(\), IsStylable\(\)") turns on support for multiple character formats.

Paragraph formats—such as alignment and tab widths—are uniform for all text the BTextView displays. These properties can be set, but the setting always applies to the entire text.

Warning

![Warning](./images/admonitions/Stop_32.png)

The [`BTextView`](BTextView.md "BTextView") class isn't multi-thread safe; don't issue [`BTextView`](BTextView.md "BTextView") calls on a [`BTextView`](BTextView.md "BTextView") object from multiple threads, or you may see unusual behavior; in general, only the thread that created the [`BTextView`](BTextView.md "BTextView") should issue calls on it.

* * *

### Offsets

The [`BTextView`](BTextView.md "BTextView") locates particular characters in its text buffer by offsets from the beginning of the data. The offsets count bytes, not characters, and begin at 0. A single character is identified by the offset of the first byte of the character. A group of characters—the current selection, for example—is delimited by the offsets that bound its first and last characters; all characters beginning with the first offset up to, but not including, the last offset are part of the group.

For example, suppose the [`BTextView`](BTextView.md "BTextView") contains the following text in Unicode UTF-8 encoding,
    
    
    The BeOS(TM) is . . .
    

and "BeOS(TM)" is selected. [`GetSelection()`](BTextView.md#BTextView_GetSelection "GetSelection\(\)") would return 4 and 11 as the offsets that enclose the selection. The character 'B' occupies the fourth byte of text and the space following the trademark symbol is the eleventh byte of text. The characters in "BeOS" are each encoded in one byte, but '(TM) ' takes up three bytes in UTF-8. Thus the five-character selection occupies 7 bytes (and offsets) of text.

Although offsets count bytes, they can also be thought of as designating positions between characters. The position at the beginning of the text is offset 0, the position between the space and the 'B' in the example above is at offset 4, the position between the '(TM) ' and the space is at offset 11, and so on. Thus, even if no characters are selected, the insertion point (and location of the caret) can still be designated by an offset.

Most [`BTextView`](BTextView.md "BTextView") functions expect the offsets they're passed to mark positions between characters; the results are not defined if a character-internal offset is specified instead.

* * *

### Graphics Primitives

The [`BTextView`](BTextView.md "BTextView")'s mechanism for formatting and drawing text uses the graphics primitives it inherits from the ["`BView`](BView.md "BView") class. However, it largely presents its own API for determining the appearance of the text it draws. You should not attempt to affect the [`BTextView`](BTextView.md "BTextView") by calling primitive [`BView`](BView.md "BView") functions like [`MovePenTo()`](BView.md#BView_MovePenTo), [`SetFont()`](BView.md#BView_SetFont "SetFont\(\), GetFont\(\)"), or [`SetHighColor()`](BView.md#BView_SetHighColor "SetHighColor\(\), HighColor\(\) , SetLowColor\(\) , LowColor\(\)"). Instead, use [`BTextView`](BTextView.md "BTextView") functions like [`SetFontAndColor()`](BTextView.md#BTextView_SetFontAndColor "SetFontAndColor\(\), GetFontAndColor\(\)") and let the object take care of formatting and drawing the text.

The one inherited function that can influence the [`BTextView`](BTextView.md "BTextView") is [`SetViewColor()`](BView.md#BView_SetViewColor "SetViewColor\(\), ViewColor\(\)"). This function determines the background against which the text is drawn and the color that is used in antialiasing calculations.

* * *

### Resizing

A [`BTextView`](BTextView.md "BTextView") can be made to resize itself to exactly fit the text that the user enters. This is sometimes appropriate for small one-line text fields. See the [`MakeResizable()`](BTextView.md#BTextView_MakeResizable "MakeResizable\(\), IsResizable\(\)") function.

* * *

### BTextView and BScrollBars

If you add scrollbars to control the position of the [`BTextView`](BTextView.md "BTextView")'s document—this includes using a [`BTextView`](BTextView.md "BTextView") as the target of a [`BTextView`](BTextView.md "BTextView")BScrollView—the [`BTextView`](BTextView.md "BTextView") will update the scrollbars for you. (Note that it doesn't own the scrollbars; you still have to delete them yourself if you created them.) When the [`BTextView`](BTextView.md "BTextView") is first displayed and thereafter resized, the scrollbars' ranges, step sizes, and scroller positions and proportions are automatically reset to reflect the [`BTextView`](BTextView.md "BTextView") object's bounds. Attempts to set these parameters directly (through [`BScrollBar::SetRange()`](BScrollBar.md#BScrollBar_SetRange "SetRange\(\), GetRange\(\)") etc.), are worse than ignored; they're actually applied, and then (at some point) the [`BTextView`](BTextView.md "BTextView") will notice the change in the scrollbars and reset them. Looks like flicker to me.

* * *

### Shortcuts and Menu Items

When it's the focus view for its window, a [`BTextView`](BTextView.md "BTextView") automatically responds to a set of keyboard shortcuts:

  * Command+x cuts text and copies it to the clipboard

  * Command+c copies text to the clipboard without cutting it

  * Command+v pastes text taken from the clipboard

  * Command+a selects all of the text in the [`BTextView`](BTextView.md "BTextView")

  * Command+z undoes the previous action

If you create menu items for these actions, you have to assign the [`BMenuItem`](BMenuItem.md "BMenuItem") shortcuts and command constants yourself:

Action| Constant| Shortcut  
---|---|---  
Select all| `B_SELECT_ALL`| Command+a  
Cut| `B_CUT`| Command+x  
Copy| `B_COPY`| Command+c  
Paste| `B_PASTE`| Command+v  
Undo| `B_UNDO`| Command+z

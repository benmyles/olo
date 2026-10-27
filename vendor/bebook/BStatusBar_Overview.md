<!-- System Overview - The Interface Kit - BStatusBar -->

## BStatusBar

A [`BStatusBar`](BStatusBar.md "BStatusBar") is a cosmetic object that graphically indicates the progress of an operation. It displays a horizontal bar that's filled with color from left to right as operation is progresses.

The label and text are abutted and aligned at the left of the bar; the trailing text and trailing label are abutted and aligned at the right of the bar.

The texts can change each time the bar is updated. The label and trailing label are set by the constructor and remain constant throughout the display, or until the object is reset for another operation.

To set the amount that the status bar is filled, you can call [`Update()`](BStatusBar.md#BStatusBar_Update "Update\(\)"), which lets you add a delta to the current value. The "filling" itself reflects the ratio (current_value)/(max_value), where max_value is 100.0 by default, but can be reset through [`SetMaxValue()`](BStatusBar.md#BStatusBar_SetMaxValue "SetMaxValue\(\), MaxValue\(\), CurrentValue\(\)"). The minimum value of the bar is always 0.0.

You can call [`Update()`](BStatusBar.md#BStatusBar_Update "Update\(\)") directly, or invoke it indirectly (and asynchronously) by sending a `B_UPDATE_STATUS_BAR` message to the [`BStatusBar`](BStatusBar.md "BStatusBar").

The [`Reset()`](BStatusBar.md#BStatusBar_Reset "Reset\(\)") function, which empties the bar can also be invoked through a message (`B_RESET_STATUS_BAR`).

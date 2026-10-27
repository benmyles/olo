<!-- System Overview - The Interface Kit - BTab -->

## BTab

The [`BTab`](BTab.md "BTab") class defines the tabs used by the [`BTabView`](BTabView.md "BTabView") class. Each tab is represented by a single [`BTab`](BTab.md "BTab") object, which is called to render and manage the tab.

When a tab is created, a target view is specified as a parameter to the [`BTab`](BTab.md "BTab") [constructor](BTab.md#BTab_Constructor "BTab\(\)"), or by calling [`SetView()`](BTab.md#BTab_SetView). The target view is the view that will be displayed in the [`BTabView`](BTabView.md "BTabView")'s container view when the [`BTab`](BTab.md "BTab") is selected.

Users select tabs by clicking on them, or by using keyboard navigation to focus and select the tab.

An example of how to create a [`BTabView`](BTabView.md "BTabView") and attach [`BTab`](BTab.md "BTab") objects to it is given in the [`BTabView`](BTabView.md "BTabView") section.

* * *

### Customizing the Appearance of a BTab

Customizing the appearance of your tabs is achieved by overriding the [`DrawTab()`](BTab.md#BTab_DrawTab), [`DrawFocusMark()`](BTab.md#BTab_DrawFocusMark "DrawFocusMark\(\) , DrawTab\(\) , DrawLabel\(\)"), and/or [`DrawLabel()`](BTab.md#BTab_DrawLabel) functions.

These functions are responsible for all drawing of the [`BTab`](BTab.md "BTab"). [`DrawTab()`](BTab.md#BTab_DrawTab) renders the entire tab, excluding the focus mark: it draws the borders and calls [`DrawLabel()`](BTab.md#BTab_DrawLabel) to render the text of the label.

[`DrawFocusMark()`](BTab.md#BTab_DrawFocusMark "DrawFocusMark\(\) , DrawTab\(\) , DrawLabel\(\)") draws the indicator that shows which tab is the current focus for keyboard navigation.

By default, tabs have a beveled, rounded look. Let's look at an example in which we replace this appearance with a square shape:

To do this, we create a new class, derived from [`BTab`](BTab.md "BTab"), that overrides the [`DrawTab()`](BTab.md#BTab_DrawTab) function.
    
    
    class CustomTab : public BTab {
       public:
          virtual void      DrawTab(BView *owner, BRect frame,
                         tab_position position, bool full=true);
    };

The [`DrawTab()`](BTab.md#BTab_DrawTab) function is implemented as follows:
    
    
    const rgb_color kWhite = {255,255,255,255};
    const rgb_color kGray = {219,219,219,255};
    const rgb_color kMedGray = {180,180,180,255};
    const rgb_color kDarkGray = {100,100,100,255};
    const rgb_color kBlackColor = {0,0,0,255};
    
    void CustomTab::DrawTab(BView *owner, BRect frame,
                            tab_position position,
                            bool full) {
       rgb_color      hi;
       rgb_color      lo;
    
       // Save the original colors
    
       hi = owner->HighColor();
       lo = owner->LowColor();
    
       // Draw the label by calling DrawLabel()
    
       owner->SetHighColor(kBlackColor);
       owner->SetLowColor(kGray);
       DrawLabel(owner, frame);
    
       // Start a line array to draw the tab --
       // this is faster than drawing the lines
       // one by one.
    
       owner->BeginLineArray(7);
    
       // Do the bottom left corner, visible
       // only on the frontmost tab.
    
       if (position != B_TAB_ANY) {
          owner->AddLine(BPoint(frame.left, frame.bottom),
                   BPoint(frame.left+3, frame.bottom-3), kWhite);
       }
    
       // Left wall -- always drawn
    
       owner->AddLine(BPoint(frame.left+4, frame.bottom-4),
                BPoint(frame.left+4, frame.top), kWhite);
    
       // Top -- always drawn
    
       owner->AddLine(BPoint(frame.left+5, frame.top),
                BPoint(frame.right-5, frame.top), kWhite);
    
       // Right wall -- always drawn. Has a nice bevel.
    
       owner->AddLine(BPoint(frame.right-4, frame.top),
                BPoint(frame.right-4, frame.bottom-4), kDarkGray);
       owner->AddLine(BPoint(frame.right-5, frame.top),
                BPoint(frame.right-5, frame.bottom-4), kMedGray);
    
       // Bottom-right corner, only visible if the tab
       // is either frontmost or the rightmost tab.
    
       if (full) {
          owner->AddLine(BPoint(frame.right-3, frame.bottom-3),
                   BPoint(frame.right, frame.bottom), kDarkGray);
          owner->AddLine(BPoint(frame.right-4, frame.bottom-3),
                   BPoint(frame.right-1, frame.bottom), kMedGray);
       }
    
       owner->EndLineArray();
    
       owner->SetHighColor(hi);
       owner->SetLowColor(lo);
    }

The `owner` parameter is a pointer to the [`BView`](BView.md "BView") in which the tab is drawn. `frame` is the frame rectangle of the tab; the tab should be drawn to fill this rectangle. The `position` parameter, which can be one of the following values, specifies the placement of the tab, to assist in making intelligent decisions on which parts of the tab are visible and which are not:

Constant| Description  
---|---  
`B_TAB_FIRST`| The tab is the leftmost tab (the one with index 0 in the [`BTabView`](BTabView.md "BTabView")).  
`B_TAB_FRONT`| The tab is the frontmost tab.  
`B_TAB_ANY`| The tab neither the frontmost nor the leftmost tab.  
  
This is a fairly trivial example, and is self-explanatory—with two caveats:
    
    
    // Do the bottom left corner, visible
    // only on the frontmost tab.
    
    if (position != B_TAB_ANY) {
       owner->AddLine(BPoint(frame.left, frame.bottom),
                BPoint(frame.left+3, frame.bottom-3), kWhite);
    }

This code is responsible for drawing the portion of the tab that connects to the box surrounding the [`BTabView`](BTabView.md "BTabView")'s container. In our custom [`BTab`](BTab.md "BTab"), this is simply a diagonal line that extends from the bottom left corner of the frame upward and inward slightly.

However, this portion of the tab is only visible on the first or frontmost tab, so we only draw this segment if its position isn't `B_TAB_ANY` (in other words, if its position is either `B_TAB_FRONT` or `B_TAB_FIRST`).
    
    
    // Bottom-right corner, only visible if the tab
    // is either frontmost or the rightmost tab.
    
    if (full) {
       owner->AddLine(BPoint(frame.right-3, frame.bottom-3),
                BPoint(frame.right, frame.bottom), kDarkGray);
       owner->AddLine(BPoint(frame.right-4, frame.bottom-3),
                BPoint(frame.right-1, frame.bottom), kMedGray);
    }

This code, which draws the lower-right corner of the tab (where it meets back up with the box surrounding the container view), only runs if the `full` parameter is `true`. This is because the right edge of a tab can be obscured by the tab to its left.

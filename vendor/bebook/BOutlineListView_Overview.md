<!-- System Overview - The Interface Kit - BOutlineListView -->

## BOutlineListView

A [`BOutlineListView`](BOutlineListView.md "BOutlineListView") displays a list of items that can be structured like an outline, with items grouped under other items. The levels of the outline are indicated by successive levels of indentation.

![Outline ListView](./images/TheInterfaceKit/outlinelist.png)

The outline list view shown above was created using the following code:
    
    
    BOutlineListView *outline;
    BListItem *region;
    BListItem *state;
    
    SetViewColor(216,216,216,0);
    
    r = Bounds();
    r.InsetBy(5,5);
    r.right -= 16;
    r.top += 20;
    
    outline = new BOutlineListView(r, "cities_list",
                   B_MULTIPLE_SELECTION_LIST);

First, the [`BOutlineListView`](BOutlineListView.md "BOutlineListView") is created, with a rectangle computed by insetting from the parent view's bounds rectangle. The `B_MULTIPLE_SELECTION_LIST` flag is specified to indicate that the user should be allowed to choose more than one item in the list.
    
    
    region = new BStringItem("United States of America")
    outline->AddItem(region);

This creates a new item in level 0 (the "region" level).

The United States is then divided into states, which comprise level 1 of the outline list. This line of code adds California to the list, placing it "under" the region (United States of America):
    
    
    state = new BStringItem("California")
    outline->AddUnder(state, region);

a pointer to the new item for California is saved in the variable `state`.
    
    
    outline->AddUnder(new BStringItem("Menlo Park"), state);
    outline->AddUnder(new BStringItem("Los Angeles"), state);

California is then further divided into cities: Menlo Park and Los Angeles, which reside at level 2 of our outline list. These are inserted under the California item by specifying the pointer to that item (locality) when calling [`AddUnder()`](BOutlineListView.md#BOutlineListView_AddUnder).

This process is repeated for New York state, which has three cities available in our list:
    
    
    locality = new BStringItem("New York")
    outline->AddUnder(locality, region);
    outline->AddUnder(new BStringItem("Albany"), locality);
    outline->AddUnder(new BStringItem("Buffalo"), locality);
    outline->AddUnder(new BStringItem("New York City"), locality);

Then the Europe region is added (in level 0), and the nations of France, Germany, and Italy are added as localities (level 1). Each of those three localities has cities, which are added into level 2.
    
    
    region = new BStringItem("Europe")
    outline->AddItem(region);
    locality = new BStringItem("France")
    outline->AddUnder(locality, region);
    outline->AddUnder(new BStringItem("Paris"), locality);
    
    locality = new BStringItem("Germany")
    outline->AddUnder(locality, region);
    outline->AddUnder(new BStringItem("Berlin"), locality);
    outline->AddUnder(new BStringItem("Hamburg"), locality);
    
    locality = new BStringItem("Italy")
    outline->AddUnder(locality, region);
    outline->AddUnder(new BStringItem("Milan"), locality);
    outline->AddUnder(new BStringItem("Rome"), locality);

Once the list has been completely constructed, a [`BScrollView`](BScrollView.md "BScrollView") is created to contain the outline list view, and is then added to the list. See the [`BScrollView`](BScrollView.md "BScrollView") class for details on how this works:
    
    
    AddChild(new BScrollView("scroll_cities", outline,
             B_FOLLOW_LEFT|B_FOLLOW_TOP, 0, false, true));

Finally, a [`BStringView`](BStringView.md "BStringView") is created to label the list with a prompt indicating that you should "Select the Cities to Attack:".
    
    
    r = Bounds();
    r.InsetBy(5,5);
    r.bottom = r.top + 12;
    AddChild(new BStringView(r, "message_string",
          "Select the Cities to Attack:"));

* * *

### Outline Structure

If an item has other items under it—that is, if the immediately following item in the list is at a deeper level of the outline—it is a superitem; the items grouped under it are its subitems. Superitems are marked by a triangular icon or latch, in the usual interface for hypertext lists.

The user can collapse or expand sections of the outline by manipulating the latch. When a section is collapsed, only the superitem for that section is visible (and the latch points to the superitem). All items that follow the superitem are hidden, up to the next item that's not at a deeper outline level. When a section is expanded, subitems are visible (and the latch points downward).

* * *

### Inherited Functions

The [`BOutlineListView`](BOutlineListView.md "BOutlineListView") class inherits most of its functionality from the [`BListView`](BListView.md "BListView") class. However, inherited functions are concerned only with the expanded sections of the list, not with sections that are hidden because they're collapsed. If an inherited function returns an index or takes an index as an argument, the index counts just the items that are shown on-screen (or could be shown on-screen if they were scrolled into the visible region of the view). [`DoForEach()`](BListView.md#BListView_DoForEach "DoForEach\(\)") skips items that can't be displayed. [`CountItems()`](BListView.md#BListView_CountItems "CountItems\(\)") counts items only in the expanded sections of the list.

However, the functions that the [`BOutlineListView`](BOutlineListView.md "BOutlineListView") class itself defines are concerned with all sections of the list, expanded or collapsed. For its functions, an index counts all items in the list, whether visible or not.

The class defines some functions that match those it inherits, but its versions prefix `FullList…()` to the function name and don't ignore any items. For example, [`FullListCountItems()`](BOutlineListView.md#BOutlineListView_FullListCountItems "FullListCountItems\(\), FullListCurrentSelection\(\), FullListFirstItem\(\) , FullListLastItem\(\), FullListIndexOf\(\), FullListItemAt\(\), FullListHasItem\(\), FullListIsEmpty\(\), FullListDoForEach\(\), FullListSortItems\(\)") counts every item in the list and [`FullListDoForEach()`](BOutlineListView.md#BOutlineListView_FullListDoForEach) doesn't skip items in collapsed sections.

In some cases, [`BOutlineListView`](BOutlineListView.md "BOutlineListView") simply overrides an inherited function without adding the `FullList…()` prefix. You should always use the [`BOutlineListView`](BOutlineListView.md "BOutlineListView") versions of these functions, not the [`BListView`](BListView.md "BListView") versions. For example, [`BOutlineListView`](BOutlineListView.md "BOutlineListView")'s version of [`MakeEmpty()`](BOutlineListView.md#BOutlineListView_MakeEmpty "MakeEmpty\(\)") truly empties the list; [`BListView`](BListView.md "BListView")'s version would remove items from the screen, but not from the real list.

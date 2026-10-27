<!-- Classes And Methods - The Storage Kit - BRefFilter -->

## BRefFilter

| Derived From:| None  
---|---  
Mix-in Classes:| None  
Declared In:| `storage/FilePanel.h`  
Library:| None  
Allocation:| –  
[Class Overview](BRefFilter_Overview.md "BRefFilter")

  * [Hook Functions](BRefFilter.md#BRefFilter_Hook_Functions)

  
  
* * *

### Hook Functions

#### Filter()

`virtual bool Filter(const entry_ref* ref,  
[`BNode`](BNode.md "BNode")* node,  
struct stat* st,  
const char* filetype);`

`Filter()` is a hook function that's invoked whenever the file panel to which it's been assigned reads the entries in its "panel directory." The function is invoked once for each entry in the directory. All the arguments to the function refer to the entry currently under consideration. (Note that the function is never sent an abstract entry, so the `node`, `st`, and `filetype` arguments will always be valid.)

Your implementation of `Filter()` can use any or all of the arguments to figure out if the entry is a valid candidate for display in the file panel's file list. Simply return `true` or `false` to indicate if the entry is a winner or a loser.

Technically, `Filter()` is invoked when…

  * …the file panel's panel directory is set, either through the `BFilePanel` constructor or the [`SetPanelDirectory()`](BFilePanel.md#BFilePanel_SetPanelDirectory "SetPanelDirectory\(\), GetPanelDirectory\(\)"), and when…

  * …the file panel's [`Refresh()`](BFilePanel.md#BFilePanel_Refresh "Refresh\(\)") function is called.

A `BRefFilter` can be assigned to more than one [`BFilePanel`](BFilePanel.md "BFilePanel") object (assignation is performed through [`BFilePanel`](BFilePanel.md "BFilePanel")'s constructor or [`SetRefFilter()`](BFilePanel.md#BFilePanel_SetRefFilter "SetRefFilter\(\), RefFilter\(\)") function). But it's not probably not a great idea to do so: At any particular invocation of `Filter()`, the `BRefFilter` doesn't which [`BFilePanel`](BFilePanel.md "BFilePanel") object it's working for.

You maintain ownership of the `BRefFilter` objects that you create. Assigning a ref filter to a file panel does not hand ownership of the `BRefFilter` to the [`BFilePanel`](BFilePanel.md "BFilePanel"). You shouldn't delete a `BRefFilter` while a [`BFilePanel`](BFilePanel.md "BFilePanel") is still using it; but it's your responsibility to delete it when it's done.

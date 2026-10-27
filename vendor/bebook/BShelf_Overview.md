<!-- System Overview - The Interface Kit - BShelf -->

## BShelf  
  
A [`BShelf`](BShelf.md "BShelf") is an object that you "attach" to a view in order to make the view accept dropped [`BDragger`](BDragger.md "BDragger") objects. In user-talk, a shelf receives and displays replicants. Attaching a [`BShelf`](BShelf.md "BShelf") to a view (called the "container" view) is remarkably simple:
    
    
    BShelf *shelf = new BShelf(container_view);

That's all there is to it: With this single line of code, `container_view` is primed to accept and (automatically) display dropped replicants. A dropped replicant becomes a child of the container view. The container view itself can be any [`BView`](BView.md "BView") object—you don't alter the view in any way, or even tell it that it's going to be a container.

Note

![Note](./images/admonitions/Info_32.png)

Attaching a shelf to a view is performed by the [`BShelf`](BShelf.md "BShelf") constructor only. You can't create a [`BShelf`](BShelf.md "BShelf") and then decide which view you want it to serve.

* * *

### Dropping into the View Hierarchy

When the user drops a replicant on a container view, the view receives a `B_ARCHIVED_OBJECT` message that contains a [`BDragger`](BDragger.md "BDragger") and the dragger's target (a [`BView`](BView.md "BView")). These two views (the [`BDragger`](BDragger.md "BDragger") and its target) are related as parent-child, child-parent, or as siblings (as explained in the [`BDragger`](BDragger.md "BDragger") class). The "more elderly" of the two objects is added as a child of the container view; if they're siblings, the two objects are both added as children.

You can also send or post `B_ARCHIVED_OBJECT` messages to a [`BShelf`](BShelf.md "BShelf") to simulate a drag and drop.

* * *

### Other BShelf Features

Settings.
    

A [`BShelf`](BShelf.md "BShelf") can configure itself from a "settings" file, and can write its contents to that file. Your only chance to associate a [`BShelf`](BShelf.md "BShelf") with a settings file is during construction. You can save the contents at any time through the [`Save()`](BShelf.md#BShelf_Save "Save\(\), SaveLocation\(\), SetSaveLocation\(\), SetDirty\(\), IsDirty\(\)") function.

Replicant rejection.
    

A [`BShelf`](BShelf.md "BShelf") can reject replicants, and can adjust the position of the replicants that it accepts. These features are provided through the hook functions [`CanAcceptReplicantMessage()`](BShelf.md#BShelf_CanAcceptReplicantMessage "CanAcceptReplicantMessage\(\), CanAcceptReplicantView\(\)"), [`CanAcceptReplicantView()`](BShelf.md#BShelf_CanAcceptReplicantView), and [`AdjustReplicantBy()`](BShelf.md#BShelf_AdjustReplicantBy "AdjustReplicantBy\(\)"). A named shelf can also reject replicants, as explained next.

`BShelf` names.
    

When a replicant message is dropped on the shelf, the shelf's name (set in the constructor) is compared to the dropped message's "shelf_type" field (if it has one). If the two don't match, the replicant rejects the shelf. In this way, individual replicants can be picky about the shelves that they want to be displayed on. By default, a shelf accepts replicants that don't have a "shelf_type" field. However, if a [`BShelf`](BShelf.md "BShelf") is set to "enforce type" (set through [`SetTypeEnforced()`](BShelf.md#BShelf_SetTypeEnforced "SetTypeEnforced\(\) , IsTypeEnforced\(\)")), then it will only accept replicants that name the shelf (in their "shelf_type" fields).

Persistent IDs
    

A [`BShelf`](BShelf.md "BShelf") assigns each replicant a unique id that's valid for as long as the replicant is attached to the shelf. Replicant ids don't change, even if the shelf is [`Save()`](BShelf.md#BShelf_Save "Save\(\), SaveLocation\(\), SetSaveLocation\(\), SetDirty\(\), IsDirty\(\)")'d and then later restored (through the constructor).

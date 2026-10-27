<!-- System Overview - The Interface Kit - BDragger -->

## BDragger  
  
A [`BDragger`](BDragger.md "BDragger") is a view that lets users drag and drop some other view. The other view is the target of the [`BDragger`](BDragger.md "BDragger") and its immediate relative—a sibling, a parent, or an only child. The [`BDragger`](BDragger.md "BDragger") draws a handle, usually at the corner of the target view, that the user can grab. When the user drags the handle the target view appears to move with the handle.

When dragged in this way, the target view itself doesn't actually move. Instead, the view is archived in a [`BMessage`](BMessage.md "BMessage") object and the [`BMessage`](BMessage.md "BMessage") is dragged. When the [`BMessage`](BMessage.md "BMessage") is dropped, the target [`BView`](BView.md "BView") can be reconstructed from the archive (along with the [`BDragger`](BDragger.md "BDragger")). The new object is a duplicate—a replicant—of the target view.

This class works closely with the [`BShelf`](BShelf.md "BShelf") class. A [`BShelf`](BShelf.md "BShelf") object accepts dragged [`BView`](BView.md "BView")s, reconstructs them from their archives, and installs them in another view hierarchy.

[`BDragger`](BDragger.md "BDragger")s are under the control of DeskBar's "Show Replicants" / "Hide Replicants" menu item. Showing replicants means that the [`BDragger`](BDragger.md "BDragger") handles are visible on-screen; hiding replicants means that the handles are hidden.

<!-- Classes And Methods - The Interface Kit - BShelf -->

## BShelf

| Derived From:| [`BHandler`](BHandler.md "BHandler")  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/Shelf.h`  
Library:| `libbe.so`  
Allocation:| Constructor only  
[Class Overview](BShelf_Overview.md "BShelf")

  * [Constructor and Destructor](BShelf.md#BShelf_ConstructorDestructor)
  * [Hook Functions](BShelf.md#BShelf_HookFunctions)
  * [Member Functions](BShelf.md#BShelf_MemberFunctions)
  * [Static Functions](BShelf.md#BShelf_StaticFunctions)
  * [Scripting Support](BShelf.md#BShelf_ScriptingSupport)

  
  
* * *

### Constructor and Destructor

#### BShelf()

`BShelf([`BView`](BView.md "BView")* view,  
bool allowsDragging = true,  
const char* name = NULL);` `BShelf(const entry_ref* ref,  
[`BView`*](BView.md "BView") view,  
bool allowsDragging = true,  
const char* name = NULL);` `BShelf([`BDataIO`](BDataIO.md "BDataIO")* stream,  
[`BView`*](BView.md "BView") view,  
bool allowsDragging = true,  
const char* name = NULL);`

Initializes the `BShelf` object so that it serves a container view. The versions that accept an entry_ref or [`BDataIO`](BDataIO.md "BDataIO") argument prime the shelf so that it (initially) contains the replicants that are archived in the referred to file or stream using [`Save()`](BShelf.md#BShelf_Save "Save\(\), SaveLocation\(\), SetSaveLocation\(\), SetDirty\(\), IsDirty\(\)"). The `ref`/`stream` argument is also used as the archival repository when you tell your `BShelf` to [`Save()`](BShelf.md#BShelf_Save "Save\(\), SaveLocation\(\), SetSaveLocation\(\), SetDirty\(\), IsDirty\(\)") itself.

If the `allowsDragging` flag is `true`, the user will be able to drag replicant view within the container's bounds. If the flag is `false`, dropped views stay where they're first put.

`name` is the `BShelf`'s handler name. The name can be important: It's compared to the replicant's `shelf_type` field, as explained in [`AddReplicant()`](BShelf.md#BShelf_AddReplicant "AddReplicant\(\)").

Warning

![Warning](./images/admonitions/Stop_32.png)

There's an archive-accepting version of the `BShelf` constructor declared in `Shelf.h`. Don't use it.

#### ~BShelf()

`virtual ~BShelf();`

The destructor calls [`Save()`](BShelf.md#BShelf_Save "Save\(\), SaveLocation\(\), SetSaveLocation\(\), SetDirty\(\), IsDirty\(\)"), and then frees the object.

* * *

### Hook Functions

#### AdjustReplicantBy()

##### protected

`virtual [`BPoint`](BPoint.md "BPoint") AdjustReplicantBy([`BRect`](BRect.md "BRect") destRect,  
[`BMessage`](BMessage.md "BMessage")* archive) const;`

This hook function is invoked automatically when a replicant is dropped on the `BShelf`. It gives the shelf a chance to fine-tune the placement of the `BDragger` and its target view.

`destRect` is the rectangle (in the container view's coordinates) in which the dropped replicant is about to be drawn. Exactly what the rectangle means depends on the relationship between the dragger and its target:

  * If the dragger is the target's parent, then `destRect` encloses the [`BDragger`](BDragger.md "BDragger")'s frame.

  * Otherwise (if the target is the parent, or if the two views are siblings), `destRect` encloses the target's frame. Note that in the case of siblings, the [`BDragger`](BDragger.md "BDragger")'s frame isn't included in the rectangle.

`archive` is the archive message that was dropped on the shelf.

The [`BPoint`](BPoint.md "BPoint") that this function returns offsets (is added into) the location of the replicant. If you don't want to move the replicant, return [`BPoint`](BPoint.md "BPoint")(0, 0). Note that the [`BDragger`](BDragger.md "BDragger") and the view are both moved by this offset, even in the case where `destRect` doesn't include the dragger's frame.

This function ignores the "allows dragging" flag given in the `BShelf` constructor. In other words, you can adjust a replicant's placement through this function even if the `BShelf` doesn't otherwise allow dragging.

####  CanAcceptReplicantMessage(), CanAcceptReplicantView()

##### protected

`virtual bool CanAcceptReplicantMessage([`BMessage`](BMessage.md "BMessage")* archive) const;``virtual bool CanAcceptReplicantView([`BRect`](BRect.md "BRect") destRect,  
[`BView`](BView.md "BView")* view,  
[`BMessage`](BMessage.md "BMessage")* archive) const;`

These hook functions are invoked from within [`AddReplicant()`](BShelf.md#BShelf_AddReplicant "AddReplicant\(\)") whenever a replicant is dropped on the `BShelf`. You can implement these functions to reject unwanted replicants.

`CanAcceptReplicantMessage()` is called first; the argument is the archive that (should) contain the replicated view. If you don't like the look of the archive, return `false` and the message will be thrown away. Note that you shouldn't return `false` if the archive doesn't seem to be in the correct form (specifically, if it doesn't contain any views). Rejection of such messages is handled more elegantly (and after this function is invoked) by the [`AddReplicant()`](BShelf.md#BShelf_AddReplicant "AddReplicant\(\)") function.

`CanAcceptReplicantView()` is invoked after the message has been unarchived. `destRect` is the rectangle that the replicant will occupy in the `BShelf`'s container view's coordinates. `view` is the replicated view itself. `archive` is the original message.

If either function returns `false`, the replicant is rejected and the message is thrown away (it isn't passed on to another handler). A return of `true` does the obvious thing.

#### ReplicantDeleted()

##### protected

`virtual void ReplicantDeleted(int32 index,  
const [`BMessage`](BMessage.md "BMessage")* archive,  
const [`BView`](BView.md "BView")* view);`

This hook function is invoked from within [`DeleteReplicant()`](BShelf.md#BShelf_DeleteReplicant "DeleteReplicant\(\)") to indicate a replicant has been deleted from the `BShelf`. It is called after the view has been removed from the shelf with the `index` of the replicant in the shelf, its `archive` message, and the detached `view`.

* * *

### Member Functions

#### AddReplicant()

`status_t AddReplicant([`BMessage`](BMessage.md "BMessage")* archive,  
[`BPoint`](BPoint.md "BPoint") point);`

This function is invoked automatically when a replicant is dropped on the `BShelf`. The `archive` message contains the [`BDragger`](BDragger.md "BDragger") archive that's being dropped; `point` is where, within the container view's bounds, the message was dropped. The function goes through these steps to reject and adjust the replicant:

  * First, it invokes the [`CanAcceptReplicantMessage()`](BShelf.md#BShelf_CanAcceptReplicantMessage "CanAcceptReplicantMessage\(\), CanAcceptReplicantView\(\)") hook function. If the hook returns `false`, then `AddReplicant()` doesn't add the replicant.

  * Next, it looks for a `shelf_type` string field in the [`BMessage`](BMessage.md "BMessage"). If it finds one and the value of the field doesn't match the `BShelf`'s name, the replicant is rejected.

  * If type enforcement is `true` (see [`SetTypeEnforced()`](BShelf.md#BShelf_SetTypeEnforced "SetTypeEnforced\(\) , IsTypeEnforced\(\)")) and the shelf has a name, then the [`BMessage`](BMessage.md "BMessage") must have a `shelf_type` string and this string must match the shelf name. Otherwise, the replicant is rejected.

There's no specific API for adding the `shelf_type` field to a view. If you want to configure your views to accept only certain `BShelf` objects, you have to add the field directly as part of the view's [`Archive()`](BView.md#BView_Archive "Archive\(\)") implementation.

  * The archive message is then unarchived (the replicant is instantiated). If the archive doesn't contain a [`BView`](BView.md "BView"), the message is passed on to another handler (`B_DISPATCH_MESSAGE` is returned).

  * [`CanAcceptReplicantView()`](BShelf.md#BShelf_CanAcceptReplicantView) hook function is called next (with a return of `false` meaning rejection).

  * Finally, [`AdjustReplicantBy()`](BShelf.md#BShelf_AdjustReplicantBy "AdjustReplicantBy\(\)") is called, and the replicant is drawn in the container view.

Except in the case of a no-view archive, `AddReplicant()` returns `B_SKIP_MESSAGE`.

Note

![Note](./images/admonitions/Info_32.png)

If you want the ensure that the replicant is unique within the container view, add a "be:unique_replicant" entry of type `B_BOOL_TYPE` to the archive with the value `true`.

It's possible to archive a `BDragger` and call this function yourself, although that's not its expected use.

#### Archive()

Warning

![Warning](./images/admonitions/Stop_32.png)

`Archive()` is currently a no-op that returns `B_ERROR`. You can't archive a `BShelf`. If you want to archive something, archive the shelf's contents by calling [`Save()`](BShelf.md#BShelf_Save "Save\(\), SaveLocation\(\), SetSaveLocation\(\), SetDirty\(\), IsDirty\(\)").

#### CountReplicants()

`int32 CountReplicants() const;`

Returns the number of replicants attached to the shelf.

#### DeleteReplicant()

`status_t DeleteReplicant([`BView`](BView.md "BView")* replicant_view);` `status_t DeleteReplicant(const [`BMessage`](BMessage.md "BMessage")* archive);` `status_t DeleteReplicant(uint32 uid);`

Removes the specified replicant from the shelf. It identifies replicants by either a view, a replicant message, or a unique id.

####  IndexOf(), ReplicantAt()

`int32 IndexOf(const [`BView`](BView.md "BView")* replicant_view) const;` `int32 IndexOf(const [`BMessage`](BMessage.md "BMessage")* archive) const;` `int32 IndexOf(uint32 uid) const;`

`[`BMessage`](BMessage.md "BMessage")* ReplicantAt(int32 index,  
[`BView`](BView.md "BView")** view = NULL,  
uint32* view = NULL,  
status_t err = NULL) const;`

`IndexOf()` returns the index of a specified replicant in the shelf, or -1 if no such replicant exists. It accepts either a view, a replicant message, or a unique id as identifiers.

`ReplicantAt()` returns information about a replicant in a shelf given its `index` (as returned by `IndexOf()`). It returns the [`BMessage`](BMessage.md "BMessage") archive of the replicant or `NULL` if the index is invalid. It returns the `view` of the replicant as well as its `uid`, if these parameters are non-`NULL`. It returns an error message in `err` if there was an error in initializing the given replicant.

####  Save(), SaveLocation(), SetSaveLocation(), SetDirty(), IsDirty()

`status_t Save();``[`BDataIO`](BDataIO.md "BDataIO")* SaveLocation(entry_ref* ref) const;`

`status_t SetSaveLocation([`BDataIO`](BDataIO.md "BDataIO")* data_io);` `status_t SetSaveLocation(const entry_ref* ref);`

`virtual void SetDirty(bool flag);``bool IsDirty();`

Writes the shelf's contents (the replicants that it contains) as an archive to the entry_ref or [`BDataIO`](BDataIO.md "BDataIO") object that was specified in the constructor. You can also set the location where the shelf is saved with `SetSaveLocation()` and fetch it with `SaveLocation()`. The entry_ref is stored in `ref` (if ref is non-`NULL`) and the [`BDataIO`](BDataIO.md "BDataIO") the shelf will be written to is returned. If the shelf will be written to an entry_ref that is not a [`BDataIO`](BDataIO.md "BDataIO"), `SaveLocation()` returns `NULL`.

By default, the save is only performed if the object's "dirty" flag is set—in other words, if it has changed since it was last written. You can force set the dirty flag by calling `SetDirty()`.

`IsDirty()` returns the current state of the "dirty" flag.

####  SetAllowsDragging() , AllowsDragging()

`void SetAllowsDragging(bool state);``bool AllowsDragging() const;`

`SetAllowsDragging()` determines whether the `BShelf` accepts replicants dragged into the view by the user. `AllowsDragging()` returns whether the `BShelf` accepts replicants dragged by the user into the container view's frame

####  SetAllowsZombies(), AllowsZombies(), SetDisplayZombies(), DisplaysZombies()

`void SetAllowsZombies(bool state);``bool AllowsZombies() const;``void SetDisplaysZombies(bool state);``bool DisplaysZombies() const;`

`SetAllowsZombies()` determines whether the `BShelf` accepts zombie views. `AllowsZombies()` returns whether the `BShelf` accepts zombie views. A zombie view is one whose associated executable cannot be located.

Similarly, `SetDisplaysZombies()` determines whether the `BShelf` displays zombie views and `DisplaysZombies()` returns whether the `BShelf` displays zombie views.

####  SetTypeEnforced() , IsTypeEnforced()

`void SetTypeEnforced(bool state);``bool IsTypeEnforced() const;`

These two methods set and return the type enforcement flag. When it is `true`, the shelf compares its name to the `shelf_type` field of any dropped messages. The replicant is accepted only if the two match. If the dropped message does not have a `shelf_type` field, then it is rejected. Type enforcement is `false` by default.

* * *

### Static Functions

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Warning

![Warning](./images/admonitions/Stop_32.png)

`Instantiate()` is currently a no-op that returns `NULL`. You can't archive a `BShelf`. If you want to archive something, archive the shelf's contents by calling [`Save()`](BShelf.md#BShelf_Save "Save\(\), SaveLocation\(\), SetSaveLocation\(\), SetDirty\(\), IsDirty\(\)").

* * *

### Scripting Support

The `BShelf` class implements the suite called "suite/vnd.Be-shelf" consisting of the following message:

#### The Replicant Property

The "Replicant" property provides access to the replicants contained in a `BShelf`. It also allows you to manipulate the replicants themselves by forwarding certain messages to a "suite/vnd.Be-replicant" interface. This interface consists of the following messages:

Message| Specifier| Description  
---|---|---  
`B_COUNT_PROPERTIES`| `B_DIRECT_SPECIFIER`| Returns number of replicants in the shelf.  
`B_CREATE_PROPERTIES`| `B_DIRECT_SPECIFIER`| Adds the archived replicant in the [`BMessage`](BMessage.md "BMessage") "data" to the view at the [`BPoint`](BPoint.md "BPoint") in "location."  
`B_DELETE_PROPERTY`| `B_INDEX_SPECIFIER`, `B_REVERSE_INDEX_SPECIFIER`, `B_NAME_SPECIFIER`, `B_ID_SPECIFIER`| Removes the specified replicant from the shelf.  
`B_GET_PROPERTY`| `B_INDEX_SPECIFIER`, `B_REVERSE_INDEX_SPECIFIER`, `B_NAME_SPECIFIER`, `B_ID_SPECIFIER`| Archives the specified replicant into a BMessage in "result."  
anything else| `B_INDEX_SPECIFIER`, `B_REVERSE_INDEX_SPECIFIER`, `B_NAME_SPECIFIER`, `B_ID_SPECIFIER`| Directs the scripting message to the replicant interface "suite/vnd.Be-replicant" (described below), first popping the current specifier off the stack.  
  
#### The ID Property

The replicant ID

Message| Specifier| Description  
---|---|---  
`B_GET_PROPERTY`| `B_DIRECT_SPECIFIER`| Returns the replicant ID.  
  
#### The Name Property

The name of the replicant view

Message| Specifier| Description  
---|---|---  
`B_GET_PROPERTY`| `B_DIRECT_SPECIFIER`| Returns the name of the replicant view.  
  
#### The Signature Property

The replicant add-on MIME signature

Message| Specifier| Description  
---|---|---  
`B_GET_PROPERTY`| `B_DIRECT_SPECIFIER`| Returns the signature of the add-on containing the code for the replicant.  
  
#### The Suites Property

The supported scripting suites

Message| Specifier| Description  
---|---|---  
`B_GET_PROPERTY`| `B_DIRECT_SPECIFIER`| Returns "suite/vnd.Be-replicant" in "suites" and a flattened [`BPropertyInfo`](BPropertyInfo.md "BPropertyInfo") describing the scripting suite in "messages."  
  
#### The View Property

Redirects messages to the replicant view

Message| Specifier| Description  
---|---|---  
any| `B_DIRECT_SPECIFIER`| Directs the scripting message to the replicant view, first popping the current specifier off the specifier stack.

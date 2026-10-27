<!-- Classes And Methods - The Input Server - BInputServerFilter -->

## BInputServerFilter

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `add-ons/input_server/InputServerFilter.h`  
Library:| `libbe.so`  
Allocation:| By the Input Server only  
[Class Overview](BInputServerFilter_Overview.md "BInputServerFilter")

  * [Constructor and Destructor](BInputServerFilter.md#BInputServerFilter_ConstructorDestructor)
  * [Hook Functions](BInputServerFilter.md#BInputServerFilter_HookFunctions)
  * [Member Functions](BInputServerFilter.md#BInputServerFilter_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BInputServerFilter()

`BInputServerFilter();`

Creates a new `BInputServerFilter` object. You can initialize the object—set initial values, spawn threads, etc.—either here or in the [`InitCheck()`](BInputServerFilter.md#BInputServerFilter_InitCheck "InitCheck\(\)") function, which is called immediately after the constructor.

#### ~BInputServerFilter()

`BInputServerFilter();`

Deletes the `BInputServerFilter` object. The destructor is invoked by the Input Server only—you never delete a `BInputServerFilter` object from your own code. If this object has spawned its own threads or allocated memory on the heap, it must clean up after itself here.

* * *

### Hook Functions

#### Filter()

`virtual filter_result Filter([`BMessage`](BMessage.md "BMessage")* message,  
[`BList`](BList.md "BList")* outList);`

The `Filter()` hook function is invoked by the Input Server to filter the in-coming message. You can discard the message, allow it to pass downstream (as is or modified), or replace it with one or more other messages:

  * To discard message, implement `Filter()` to return `B_SKIP_MESSAGE` (don't delete the [`BMessage`](BMessage.md "BMessage") yourself—it's owned by the Input Server).

  * If you implement `Filter()` to return `B_DISPATCH_MESSAGE` (and if you leave `outList` unchanged), `message` is allowed to continue downstream. You're allowed to modify (but not destroy or replace) `message`.

  * To replace `message`, place one or more [`BMessage`](BMessage.md "BMessage") in `outList` and return `B_DISPATCH_MESSAGE`. `message` is discarded; the messages in `outList` are passed downstream in the order that they appear in the list. The Input Server owns all of the messages you pass back in `outList`, so don't delete them yourself. Note that if you want to include `message` in `outList`, you must copy the object before adding it to the list.

The default implementation returns `B_DISPATCH_MESSAGE` without modifying the message.

#### InitCheck()

`virtual status_t InitCheck();`

Invoked by the Input Server immediately after the object is constructed to test the validity of the initialization. If the object is properly initialized (i.e. all required resources are located or allocated), this function should return `B_OK`. If the object returns non-`B_OK`, the object is deleted and the add-on is unloaded.

The default implementation returns `B_OK`.

* * *

### Member Functions

#### GetScreenRegion()

`status_t GetScreenRegion([`BRegion`](BRegion.md "BRegion")* region) const;`

The `GetScreenRegion()` function returns the screen's region in `region`. This is the most efficient way for an input filter to get the screen's region. The system screen saver's input filter uses this for its "sleep now"/"sleep never" corners.

`GetScreenRegion()` returns `B_OK`.

<!-- Classes And Methods - The Interface Kit - BPictureButton -->

## BPictureButton  
  
| Derived From:| [`BControl`](BControl.md "BControl") > [`BView`](BView.md "BView") > [`BHandler`](BHandler.md "BHandler") > [`BArchivable`](BArchivable.md "BArchivable")  
---|---  
Mix-in Classes:| [`BInvoker`](BInvoker.md "BInvoker")  
Declared In:| `interface/PictureButton.h`  
Library:| `libbe.so`  
Allocation:| Constructor only  
[Class Overview](BPictureButton_Overview.md "BPictureButton")

  * [Constructor and Destructor](BPictureButton.md#BPictureButton_ConstructorDestructor)
  * [Member Functions](BPictureButton.md#BPictureButton_MemberFunctions)
  * [Static Functions](BPictureButton.md#BPictureButton_StaticFunctions)
  * [Archived Fields](BPictureButton.md#BPictureButton_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BPictureButton()

`BPictureButton([`BRect`](BRect.md "BRect") frame,  
const char* name,  
[`BPicture`](BPicture.md "BPicture")* off,  
[`BPicture`](BPicture.md "BPicture")* on,  
[`BMessage`](BMessage.md "BMessage")* message,  
uint32 behavior = B_ONE_STATE_BUTTON,  
uint32 resizingMode = B_FOLLOW_LEFT | B_FOLLOW_TOP,  
uint32 flags = B_WILL_DRAW | B_NAVIGABLE);` `BPictureButton([`BMessage`](BMessage.md "BMessage")* archive);`

Creates a new `BPictureButton`. The `off` and an `on` images correspond to the object's `B_CONTROL_OFF` and `B_CONTROL_ON` values; the `behavior` argument sets the object to be a one-state (`B_ONE_STATE_BUTTON`) or two-state (`B_TWO_STATE_BUTTON`) control, as explained in the class overview. The other arguments are inherited from the [`BView`](BView.md "BView") and [`BControl`](BControl.md "BControl") constructors. The object's initial value is `B_CONTROL_OFF`.

If the `BPictureButton` can be disabled, it needs additional "disabled" images, as set through [`SetDisabledOff()`](BPictureButton.md#BPictureButton_SetDisabledOff) and [`SetDisabledOn()`](BPictureButton.md#BPictureButton_SetDisabledOn).

The `BPictureButton` copies all [`BPicture`](BPicture.md "BPicture")s that are passed to it. It's the caller's responsibility to free the [`BPicture`](BPicture.md "BPicture") objects that are passed as arguments.

#### ~BPictureButton()

`virtual ~BPictureButton();`

Deletes the object and its data.

* * *

### Member Functions

#### Archive()

`virtual Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Stores the `BPictureButton` in the [`BMessage`](BMessage.md "BMessage") archive.

See also: [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)"), [`Instantiate()`](BPictureButton.md#BPictureButton_Instantiate "Instantiate\(\)") static function

####  SetBehavior(), Behavior()

`virtual void SetBehavior(uint32 behavior);``uint32 Behavior() const;`

These functions set and return whether the `BPictureButton` is a one-state (`B_ONE_STATE_BUTTON`) or a two-state (`B_TWO_STATE_BUTTON`) control. A one-state object acts like a normal button: It's on while the user is pressing it, and off otherwise. A two-state object switches to the opposite state each time the user presses and release the button.

####  SetEnabledOff(), SetEnabledOn(), SetDisabledOff(), SetDisabledOn(), EnabledOff(), EnabledOn(), DisabledOff(), DisabledOn()

`virtual void SetEnabledOff([`BPicture`](BPicture.md "BPicture")* picture);``[`BPicture`](BPicture.md "BPicture")* EnabledOff() const;``virtual void SetEnabledOn([`BPicture`](BPicture.md "BPicture")* picture);``[`BPicture`](BPicture.md "BPicture")* EnabledOn() const;``virtual void SetDisabledOff([`BPicture`](BPicture.md "BPicture")* picture);``[`BPicture`](BPicture.md "BPicture")* DisabledOff() const;``virtual void SetDisabledOn([`BPicture`](BPicture.md "BPicture")* picture);``[`BPicture`](BPicture.md "BPicture")* DisabledOn() const;`

These pairs of functions set and return one of the four images the `BPictureButton` displays: enabled-and-on, enabled-and-off, disabled-and-on, and disabled-and-off, respectively. If this is a one-state object, the disabled-and-on image needn't be set since a disabled one-state control can never be on.

The [`BPicture`](BPicture.md "BPicture")-retrieving functions return `NULL` if the requested image hasn't been set.

The `BPictureButton` copies all [`BPicture`](BPicture.md "BPicture") that are passed to it. It's the caller's responsibility to free the [`BPicture`](BPicture.md "BPicture") objects that are passed as arguments.

* * *

### Static Functions

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Returns a new `BPictureButton` object, allocated by new and created with the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive. However, if the archive message doesn't contain data for a `BPictureButton` object, this function returns `NULL`.

See also: [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"), [`Archive()`](BPictureButton.md#BPictureButton_Archive "Archive\(\)")

* * *

### Archived Fields

The [`Archive()`](BPictureButton.md#BPictureButton_Archive "Archive\(\)") function adds the following fields to its [`BMessage`](BMessage.md "BMessage") argument:

Field| Type code| Description  
---|---|---  
`_e_on` (deep copy only)| `B_MESSAGE_TYPE`| [`BPicture`](BPicture.md "BPicture") displayed when control is enabled and on.  
`_e_off` (deep copy only)| `B_MESSAGE_TYPE`| [`BPicture`](BPicture.md "BPicture") displayed when control is enabled and off.  
`_d_on` (deep copy only)| `B_MESSAGE_TYPE`| [`BPicture`](BPicture.md "BPicture") displayed when control is disabled and on.  
`_d_off` (deep copy only)| `B_MESSAGE_TYPE`| [`BPicture`](BPicture.md "BPicture") displayed when control is disabled and off.  
`_behave`| `B_INT32_TYPE`| `BPictureButton` behavior either `B_ONE_STATE_BUTTON` or `B_TWO_STATE_BUTTON`  
  
See also: [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)")

<!-- Classes And Methods - The Interface Kit - BCheckBox -->

## BCheckBox

| Derived From:| [BControl](BControl.md "BControl") > [BView](BView.md "BView") > [BHandler](BHandler.md "BHandler")  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/CheckBox.h`  
Library:| `libbe.so`  
Allocation:| Constructor only  
[Class Overview](BCheckBox_Overview.md "BCheckBox")

  * [Constructor and Destructor](BCheckBox.md#BCheckBox_ConstructorDestructor)
  * [Member Functions](BCheckBox.md#BCheckBox_MemberFunctions)
  * [Archived Fields](BCheckBox.md#BCheckBox_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BCheckBox()

`BCheckBox([`BRect`](BRect.md "BRect") frame,  
const char* name,  
const char* label,  
[`BMessage`](BMessage.md "BMessage")* message,  
uint32 resizingMode = B_FOLLOW_LEFT | B_FOLLOW_TOP,  
uint32 flags = B_WILL_DRAW | B_NAVIGABLE);` `BCheckBox([`BMessage`](BMessage.md "BMessage")* archive);`

Initializes the `BCheckBox` by passing all arguments to the [`BControl`](BControl.md "BControl") [`constructor`](BControl.md#BControl_Constructor "BControl\(\)")

If necessary, the bottom coordinate of the object's frame rectangle is increased to accommodate the height of the label, given the object's font (`be_plain_font`, by default).

Freshly constructed, the object's state is `B_CONTROL_OFF`. Like all controls, the check box's view and low colors are set to those of its parent when the object is attached to a window.

#### ~BCheckBox()

`virtual ~BCheckBox();`

Deletes the `BCheckBox` object.

* * *

### Member Functions

All of `BCheckBox`'s member functions are implementations of functions declared by its superclasses. For information on specific functions, look in [`BControl`](BControl.md "BControl") , then in [`BView`](BView.md "BView") , and so on up the inheritance hierarchy.

* * *

### Archived Fields

See "[Archived Fields](BControl.md#BControl_ArchivedFields "Archived Fields")" in the [`BControl`](BControl.md "BControl") class. `BCheckbox` doesn't add any of its own fields to the archive message.

<!-- Classes And Methods - The Interface Kit - BSeparatorItem -->

## BSeparatorItem

| Derived From:| [`BMenuItem`](BMenuItem.md "BMenuItem")  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/SeparatorItem.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BSeparatorItem_Overview.md "BSeparatorItem")

  * [Constructor and Destructor](BSeparatorItem.md#BSeparatorItem_ConstructorDestructor)
  * [Member Functions](BSeparatorItem.md#BSeparatorItem_MemberFunctions)
  * [Static Functions](BSeparatorItem.md#BSeparatorItem_StaticFunctions)

  
  
* * *

### Constructor and Destructor

#### BSeparatorItem()

`BSeparatorItem();``BSeparatorItem([`BMessage`](BMessage.md "BMessage")* data);`

#### ~BSeparatorItem()

`virtual ~BSeparatorItem();`

* * *

### Member Functions

#### Archive

`virtual status_t Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

#### SetEnabled()

`virtual void SetEnabled(bool state);`

#### GetContentSize()

`virtual void GetContentSize(float* width,  
float* height);`

#### Draw()

`virtual void Draw();`

* * *

### Static Functions

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* data);`

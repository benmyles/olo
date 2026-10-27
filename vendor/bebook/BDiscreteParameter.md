<!-- Classes And Methods - The Media Kit - BDiscreteParameter -->

## BDiscreteParameter

| Derived From:| [`BParameter`](BParameter.md "BParameter")  
---|---  
Mix-in Classes:| –  
Declared In:| `media/ParameterWeb.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BDiscreteParameter_Overview.md "BDiscreteParameter")

  * [Constructor and Destructor](BDiscreteParameter.md#BDiscreteParameter_ConstructorDestructor)
  * [Member Functions](BDiscreteParameter.md#BDiscreteParameter_MemberFunctions)

  
  
* * *

### Constructor and Destructor

You never create or delete a `BDiscreteParameter` object yourself. Instead, call the [`BParameterGroup::MakeDiscreteParameter()`](BParameterGroup.md#BParameterGroup_MakeDiscreteParameter "MakeDiscreteParameter\(\)") function to create a `BDiscreteParameter` within the desired [`BParameterGroup`](BParameterGroup.md "BParameterGroup").

* * *

### Member Functions

#### AddItem()

`status_t AddItem(int32 value,  
const char* name);`

Adds a new `name`/`value` pair to the list of possible values the control can take. Whenever the selection named name is chosen, the user interface application will send a value change message to the control with the specified value.

Return Code| Description  
---|---  
`B_OK`.| The item was added successfully.  
`B_NO_MEMORY`| Not enough memory to add the item to the list.  
  
#### CountItems()

`int32 CountItems();`

Returns the number of discrete values the control can take.

####  ItemNameAt(), ItemValueAt()

`const char* CountItems(int32 value);``int32 ItemValueAt(int32 value);`

These functions return the name and value of the item at the specified index, where index ranges from 0 to [`CountItems()`](BDiscreteParameter.md#BDiscreteParameter_CountItems "CountItems\(\)")-1.

Note

![Note](./images/admonitions/Info_32.png)

Selection values aren't necessarily in any sort of order, nor are they necessarily unique.

#### MakeEmpty()

`void MakeEmpty();`

Removes all items from the `BDiscreteParameter`.

####  MakeItemsFromInputs(), MakeItemsFromOutputs()

`void MakeItemsFromInputs();``void MakeItemsFromOutputs();`

These functions add all inputs or outputs as selections, where the input or output's name is used as the item's name, and the input or output's index number is used as the value of the item.

These shortcuts let you easily create the item lists for MUX-like controls, which allow the user to choose one of a number of inputs or outputs.

Note

![Note](./images/admonitions/Info_32.png)

It's important to note that the correct inputs and outputs must already be connected to the parameter when you call either of these functions, otherwise no items will be added, since the inputs and outputs aren't there.

#### ValueType()

`virtual type_code ValueType();`

Returns `B_INT32_TYPE`, which is the data type a `BDiscreteParameter`'s value is maintained in.

<!-- Classes And Methods - The Media Kit - BParameterWeb -->

## BParameterWeb

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `media/ParameterWeb.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BParameterWeb_Overview.md "BParameterWeb")

  * [Constructor and Destructor](BParameterWeb.md#BParameterWeb_ConstructorDestructor)
  * [Member Functions](BParameterWeb.md#BParameterWeb_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BParameterWeb()

`BParameterWeb();`

The `BParameterWeb` constructor. You'll usually create one `BParameterWeb` object per [`BControllable`](BControllable.md "BControllable") node; to attach a `BParameterWeb` to a [`BControllable`](BControllable.md "BControllable") node, you should call [`BControllable::SetParameterWeb()`](BControllable.md#BControllable_SetParameterWeb "SetParameterWeb\(\), Web\(\)").

#### ~BParameterWeb()

`~BParameterWeb();`

Once you've called [`BControllable::SetParameterWeb()`](BControllable.md#BControllable_SetParameterWeb "SetParameterWeb\(\), Web\(\)"), the node takes responsibility for the parameter web object and you shouldn't delete it.

If you don't call [`BControllable::SetParameterWeb()`](BControllable.md#BControllable_SetParameterWeb "SetParameterWeb\(\), Web\(\)"), then delete the `BParameterWeb` object when you're done with it.

* * *

### Member Functions

#### CountGroups()

`int32 CountGroups();`

Returns the number of [`BParameterGroup`](BParameterGroup.md "BParameterGroup") objects that are currently attached to the `BParameterWeb`.

#### CountParameters()

`int32 CountParameters();`

Returns the number of [`BParameter`](BParameter.md "BParameter")s in the entire web, including those in all [`BParameterGroup`](BParameterGroup.md "BParameterGroup")s attached to it.

#### GroupAt()

`BParameterGroup* GroupAt(int32 index);`

Returns the [`BParameterGroup`](BParameterGroup.md "BParameterGroup") located at the specified `index` within the list of groups contained by the `BParameterWeb`.

The first group is numbered 0, so the maximum legal value for `index` is [`CountGroups()`](BParameterGroup.md#BParameterGroup_CountGroups "CountGroups\(\)")-1. If the specified `index` is outside that range, `NULL` is returned.

#### MakeGroup()

`BParameterGroup* MakeGroup(const char* name);`

Creates a new [`BParameterGroup`](BParameterGroup.md "BParameterGroup") object to be used for grouping parameters within the `BParameterWeb`, and attaches it to the `BParameterWeb`.

All [`BParameter`](BParameter.md "BParameter")s created in the group will belong to that group, and, by recursion, to the web itself.

Note

![Note](./images/admonitions/Info_32.png)

You can nest [`BParameterGroup`](BParameterGroup.md "BParameterGroup")s if you want to; however, [`BParameter`](BParameter.md "BParameter")s can't be shared among multiple groups.

#### Node()

`media_node Node();`

Returns the [media_node](TheMediaKit_DefinedTypes.md#media_node "media_node") for the [`BControllable`](BControllable.md "BControllable") node that owns this `BParameterWeb` object.

If the `BParameterWeb` hasn't been attached to a [`BControllable`](BControllable.md "BControllable") node yet, media_node::`null` is returned.

#### ParameterAt()

`BParameter* ParameterAt(int32 index);`

Returns the [`BParameter`](BParameter.md "BParameter") at the specified `index` within the entire `BParameterWeb`, including those in all attached groups. The first parameter is numbered 0, so the maximum legal value for `index` is [`CountParameters()`](BParameterGroup.md#BParameterGroup_CountParameters "CountParameters\(\)")-1. If the specified `index` is outside that range, `NULL` is returned.

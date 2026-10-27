<!-- Classes And Methods - The Interface Kit - BShapeIterator -->

## BShapeIterator  
  
| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/Shape.h`  
Library:| `libbe.so`  
Allocation:| Constructor only  
[Class Overview](BShapeIterator_Overview.md "BShapeIterator")

  * [Constructor and Destructor](BShapeIterator.md#BShape_Iterator_ConstructorDestructor)
  * [Member Functions](BShapeIterator.md#BShape_Iterator_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BShapeIterator()

`BShapeIterator();`

Constructor

#### BShapeIterator()

`~BShapeIterator();`

Destructor

* * *

### Member Functions

#### Iterate()

`status_t Iterate([`BShape`](BShape.md "BShape")* shape);`

`Iterate()` iterates over each command that comprises the shape, in order, calling the `Iterate…()` function that corresponds to each command.

Return Code| Description  
---|---  
`B_OK`.| No error.  
Other errors.| None are defined at this time, but you should always check for errors returned by this function.  
  
####  IterateBezierTo(), IterateClose(), IterateLineTo(), IterateMoveTo()

`virtual status_t IterateBezierTo(int32 bezierCount,  
[`BPoint`](BPoint.md "BPoint")* bezierPoints);``virtual status_t IterateClose();``virtual status_t IterateLineTo(int32 lineCount,  
[`BPoint`](BPoint.md "BPoint")* linePoints);``virtual status_t IterateMoveTo([`BPoint`](BPoint.md "BPoint")* linePoints);`

These functions are called by [`Iterate()`](BShapeIterator.md#BShape_Iterator_Iterate "Iterate\(\)") to process the shape commands. Your `BShapeIterator`-derived class must implement these four functions.

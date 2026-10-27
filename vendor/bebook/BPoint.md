<!-- Classes And Methods - The Interface Kit - BPoint -->

## BPoint

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/Point.h`  
Library:| `libbe.so`  
Allocation:| Constructor or on the stack, typically the latter.  
[Class Overview](BPoint_Overview.md "BPoint")

  * [Data Members](BPoint.md#BPoint_DataMembers)
  * [Constructor and Destructor](BPoint.md#BPoint_ConstructorDestructor)
  * [Member Functions](BPoint.md#BPoint_MemberFunctions)
  * [Operators](BPoint.md#BPoint_Operators)
  * [Global Objects](BPoint.md#BPoint_GlobalObjects)

  
  
* * *

### Data Members

#### float x

The coordinate value measured horizontally along the x-axis.

#### float y

The coordinate value measured vertically along the y-axis.

* * *

### Constructor and Destructor

#### BPoint()

`inline BPoint(float x,  
float y);` `inline BPoint(const BPoint& point);` `inline BPoint();`

Creates a new `BPoint` object that corresponds to the point (`x`, `y`), or `BPoint` that's copied from point. If no coordinate values are assigned, the `BPoint`'s location is indeterminate.

See also: [`Set()`](BPoint.md#BPoint_Set "Set\(\)"), the assignment operator

* * *

### Member Functions

#### ConstrainTo()

`void ConstrainTo([`BRect`](BRect.md "BRect") rect);`

Ensures that the `BPoint` lies within `rect`. If it's already contained in the rectangle, the `BPoint` is unchanged; otherwise, it's moved to the `rect`'s nearest edge.

See also: [`BRect::Contains()`](BRect.md#BRect_Contains "Contains\(\), Intersects\(\)")

#### PrintToStream()

`void PrintToStream() const;`

Prints the `BPoint`'s coordinates to standard output in the form:
    
    
    "BPoint(x, y)"
    

#### Set()

`inline void Set(float x,  
float y);`

Sets the `BPoint`'s x and y coordinates.

* * *

### Operators

#### = (assignment)

`inline BPoint& operator =(const BPoint& from);`

Copies from's coordinate data into the left-side object.

####  == (equality), != (inequality)

`bool operator ==(BPoint) const;``bool operator !=(BPoint) const;`

`==` returns `true` if the two objects' point exactly coincide.

`!=` returns `true` if the two objects' points don't coincide.

####  \+ (addition), += (addition and assignment)

`BPoint operator +(const BPoint& ) const;``BPoint operator +=(const BPoint& ) const;`

`+` creates and returns a new `BPoint` that adds the two operands together. The new object's x coordinate is the sum of the operands' x values; its y value is the sum of the operands' y values.

`+=` adds the operands together and stores the result in the left operand.

####  \- (subtraction), -= (subtraction and assignment)

`BPoint operator -(const BPoint& ) const;``BPoint operator -=(const BPoint& ) const;`

`-` creates and returns a new `BPoint` that subtracts the right operand from the left. The new object's x coordinate is the difference between the operands' x values; its y value is the difference between the operands' y values.

`-=` performs the subtraction and stores the result in the left operand.

* * *

### Global Objects

#### B_ORIGIN

`BPoint` object that represents (0.0, 0.0).

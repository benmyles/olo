<!-- Classes And Methods - The Interface Kit - BRegion -->

## BRegion

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/Region.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BRegion_Overview.md "BRegion")

  * [Constructor and Destructor](BRegion.md#BRegion_ConstructorDestructor)
  * [Member Functions](BRegion.md#BRegion_MemberFunctions)
  * [Operators](BRegion.md#BRegion_Operators)

  
  
* * *

### Constructor and Destructor

#### BRegion()

`BRegion (const BRegion& region);` `BRegion (const BRect& rect);` `BRegion ();`

Initializes the `BRegion` object to be a copy of `region`, or to be a rectangular region containing the space indicated by `rect`. If no argument is supplied, the `BRegion` is empty and [`Frame()`](BRegion.md#BRegion_Frame "Frame\(\)") returns and invalid [`BRect`](BRect.md "BRect").

#### ~BRegion()

`virtual ~BRegion();`

Frees the memory that was allocated to hold data describing the region.

* * *

### Member Functions

#### Contains()

`bool Contains([`BPoint`](BPoint.md "BPoint") point) const;`

Returns `true` if point lies within the region, and `false` if not.

#### Frame()

`[`BRect`](BRect.md "BRect") Frame() const;`

Returns the smallest rectangle that encloses all the rectangles in the region.

If the region is empty, the returned object will be invalid. Check the return thus:
    
    
    rect = region.Frame();
    if (rect.IsValid())
       /* okay */
    else
       /* nope */

#### Intersects()

`bool Intersects([`BRect`](BRect.md "BRect") rect) const;`

Returns `true` if the `BRegion` has any area in common with `rect`, and `false` if not.

#### MakeEmpty()

`void MakeEmpty();`

Empties the `BRegion` and invalidates the object's [`Frame()`](BRegion.md#BRegion_Frame "Frame\(\)") rectangle.

#### OffsetBy()

`void OffsetBy(int32 horizontal,  
int32 vertical);`

Offsets all rectangles in the region by `horizontal` and `vertical` deltas.

#### PrintToStream()

`void PrintToStream() const;`

Prints (to standard out) each rectangle in the `BRegion` in the form:
    
    
    "BRect(_left_ , _top_ , _right_ , _bottom_)"
    

The first string that's printed is the object's frame rectangle. Each subsequent string describes a single rectangle in the `BRegion`.

####  RectAt(), CountRects()

`[`BRect`](BRect.md "BRect") RectAt(int32 index);``int32 CountRects();`

`CountRects()` returns the total number of rectangles in the `BRegion`; `RectAt()` returns the rectangle at a particular `index`. The order of the rectangles is unimportant.

####  Set(), Include(), Exclude(), IntersectWith()

`void Set([`BRect`](BRect.md "BRect") rect);`

`void Include([`BRect`](BRect.md "BRect") rect);` `void Include(const BRegion* region);`

`void Exclude([`BRect`](BRect.md "BRect") rect);` `void Exclude(const BRegion* region);`

`void IntersectWith(const BRegion* region);`

`Set()` modifies the `BRegion` so that it only contains `rect`.

`Include()` modifies the region to include (copies of) the rectangles contained in `rect` or `region`.

`Exclude()` modifies the region to exclude all rectangles contained in `rect` or `region`..

`IntersectWith()` modifies the region so that it includes only those areas that it has in common with another region.

* * *

### Operators

#### = (assignment)

`BRegion& operator =(const BRegion& );`

Sets the left region to be an independent copy of the right region:
    
    
    BRegion left_region = right_region;

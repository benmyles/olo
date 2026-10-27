<!-- Classes And Methods - The Interface Kit - BPolygon -->

## BPolygon

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/Polygon.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BPolygon_Overview.md "BPolygon")

  * [Constructor and Destructor](BPolygon.md#BPolygon_ConstructorDestructor)
  * [Member Functions](BPolygon.md#BPolygon_MemberFunctions)
  * [Operators](BPolygon.md#BPolygon_Operators)

  
  
* * *

### Constructor and Destructor

#### BPolygon()

`BPolygon(const [`BPoint`](BPoint.md "BPoint")* pointList,  
int32 numPoints);` `BPolygon(const BPolygon* polygon);` `BPolygon();`

Initializes the `BPolygon` by copying `numPoints` from `pointList`, or by copying the list of points from another polygon. If one polygon is constructed from another, the original and the copy won't share any data; independent memory is allocated for the copy to hold a duplicate list of points.

If a `BPolygon` is constructed without a point list, points must be set with the [`AddPoints()`](BPolygon.md#BPolygon_AddPoints "AddPoints\(\)") function.

#### ~BPolygon()

`virtual ~BPolygon();`

Frees all the memory allocated to hold the list of points.

* * *

### Member Functions

#### AddPoints()

`void AddPoints(const [`BPoint`](BPoint.md "BPoint")* pointList,  
int32 numPoints);`

Appends `numPoints` from `pointList` to the list of points that already define the polygon.

#### CountPoints()

`int32 CountPoints() const;`

Returns the number of points that define the polygon.

#### Frame()

`[`BRect`](BRect.md "BRect") Frame() const;`

Returns the polygon's frame rectangle—the smallest rectangle that encloses the entire polygon.

#### MapTo()

`void MapTo([`BRect`](BRect.md "BRect") source,  
[`BRect`](BRect.md "BRect") destination);`

Modifies the polygon so that it fits the `destination` rectangle exactly as it originally fit the `source` rectangle. Each vertex of the polygon is modified so that it has the same proportional position relative to the sides of the destination rectangle as it originally had to the sides of the source rectangle.

The polygon doesn't have to be contained in either rectangle. However, to modify a polygon so that it's exactly inscribed in the destination rectangle, you should pass its frame rectangle as the source:
    
    
    [BRect](BRect.md "BRect") frame = myPolygon->Frame();
    myPolygon->MapTo(frame, anotherRect);

#### PrintToStream()

`void PrintToStream() const;`

Prints the `BPolygon`'s point list to the standard output stream (stdout). The [`BPoint`](BPoint.md "BPoint") version of this function is called to report each point as a string in the form
    
    
    "[BPoint](BPoint.md "BPoint")(x, y)"
    

where x and y stand for the coordinate values of the point in question.

See also: [`BPoint::PrintToStream()`](BPoint.md#BPoint_PrintToStream "PrintToStream\(\)")

* * *

### Operators

#### = (assignment)

`BPolygon& operator =(const BPolygon& );`

Copies the point list of one `BPolygon` object and assigns it to another `BPolygon`. After the assignment, the two objects describe the same polygon, but are independent of each other. Destroying one of the objects won't affect the other.

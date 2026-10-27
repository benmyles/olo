<!-- System Overview - The Interface Kit - BShapeIterator -->

## BShapeIterator

Applications can create classes derived from [`BShapeIterator`](BShapeIterator.md "BShapeIterator") to iterate over the components of a [`BShape`](BShape.md "BShape") object. There are four virtual functions that the application must implement to process the four graphics commands described in the [`BShape`](BShape.md "BShape") documentation: [`MoveTo()`](BShape.md#BShape_MoveTo "MoveTo\(\)"), [`LineTo()`](BShape.md#BShape_LineTo "LineTo\(\)"), [`BezierTo()`](BShape.md#BShape_BezierTo "BezierTo\(\)"), and [`Close()`](BShape.md#BShape_Close "Close\(\)").

These functions can be implemented to do anything they like with the commands (rendering them to the screen comes to mind as one possibility).

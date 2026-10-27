<!-- System Overview - The Interface Kit - BPolygon -->

## BPolygon

A [`BPolygon`](BPolygon.md "BPolygon") object represents a polygon—a closed, many-sided figure that describes an area within a two-dimensional coordinate system. It differs from a [`BRect`](BRect.md "BRect") object in that it can have any number of sides and the sides don't have to be aligned with the coordinate axes.

A [`BPolygon`](BPolygon.md "BPolygon") is defined as a series of connected points. Each point is a potential vertex in the polygon. An outline of the polygon could be constructed by tracing a straight line from the first point to the second, from the second point to the third, and so on through the whole series, then by connecting the first and last points if they're not identical.

The [`BView`](BView.md "BView") functions that draw a polygon—[`StrokePolygon()`](BView.md#BView_StrokePolygon) and [`FillPolygon()`](BView.md#BView_FillPolygon "StrokePolygon\(\), FillPolygon\(\)")—take [`BPolygon`](BPolygon.md "BPolygon") objects as arguments. [`StrokePolygon()`](BView.md#BView_StrokePolygon) offers the option of leaving the polygon open—of not stroking the line that connects the first and last points in the list. The polygon therefore won't look like a polygon, but like an chain of lines fastened at their endpoints.

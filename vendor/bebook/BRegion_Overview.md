<!-- System Overview - The Interface Kit - BRegion -->

## BRegion

A [`BRegion`](BRegion.md "BRegion") object comprises a set of rectangles in a two-dimensional coordinate system. The rectangles needn't overlap; a single [`BRegion`](BRegion.md "BRegion") can contain a little rectangle way down there, a big one over yonder, and another that lives somewhere to the left of Baden Baden.

You define and modify the contents of a region through to [`Set()`](BRegion.md#BRegion_Set "Set\(\), Include\(\), Exclude\(\), IntersectWith\(\)"), [`Include()`](BRegion.md#BRegion_Include), [`Exclude()`](BRegion.md#BRegion_Exclude), and [`IntersectWith()`](BRegion.md#BRegion_IntersectWith). You can examine a region's rectangle's through [`RectAt()`](BRegion.md#BRegion_RectAt "RectAt\(\), CountRects\(\)"), but note that the object coalesces rectangles when it can, so the rectangles that you retrieve may not match the ones that you passed in.

Warning

![Warning](./images/admonitions/Stop_32.png)

The [`BRegion`](BRegion.md "BRegion") class is designed to be used with coordinates with integral values only; if any non-integral values are used in the [`BRegion`](BRegion.md "BRegion"), functions in the class may behave unpredictably.

[`BRegion`](BRegion.md "BRegion") objects can be allocated on the stack.

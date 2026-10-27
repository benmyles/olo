<!-- System Overview - The Interface Kit - BBitmap -->

## BBitmap

A [`BBitmap`](BBitmap.md "BBitmap") describes a rectangular image as a two-dimensional array of pixel data (or bitmap). The [`BBitmap`](BBitmap.md "BBitmap") class lets you create a bitmap by specifying raw pixel data (through the [`Bits()`](BBitmap.md#BBitmap_Bits "Bits\(\)") and [`SetBits()`](BBitmap.md#BBitmap_SetBits "SetBits\(\)") functions), or you can add a [`BView`](BView.md "BView") to your [`BBitmap`](BBitmap.md "BBitmap") and use the view's drawing operations ( [`FillRect()`](BView.md#BView_FillRect), [`StrokeLine()`](BView.md#BView_StrokeLine "StrokeLine\(\)"), etc) to draw into the [`BBitmap`](BBitmap.md "BBitmap") object (see "[Using a View to Draw into a Bitmap](BBitmap_Overview.md#BBitmap_Overview_Using_A_View "Using a View to Draw into a Bitmap")", below).

The [`BBitmap`](BBitmap.md "BBitmap") class doesn't provide a way to actually display bitmap data. Displaying a bitmap is the task of [`BView`](BView.md "BView") functions such as [`DrawBitmap()`](BView.md#BView_DrawBitmap "DrawBitmap\(\), DrawBitmapAsync\(\)").

* * *

### Bitmap Data

A bitmap records the color values of every pixel within a rectangular area. The data is specified in rows, beginning with the top row of pixels in the image and working downward to the bottom row. Each row is aligned on a long word boundary.

When you construct a [`BBitmap`](BBitmap.md "BBitmap") object, you give it a bounds rectangle (integer coordinates only!) and a color space. For example, this code
    
    
    BBitmap* image = new BBitmap(BRect(0.0, 0.0, 79.0, 39.0),
                                 B_CMAP8);

constructs a 40 x 80 bitmap of 8-bit color data.

The data in a [`BBitmap`](BBitmap.md "BBitmap") object isn't initialized—a [`BBitmap`](BBitmap.md "BBitmap") has no default background color. When you set the bitmap's data (however you set it) you must "paint" every pixel within the bitmap's bounds rectangle.

* * *

### Using a View to Draw into a Bitmap

If you're going to use a view to draw into your bitmap, you must tell the [`BBitmap`](BBitmap.md "BBitmap") constructor by setting the third argument to `true`:
    
    
    BBitmap *image = new BBitmap(BRect(...), B_CMAP8, true);

You then add the view to the bitmap (you don't have to do anything special when constructing the [`BView`](BView.md "BView")):
    
    
    bitmap->AddChild(view);

When the view draws, the drawing operations are rendered into the bitmap. Note that you must explicitly tell the [`BView`](BView.md "BView")): to draw—the [`BView`](BView.md "BView")s that you use to draw into a [`BBitmap`](BBitmap.md "BBitmap") aren't part of the user interface, so they won't receive user event messages. When you're done drawing, you should call [`BView`](BView.md "BView")'s [`Sync()`](BView.md#BView_Sync) function to make sure the drawing has all been performed. If the bitmap that you've created is static—if it doesn't need to change after you've drawn into it—you can throw away the [`BView`](BView.md "BView") that you used create the bitmap data.

A [`BBitmap`](BBitmap.md "BBitmap") can contain more than one [`BView`](BView.md "BView")—it can act as the root of an entire view hierarchy. The [`BBitmap`](BBitmap.md "BBitmap") class defines a number of [`BWindow`](BWindow.md "BWindow")-like functions—[`AddChild()`](BBitmap.md#BBitmap_AddChild "AddChild\(\)"), [`FindView()`](BBitmap.md#BBitmap_FindView "FindView\(\)"), [`ChildAt()`](BBitmap.md#BBitmap_ChildAt "ChildAt\(\), CountChildren\(\)"), and so on—to help you create and manage the hierarchy.

* * *

### Transparency

Color bitmaps can have transparent pixels. When the bitmap is imaged in a drawing mode other than `B_OP_COPY`, its transparent pixels won't be transferred to the destination view. The destination image will show through wherever the bitmap is transparent.

To introduce transparency into a `B_CMAP8` bitmap, a pixel can be assigned one of the following values, as appropriate for the bitmap's color space.

Constant| Description  
---|---  
`B_TRANSPARENT_MAGIC_CMAP8`| 8-bit indexed color transparent pixel.  
`B_TRANSPARENT_MAGIC_RGBA15`| 15-bit transparent pixel.  
`B_TRANSPARENT_MAGIC_RGBA15_BIG`| 15-bit transparent pixel, big-endian.  
`B_TRANSPARENT_MAGIC_RGBA32`| 32-bit transparent pixel.  
`B_TRANSPARENT_MAGIC_RGBA32_BIG`| 32-bit transparent pixel, big-endian.  
  
Opaque pixels should have an alpha value of 255 for 8-bit alpha channels or 1 for 1-bit alpha channels; values of 0 indicate 100% transparent pixels. Values in between (for 8-bit alpha channels) represent varying degrees of transparency.

Transparency is covered in more detail under "[Drawing Modes](TheInterfaceKit_Drawing.md#TheInterfaceKit_Drawing_Drawing_Modes "Drawing Modes")".

See also: [`system_colors()`](TheInterfaceKit_Functions.md#system_colors "system_colors\(\)")

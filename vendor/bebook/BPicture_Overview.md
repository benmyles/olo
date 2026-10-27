<!-- System Overview - The Interface Kit - BPicture -->

## BPicture  
  
A [`BPicture`](BPicture.md "BPicture") object represents a set of drawing instructions that are executed when the object is passed to [`BView`](BView.md "BView")'s [`DrawPicture()`](BView.md#BView_DrawPicture "DrawPicture\(\), DrawPictureAsync\(\)") function. Because it contains drawing instructions rather than an actual image, a [`BPicture`](BPicture.md "BPicture")BPicture (unlike a [`BBitmap`](BBitmap.md "BBitmap")) is independent of the resolution of the display device.

* * *

### Recording a Picture

To start recording into a [`BPicture`](BPicture.md "BPicture"), you pass a [`BPicture`](BPicture.md "BPicture") object to [`BView::BeginPicture()`](BView.md#BView_BeginPicture "BeginPicture\(\) , AppendToPicture\(\) , EndPicture\(\)"). All drawing instructions that are executed by the [`BView`](BView.md "BView") are recorded into the [`BPicture`](BPicture.md "BPicture") object. When you're done recording, you call [`BView::EndPicture()`](BView.md#BView_EndPicture), which passes back a pointer to the recorded object. For example:
    
    
    BPicture *myPict;
    someView->BeginPicture(new BPicture);
    /* drawing code goes here*/
    myPict = someView->EndPicture();

Only drawing that the [`BView`](BView.md "BView") does is recorded; drawing done by children and other views attached to the window is ignored, as is everything except drawing code.

Drawing instructions that are captured between [`BeginPicture()`](BView.md#BView_BeginPicture "BeginPicture\(\) , AppendToPicture\(\) , EndPicture\(\)") and [`EndPicture()`](BView.md#BView_EndPicture) are not renedered on-screen; ignored instructions may be rendered if they draw into the visible region of an on-screen window.

Any picture data in the [`BPicture`](BPicture.md "BPicture") passed to [`BeginPicture()`](BView.md#BView_BeginPicture "BeginPicture\(\) , AppendToPicture\(\) , EndPicture\(\)") is cleared; if you'd instead like to append to the [`BPicture`](BPicture.md "BPicture"), begin the picture recording with [`AppendPicture()`](BView.md#BView_AppendToPicture) instead. As with [`BeginPicture()`](BView.md#BView_BeginPicture "BeginPicture\(\) , AppendToPicture\(\) , EndPicture\(\)"), each [`AppendToPicture()`](BView.md#BView_AppendToPicture) must have a corresponding EndPicture().

* * *

### The Picture Definition

The picture captures everything that affects the image that's drawn. It takes a snapshot of the [`BView`](BView.md "BView")'s graphics state—the pen size, high and low colors, font size, and so on—when [`BeginPicture()`](BView.md#BView_BeginPicture "BeginPicture\(\) , AppendToPicture\(\) , EndPicture\(\)") is called. It then captures all subsequent modifications to those parameters, such as calls to [`MovePenTo()`](BView.md#BView_MovePenTo), [`SetLowColor()`](BView.md#BView_SetLowColor), an [`SetFontSize()`](BView.md#BView_SetFontSize "SetFontSize\(\)"). The recorded graphics state is used when the picture is drawn (through [`BView::DrawPicture()`](BView.md#BView_DrawPicture "DrawPicture\(\), DrawPictureAsync\(\)")).

The picture records all primitive drawing instruction ( [`DrawBitmap()`](BView.md#BView_DrawBitmap "DrawBitmap\(\), DrawBitmapAsync\(\)"), [`StrokeEllipse()`](BView.md#BView_StrokeEllipse "StrokeEllipse\(\), FillEllipse\(\), StrokeArc\(\), FillArc\(\)"), [`FillRect()`](BView.md#BView_FillRect), etc.) and will even record calls to [`DrawPicture()`](BView.md#BView_DrawPicture "DrawPicture\(\), DrawPictureAsync\(\)").

The picture makes its own copy of any data that's passed during the recording session, including bitmaps passed to [`DrawBitmap()`](BView.md#BView_DrawBitmap "DrawBitmap\(\), DrawBitmapAsync\(\)") and picture data passed to [`DrawPicture()`](BView.md#BView_DrawPicture "DrawPicture\(\), DrawPictureAsync\(\)").

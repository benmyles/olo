<!-- Classes And Methods - The Interface Kit - BPrintJob -->

## BPrintJob

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/PrintJob.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BPrintJob_Overview.md "BPrintJob")

  * [Constructor and Destructor](BPrintJob.md#BPrintJob_ConstructorDestructor)
  * [Member Functions](BPrintJob.md#BPrintJob_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BPrintJob()

`BPrintJob(const char* name);`

Initializes the `BPrintJob` object and assigns the job a `name`. The Print Server isn't contacted until [`ConfigPage()`](BPrintJob.md#BPrintJob_ConfigPage "ConfigPage\(\), ConfigJob\(\)") or [`ConfigJob()`](BPrintJob.md#BPrintJob_ConfigJob) is called. The spool file isn't created until [`BeginJob()`](BPrintJob.md#BPrintJob_BeginJob "BeginJob\(\)") prepares for the production of pages.

#### ~BPrintJob()

`virtual ~BPrintJob();`

Frees all memory allocated by the object.

* * *

### Member Functions

#### BeginJob()

`void BeginJob();`

Opens a spool file for the job and prepares for the production of a series of pages. Call this function only once per printing session—just after initializing the job and just before drawing the first page.

See also: [`CommitJob()`](BPrintJob.md#BPrintJob_CommitJob "CommitJob\(\)"), "[The Spool File](BPrintJob_Overview.md#BPrintJob_Overview_TheSpoolFile "The Spool File")" in the class overview

#### CancelJob()

`void CancelJob();`

Cancels the print job programmatically and gets rid of the spool file. The job cannot be restarted; you must destroy the `BPrintJob` object. Create a new object to renew printing.

#### CanContinue()

`bool CanContinue();`

Returns `true` if there's no impediment to continuing with the print job, and `false` if the user has canceled the job, the spool file has grown too big, or something else has happened to terminate printing. It's a good idea to liberally sprinkle `CanContinue()` queries throughout your printing code to make sure that the work you're about to do won't be wasted.

See also: "[Cancellation](BPrintJob_Overview.md#BPrintJob_Overview_Cancellation "Cancellation")" in the class overview

#### CommitJob()

`void CommitJob();`

Commits all spooled pages to the printer. This ends the print job; when `CommitJob()` returns, the `BPrintJob` object can be deleted. `CommitJob()` can be called only once per job.

See also: [`BeginJob()`](BPrintJob.md#BPrintJob_BeginJob "BeginJob\(\)"), "[The Spool File](BPrintJob_Overview.md#BPrintJob_Overview_TheSpoolFile "The Spool File")" in the class overview

####  ConfigPage(), ConfigJob()

`status_t ConfigPage();``status_t ConfigJob();`

These functions contact the Print Server and have the server interact with the user to lay out the document on a page (in the case of `ConfigPage()`) or to define a print job (in the case of `ConfigJob()`). The page layout includes such things as the orientation of the image (portrait or landscape), the dimensions of the paper on which the document will be printed, and the size of the margins. The job definition includes such things as which pages are to be printed and the number of copies.

Both functions record the user's choices in a [`BMessage`](BMessage.md "BMessage") object that [`Settings()`](BPrintJob.md#BPrintJob_Settings) returns.

If [`SetSettings()`](BPrintJob.md#BPrintJob_SetSettings "SetSettings\(\), Settings\(\), IsSettingsMessageValid\(\)") has been called to establish a default configuration for the page layout or the job, these functions will pass it to the Print Server so the server can present it to the user. Otherwise, the server will choose a default configuration to show the user.

These two functions return status_t error codes, despite having return values that are declared int32. They return `B_ERROR` if they have trouble communicating with the server or if the job can't be established for any reason. They return `B_OK` if all goes well.

See also: [`SetSettings()`](BPrintJob.md#BPrintJob_SetSettings "SetSettings\(\), Settings\(\), IsSettingsMessageValid\(\)"), "[Setting Up the Page Layout](BPrintJob_Overview.md#BPrintJob_Overview_SettingUpThePageLayout "Setting Up the Page Layout")" and "[Setting Up a Print Job](BPrintJob_Overview.md#BPrintJob_Overview_SettingUpAPrintJob "Setting Up a Print Job")" in the class overview

####  DrawView() , SpoolPage()

`virtual void DrawView([`BView`](BView.md "BView")* view,  
[`BRect`](BRect.md "BRect") rect,  
[`BPoint`](BPoint.md "BPoint") point);``void SpoolPage();`

`DrawView()` calls upon a `view` to draw the `rect` portion of its display at the location specified by `point` on the page. As a result, the `view`'s [`Draw()`](BView.md#BView_Draw "Draw\(\)") function will be called with `rect` passed as the update rectangle. The rectangle should be stated in the [`BView`](BView.md "BView")'s coordinate system. The `point` should be stated in a coordinate system that has the origin at the top left corner of the printable rectangle. Together the `rect` and `point` should be fashioned so that the entire rectangle lies within the boundaries of the page's printable area.

The `view` must be attached to a window; that is, it must be known to the Application Server. However, when printing, a [`BView`](BView.md "BView") can be asked to draw portions of its display that are not visible on-screen. Its drawing is not limited by the clipping region, its bounds rectangle, or the frame rectangles of ancestor views.

Any number of [`BView`](BView.md "BView")s can draw on a page if they are subjects of separate `DrawView()` calls.

`DrawView()` renders the entire hierarchy of [`BView`](BView.md "BView")s attached to the specified [`BView`](BView.md "BView").

Note the following bugs (which will be fixed in subsequent releases):

  * BeOS 5 (and earlier) renders only the first two hierarchy levels correctly.

  * Hidden [`BView`](BView.md "BView")s (and views without `B_WILL_DRAW` specified) are rendered.

  * [`BScrollBar`](BScrollBar.md "BScrollBar") and derived [`BView`](BView.md "BView")s aren't rendered.

  * View background and overlay bitmaps are ignored; instead, the view color is printed.

After all views have drawn and the page is complete, `SpoolPage()` adds it to the spool file. `SpoolPage()` must be called once to terminate each page.

See also: [`PrintableRect()`](BPrintJob.md#BPrintJob_PrintableRect), [`BView::Draw()`](BView.md#BView_Draw "Draw\(\)"), "[Drawing on the Page](BPrintJob_Overview.md#BPrintJob_Overview_DrawingOnThePage "Drawing on the Page")" in the class overview

####  FirstPage(), LastPage()

`int32 FirstPage();``int32 LastPage();`

These functions return the first and last pages that should be printed as part of the current job. If the pages are not set (for example, if the current job has been canceled), `FirstPage()` returns 0 and `LastPage()` returns a very large number (`LONG_MAX`).

#### GetResolution()

`void GetResolution(int32* xdpi,  
int32* ydpi);`

Fills the `xdpi` and `ydpi` variables with the X and Y resolution of the printer, in dots per inch.

####  PaperRect() , PrintableRect()

`[`BRect`](BRect.md "BRect") PaperRect();``[`BRect`](BRect.md "BRect") PrintableRect();`

`PaperRect()` returns a rectangle that records the presumed size of the paper that the printer will use. Its `left` and `top` sides are at 0.0, so its `right` and `bottom` coordinates reflect the size of a sheet of paper. The size depends on choices made by the user when setting up the page layout.

`PrintableRect()` returns a rectangle that encloses the portion of a page where printing can appear. It's stated in the same coordinate system as the rectangle returned by `PaperRect()`, but excludes the margins around the edge of the paper. When drawing on the printed page, the `left` `top` corner of this rectangle is taken to be the coordinate origin, (0.0, 0.0).

The "[Drawing Coordinates](BPrintJob_Overview.md#BPrintJob_Overview_DrawingCoordinates "Drawing Coordinates")" section in the class overview illustrates these rectangles and their coordinate systems.

See also: [`DrawView()`](BPrintJob.md#BPrintJob_DrawView "DrawView\(\) , SpoolPage\(\)")

#### PrinterType()

`int32 PrinterType(void* type = NULL);`

Returns a code identifying whether the printer is color or black and white. The argument is currently not used.

The return value will be either `B_BW_PRINTER` or `B_COLOR_PRINTER`.

Note

![Note](./images/admonitions/Info_32.png)

In the current release of BeOS, this function always returns `B_COLOR_PRINTER`, since there's no printer driver API for determining the printer type yet.

If an error occurs, this function will return an appropriate error code (for example, if the Print Server isn't running).

####  SetSettings(), Settings(), IsSettingsMessageValid()

`void SetSettings([`BMessage`](BMessage.md "BMessage")* configuration);``[`BMessage`](BMessage.md "BMessage")* Settings();``void IsSettingsMessageValid([`BMessage`](BMessage.md "BMessage")* configuration) const;`

These functions set and return the group of parameters that define how a document should be printed. The parameters include some that capture the page layout of the document and some that define the current job. They're recorded in a [`BMessage`](BMessage.md "BMessage") object that can be regarded as a black box; the data in the message are interpreted by the Print Server and will be documented where the print driver API is documented.

Instead of looking in the `Settings()` [`BMessage`](BMessage.md "BMessage"), rely on `BPrintJob` functions to provide specific information about the layout and the print job. Currently, there are only two functions – [`FirstPage()`](BPrintJob.md#BPrintJob_FirstPage "FirstPage\(\), LastPage\(\)") and [`LastPage()`](BPrintJob.md#BPrintJob_LastPage), which return the first and last pages that need to be printed.

`Settings()` can be called to get the current configuration message, which can then be flattened and stored with the document. You can retrieve it later and pass it to `SetSettings()` to set initial configuration values the next time the document is printed, as discussed in the "[Setting Up the Page Layout](BPrintJob_Overview.md#BPrintJob_Overview_SettingUpThePageLayout "Setting Up the Page Layout")" and "[Setting Up a Print Job](BPrintJob_Overview.md#BPrintJob_Overview_SettingUpAPrintJob "Setting Up a Print Job")" sections of the class overview.

If the message passed to settings doesn't contain all the information needed to properly configure the print job, the printer add-on being used will either determine appropriate default values or present a configuration dialog box.

`SetSettings()` assumes ownership of the object it's passed. If your application needs to retain the object, pass a copy to `SetSettings()`:
    
    
    print_job_object.SetSettings(new [BMessage](BMessage.md "BMessage")(settings_message));

On the other hand, `Settings()` transfers ownership of the object it returns to the caller; you don't need to make a copy.

`IsMessageSettingsValid()` returns `true` if the specified message is a valid settings message; otherwise it returns `false`.

See also: [`ConfigPage()`](BPrintJob.md#BPrintJob_ConfigPage "ConfigPage\(\), ConfigJob\(\)")

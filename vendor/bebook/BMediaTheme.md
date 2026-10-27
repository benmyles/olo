<!-- Classes And Methods - The Media Kit - BMediaTheme -->

## BMediaTheme

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `media/MediaTheme.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BMediaTheme_Overview.md "BMediaTheme")

  * [Constructor and Destructor](BMediaTheme.md#BMediaTheme_ConstructorDestructor)
  * [Member Functions](BMediaTheme.md#BMediaTheme_MemberFunctions)
  * [Global C Functions](BMediaTheme.md#BMediaTheme_Functions)
  * [Constants](BMediaTheme.md#BMediaTheme_Constants)

  
  
* * *

### Constructor and Destructor

#### BMediaTheme()

##### protected

`BMediaTheme(const char* name,  
const char* info,  
const entry_ref* addOn = NULL,  
int32 themeID = 0);`

The `BMediaTheme` constructor initializes the theme so it can be used by the system or by an application. The user-readable theme name is passed as the `name` argument, and some longer description of the theme in `info`.

The `name` and `info` strings are copied; you can dispose of the original strings if you wish. Both strings should be human-readable, as they may be presented to the user in a user interface for selecting a theme to be used.

If the theme lives in an add-on on disk, specify the entry_ref of the add-on file in `addOn` and the ID assigned to the theme by its add-on in `themeID`.

#### ~BMediaTheme()

`virtual BMediaTheme();`

Disposes of memory used by the theme. If you create a theme, be sure the destructor frees any memory allocated by the theme.

* * *

### Member Functions

#### BackgroundBitmapFor()

`virtual [`BBitmap`](BBitmap.md "BBitmap")* BackgroundBitmapFor([bg_kind](BMediaTheme.md#Enums_bg_kind "Background Color and Bitmap Kinds") backgroundKind);`

Returns the [`BBitmap`](BBitmap.md "BBitmap") that should be used as the background for the specified type of user interface object. Possible values for `backgroundKind` are detailed in [bg_kind](BMediaTheme.md#Enums_bg_kind "Background Color and Bitmap Kinds").

#### BackgroundColorFor()

`virtual rgb_color BackgroundBitmapFor([bg_kind](BMediaTheme.md#Enums_bg_kind "Background Color and Bitmap Kinds") backgroundKind);`

Returns the [rgb_color](TheInterfaceKit_DefinedTypes.md#rgb_color "rgb_color") that should be used as the background for the specified type of user interface object. Possible values for `backgroundKind` are detailed in [bg_kind](BMediaTheme.md#Enums_bg_kind "Background Color and Bitmap Kinds").

#### ForegroundColorFor()

`virtual rgb_color BackgroundBitmapFor([fg_kind](BMediaTheme.md#Enums_fg_kind "Foreground Color Kinds") foregroundKind);`

Returns the [rgb_color](TheInterfaceKit_DefinedTypes.md#rgb_color "rgb_color") that should be used as the foreground for the specified type of user interface object. Possible values for foregroundKind are detailed in [fg_kind](BMediaTheme.md#Enums_fg_kind "Foreground Color Kinds").

#### GetRef()

`bool GetRef(entry_ref* outRef);`

Sets `outRef` to be a reference to the add-on file from which the theme was loaded and returns `true`. If the theme wasn't loaded from an add-on, this function returns `false`.

#### ID()

`int32 ID();`

Returns the theme's ID, as assigned by the add-on in which the theme resides. The ID is unique only within that add-on.

If the theme isn't in an add-on, 0 is returned.

#### Info()

`const char* Info();`

Returns the user-readable long description of the theme, as specified when the theme was created.

#### MakeControlFor()

`virtual [`BControl`](BControl.md "BControl")* MakeControlFor([`BParameter`](BParameter.md "BParameter")* parameter)  = 0;`

If you create your own theme, your implementation of this function should return an appropriate [`BControl`](BControl.md "BControl") to operate on the specified [`BParameter`](BParameter.md "BParameter").

Applications that want to use the theme to create the controls, but handle their layout themselves, can call this function for each parameter in the web, rather than rely upon [`ViewFor()`](BMediaTheme.md#BMediaTheme_ViewFor "ViewFor\(\)"). However, if an application uses `MakeControlFor()` to create individual controls rather than letting [`ViewFor()`](BMediaTheme.md#BMediaTheme_ViewFor "ViewFor\(\)") set up the entire view, the application assumes responsibility for setting the control's value in response to value change messages.

If you want the control to send a specific message when invoked, you should call [`BInvoker::SetMessage()`](BInvoker.md#BInvoker_SetMessage "SetMessage\(\), Message\(\), Command\(\)") on it.

#### MakeFallbackViewFor()

`static [`BControl`](BControl.md "BControl")* MakeFallbackViewFor([`BParameter`](BParameter.md "BParameter")* control);`

If you're implementing your own theme, and for whatever reason you want a [`BParameter`](BParameter.md "BParameter") to use the standard system theme's [`BControl`](BControl.md "BControl"), call `MakeFallbackViewFor()` to obtain that [`BControl`](BControl.md "BControl").

This can be used if your theme is only intended to augment certain controls, or if you receive a [`BParameter`](BParameter.md "BParameter") you don't know anything about.

Note

![Note](./images/admonitions/Info_32.png)

Your [`MakeControlFor()`](BMediaTheme.md#BMediaTheme_MakeControlFor "MakeControlFor\(\)") implementation should always call `MakeFallbackViewFor()` if it's asked to create a [`BControl`](BControl.md "BControl") for a [`BParameter`](BParameter.md "BParameter") it doesn't understand. This way, all [`BParameter`](BParameter.md "BParameter")s will get some form of user interface, even if your theme doesn't specifically know how to handle some of them. This will let your theme remain compatible with future versions of the BeOS.

#### MakeViewFor()

`virtual [`BView`](BView.md "BView")* MakeViewFor([`BParameterWeb`](BParameterWeb.md "BParameterWeb")* web,  
const [`BRect`](BRect.md "BRect")* hintRect = NULL)  = 0;`

Given the specified [`BParameterWeb`](BParameterWeb.md "BParameterWeb") and `hintRect`, your implementation of this function should construct a [`BView`](BView.md "BView") that contains the [`BControl`](BControl.md "BControl")s for manipulating the [`BParameter`](BParameter.md "BParameter")s in the web. The `hintRect` is an area the caller thinks is appropriate for you to fill with your [`BView`](BView.md "BView"); your theme should try to stay within that rectangle if possible.

The web returned by this function belongs to the theme; applications shouldn't delete it (a properly-written theme will automatically dispose of the web when the view is closed).

This function is called by [`ViewFor()`](BMediaTheme.md#BMediaTheme_ViewFor "ViewFor\(\)").

####  PreferredTheme(), SetPreferredTheme()

`static BMediaTheme* PreferredTheme();``static status_t SetPreferredTheme(BMediaTheme* newPreferredTheme);`

`PreferredTheme()` returns the current preferred theme; `SetPreferredTheme()` changes the preferred theme to the theme specified by `newPreferredTheme`.

The preferred theme is the theme that will be used for creating node controlling user interfaces if no specific theme is requested when the interface is created.

Return Code| Description  
---|---  
`B_OK`| The preferred theme was changed without error.  
`B_NOT_ALLOWED`| Access to the settings file denied.  
Other errors.| Directory and file I/O errors may occur while updating the settings file.  
  
See also: [`ViewFor()`](BMediaTheme.md#BMediaTheme_ViewFor "ViewFor\(\)"), [`MakeViewFor()`](BMediaTheme.md#BMediaTheme_MakeViewFor "MakeViewFor\(\)")

#### Name()

`const char* Name();`

Returns the user-readable name of the theme, as specified when the theme was created.

#### ViewFor()

`static [`BView`](BView.md "BView")* ViewFor([`BParameterWeb`](BParameterWeb.md "BParameterWeb")* web,  
const [`BRect`](BRect.md "BRect")* hintRect = NULL,  
BMediaTheme* usingTheme = NULL);`

Given a [`BParameterWeb`](BParameterWeb.md "BParameterWeb") web, typically returned by [`BMediaRoster::GetParameterWebFor`](BMediaRoster.md#BMediaRoster_GetParameterWebFor "GetParameterWebFor\(\)"), this function creates a [`BView`](BView.md "BView") suitable for adding to a [`BWindow`](BWindow.md "BWindow") to allow the user to configure the node the web describes, including a diagram indicating the data flow path through the node, and controls to let the user configure each control point.

Note

![Note](./images/admonitions/Info_32.png)

This function is the public interface for creating a view for configuring a node (the [`MakeViewFor()`](BMediaTheme.md#BMediaTheme_MakeViewFor "MakeViewFor\(\)") function is the hook you override if you're implementing your own theme).

The returned view is created using the theme specified by `usingTheme`; if this argument is `NULL`, the preferred theme is used. The `hintRect` parameter specifies the rectangle the view should try to occupy, and is passed to `MakeViewFor()`.

* * *

### Global C Functions

#### get_nth_theme()

`status_t get_theme_at(int32 n,  
const char** outName,  
int32* outID);`

This function is called after the theme add-on is loaded, to determine what themes are available in the add-on. It's called repeatedly, with successively-higher values of `n`, until it returns an error.

Your add-on's implementation of this function should set `outName` to point to the `n`th theme's name, `outInfo` to point to information describing the `n`th theme, and `outID` should be set to the `n`th theme's ID number, which used internally by your add-on to distinguish among the themes it supports.

#### make_theme()

`BMediaTheme* make_theme(int32 themeID,  
image_id imageID);`

If you're writing a theme to be loaded from an add-on file, you must implement `make_theme()` to create a `BMediaTheme` for the specified `themeID`. `imageID` is the image_id of the add-on in which the theme is located.

* * *

### Constants

####  Background Color and Bitmap Kinds

Constant| Description  
---|---  
`B_GENERAL_BG`| Used when nothing else is applicable.  
`B_SETTINGS_BG`| Used for panels or windows full of controls.  
`B_PRESENTATION_BG`| Used for windows that present actual media content.  
`B_EDIT_BG`| Used for windows that edit media content.  
`B_CONTROL_BG`| Used for drawing controls.  
`B_HILITE_BG`| Used when highlighting information, such as selected media data.  
  
These constants identify parts of the user interface that a theme can customize the appearance of by altering the color or bitmap that's used in the background for various user interface elements. They're used when calling [`BackgroundBitmapFor()`](BMediaTheme.md#BMediaTheme_BackgroundBitmapFor "BackgroundBitmapFor\(\)") and [`BackgroundColorFor()`](BMediaTheme.md#BMediaTheme_BackgroundColorFor "BackgroundColorFor\(\)").

For example, the `B_PRESENTATION_BG` color and/or bitmap would be used as the background of a window in which an oscilloscope display shows the waveform of a sound wave being played back, and a window used for editing the waveform would have its' background in the color specified by `B_EDIT_BG` (or using the bitmap specified by `B_EDIT_BG`, if one exists), except for the part of the wave that's selected by the user, which would have the background color or bitmap specified by `B_HILITE_BG`.

####  Foreground Color Kinds

Constant| Description  
---|---  
`B_GENERAL_FG`| Used when nothing else is applicable.  
`B_SETTINGS_FG`| Used for panels or windows full of controls.  
`B_PRESENTATION_FG`| Used for windows that present actual media content.  
`B_EDIT_FG`| Used for windows that edit media content.  
`B_CONTROL_FG`| Used for drawing controls.  
`B_HILITE_FG`| Used when highlighting information, such as selected media data.  
  
The foreground color kinds are used when calling [`ForegroundColorFor()`](BMediaTheme.md#BMediaTheme_ForegroundColorFor "ForegroundColorFor\(\)") to obtain the color to use when drawing the foreground portions of various interface elements.

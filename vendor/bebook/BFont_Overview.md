<!-- System Overview - The Interface Kit - BFont -->

## BFont

A [`BFont`](BFont.md "BFont") object records a set of font attributes, such as the font's family, style, size, and so on. You can set most of these attributes to modify the font and then use the object to set the font of a [`BView`](BView.md "BView"). A [`BView`](BView.md "BView")'s font determines how the characters that it draws (with the [`DrawString()`](BView.md#BView_DrawString "DrawString\(\)") and [`DrawChar()`](BView.md#BView_DrawChar "DrawChar\(\)") functions) will be rendered.

A [`BFont`](BFont.md "BFont") object can perform calculations based on the metrics of the particular font it represents. For example, it can tell you how much screen real estate it needs to display a given line of text.

To find which fonts are currently installed on the system, call [`get_font_family()`](TheInterfaceKit_Functions.md#get_font_family "get_font_family\(\), count_font_families\(\), get_font_style\(\), count_font_styles\(\)") and [`get_font_style()`](TheInterfaceKit_Functions.md#get_font_style).

* * *

### Using a BFont Object

A [`BFont`](BFont.md "BFont") object by itself doesn't do anything. To be able to draw characters in the font, the object must be passed to [`BView`](BView.md "BView")'s [`SetFont()`](BView.md#BView_SetFont "SetFont\(\), GetFont\(\)") function (or [`BTextView`](BTextView.md "BTextView")'s [`SetFontAndColor()`](BTextView.md#BTextView_SetFontAndColor "SetFontAndColor\(\), GetFontAndColor\(\)")).

A [`BFont`](BFont.md "BFont") object is always a full representation of a font; all attributes are always set. However, you can choose which of these attributes will modify a [`BView`](BView.md "BView")'s current font by passing a mask to [`SetFont()`](BView.md#BView_SetFont "SetFont\(\), GetFont\(\)") (or [`BTextView`](BTextView.md "BTextView")'s [`SetFontAndColor()`](BTextView.md#BTextView_SetFontAndColor "SetFontAndColor\(\), GetFontAndColor\(\)")). For example, this code sets only the font shear and spacing:
    
    
    BFont font;
    font.SetShear(60.0);
    font.SetSpacing(B_CHAR_SPACING);
    myView->SetFont(&font, B_FONT_SHEAR | B_FONT_SPACING);

Alternatively, the `BView`'s font could have been modified and reset as follows:
    
    
    BFont font;
    myView->GetFont(&font);
    font.SetShear(60.0);
    font.SetSpacing(B_CHAR_SPACING);
    myView->SetFont(&font);

Notice that we had to explicitly reset the view's font (through [`SetFont()`](BView.md#BView_SetFont "SetFont\(\), GetFont\(\)")) after changing the font's attributes.

* * *

### System Fonts

The Interface Kit constructs three [`BFont`](BFont.md "BFont") objects (plain, bold, and fixed)for each application when the application starts up. The values of these fonts are set by the user through the FontPanel preferences application. You can get to these objects through global pointers :

Constant| Description  
---|---  
const BFont* `be_plain_font`| The font that's used to display most gadgets in the user interface, such as check box labels and menu items. All [`BControl`](BControl.md "BControl") objects use this font.  
const BFont* `be_bold_font`| The font that's used to display window titles and [`BBox`](BBox.md "BBox") labels.  
const BFont* `be_fixed_font`| The font that's used to display fixed-width characters.  
  
The global fonts are const objects that can't be modified by your application, and aren't updated by the system, even if the user changes their definitions while your app is running. The new values take effect the next time your application is launched.

To use a system font in a view, simply call [`SetFont()`](BView.md#BView_SetFont "SetFont\(\), GetFont\(\)"):
    
    
    myView->SetFont(be_bold_font);

If you want to modify some attributes of a system font, you have to make a copy of it first (and modify the copy):
    
    
    BFont font(be_bold_font);
    font.SetSize(13.0);
    myView->SetFont(&font);

Applications should respect the user's choices and base all font choices on these three system fonts, rather than hard-code font names into the application. You should not try to predict the fonts that will be installed on the user's machine.

<!-- Classes And Methods - The Interface Kit - BTextView -->

## BTextView

| Derived From:| [`BView`](BView.md "BView") > [`BHandler`](BHandler.md "BHandler")  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/TextView.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BTextView_Overview.md "BTextView")

  * [Constructor and Destructor](BTextView.md#BTextView_ConstructorDestructor)
  * [Hook Functions](BTextView.md#BTextView_HookFunctions)
  * [Member Functions](BTextView.md#BTextView_MemberFunctions)
  * [Static Functions](BTextView.md#BTextView_StaticFunctions)
  * [Scripting Support](BTextView.md#BTextView_ScriptingSupport)
  * [Archived Fields](BTextView.md#BTextView_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BTextView()

`BTextView([`BRect`](BRect.md "BRect") frame,  
const char* name,  
[`BRect`](BRect.md "BRect") textRect,  
uint32 resizingMode,  
uint32 flags);` `BTextView([`BRect`](BRect.md "BRect") frame,  
const char* name,  
[`BRect`](BRect.md "BRect") textRect,  
const [`BFont`](BFont.md "BFont")* font,  
const rgb_color* color,  
uint32 resizingMode,  
uint32 flags);` `BTextView([`BMessage`](BMessage.md "BMessage")* archive);`

Initializes the `BTextView` to the `frame` rectangle, stated in its eventual parent's coordinate system, assigns it an identifying `name`, sets its resizing behavior to `resizingMode` and its drawing behavior with `flags`. These four arguments—`frame`, `name`, `resizingMode`, and `flags`—are identical to those declared for the [`BView`](BView.md "BView") class and are passed to the [`BView`](BView.md "BView") [constructor](BView.md#BView_Constructor "BView\(\)"). The `frame`, `name`, and `resizingMode` arguments are passed to the [`BView`](BView.md "BView") class unchanged, but two flags are added to the flags argument—`B_FRAME_EVENTS`, so that the `BTextView` can reformat the text when it's resized, and `B_PULSE_NEEDED`, so that the caret marking the insertion point can "blink" in time with `B_PULSE` messages. Later, [`AttachedToWindow()`](BTextView.md#BTextView_AttachedToWindow "AttachedToWindow\(\)") will set the window's pulse rate to 500,000 microseconds.

The text rectangle, `textRect`, is stated in the `BTextView`'s coordinate system. It determines where text in placed within the view's bounds rectangle:

  * The first line of text is placed at the top of the text rectangle. As additional lines of text are entered into the view, the text grows downward and may actually extend beyond the bottom of the rectangle.

  * The left and right sides of the text rectangle determine where lines of text are placed within the view. Lines can be aligned to either side of the rectangle, or they can be centered between the two sides. See the [`SetAlignment()`](BTextView.md#BTextView_SetAlignment "SetAlignment\(\), Alignment\(\)") function.

  * When lines wrap on word boundaries, the width of the text rectangle determines the maximum length of a line; each line of text can be as long as the rectangle is wide. When word wrapping isn't turned on, lines can extend beyond the boundaries of the text rectangle. See the [`SetWordWrap()`](BTextView.md#BTextView_SetWordWrap "SetWordWrap\(\), DoesWordWrap\(\)") function.

The bottom of the text rectangle is ignored; it doesn't limit the amount of text the view can contain. The text can be limited by the number of characters, but not by the number of lines.

If a default font is provided, the `BTextView` will display its text in that font, unless another font is later set. Similarly, if a default color is specified, the text will be displayed in that color, unless the color is subsequently changed. If the font is `NULL` or not specified, the `BTextView` uses the system plain font, `be_plain_font`. If the color pointer is `NULL` or not specified, the text is drawn in black.

The constructor establishes the following default properties for a new `BTextView`:

  * The text is selectable and editable. (See [`MakeSelectable()`](BTextView.md#BTextView_MakeSelectable "MakeSelectable\(\) , IsSelectable\(\)") and [`MakeEditable()`](BTextView.md#BTextView_MakeEditable "MakeEditable\(\), IsEditable\(\)").)

  * Multiple character formats are not permitted. (See [`SetStylable()`](BTextView.md#BTextView_SetStylable "SetStylable\(\), IsStylable\(\)").)

  * The text is left-aligned. (See [`SetAlignment()`](BTextView.md#BTextView_SetAlignment "SetAlignment\(\), Alignment\(\)").)

  * The tab width is 28.0 coordinate units. (See [`SetTabWidth()`](BTextView.md#BTextView_SetTabWidth "SetTabWidth\(\), TabWidth\(\)").)

  * Word wrapping is turned on. (See [`SetWordWrap()`](BTextView.md#BTextView_SetWordWrap "SetWordWrap\(\), DoesWordWrap\(\)").)

  * Automatic indenting is turned off. (See [`SetAutoindent()`](BTextView.md#BTextView_SetAutoindent "SetAutoindent\(\), DoesAutoindent\(\)").)

  * The maximum amount of data is permitted. (See [`SetMaxBytes()`](BTextView.md#BTextView_SetMaxBytes "SetMaxBytes\(\), MaxBytes\(\)").)

  * The view doesn't grow to accommodate more characters. (See [`MakeResizable()`](BTextView.md#BTextView_MakeResizable "MakeResizable\(\), IsResizable\(\)").)

  * All characters the user may type are acceptable. (See [`DisallowChar()`](BTextView.md#BTextView_DisallowChar "DisallowChar\(\), AllowChar\(\)").)

See also: [`AttachedToWindow()`](BTextView.md#BTextView_AttachedToWindow "AttachedToWindow\(\)"), [`SetFontAndColor()`](BTextView.md#BTextView_SetFontAndColor "SetFontAndColor\(\), GetFontAndColor\(\)"), the [`BView`](BView.md "BView") [constructor](BView.md#BView_Constructor "BView\(\)")

#### ~BTextView

`virtual ~BTextView()();`

Frees the memory the `BTextView` allocated to hold the text and to store information about it.

* * *

### Hook Functions

#### AttachedToWindow()

`virtual void AttachedToWindow();`

Completes the initialization of the `BTextView` object after it becomes attached to a window. This function sets up the object so that it can correctly format text and display it. Among other things, it sets the drawing mode to `B_OP_COPY`. If the `BTextView` is targeted by scroll bars, it adjusts them so that they're accurately set up for scrolling the text.

Because the `BTextView` uses pulses to animate (or "blink") the caret, the vertical line that marks the current insertion point, this function also enables pulsing in the window and fixes the pulse rate at 2 per second.

`AttachedToWindow()` is called for you when the `BTextView` becomes part a window's view hierarchy; you shouldn't call it yourself, though you can override it. A function that's implemented by a derived class should begin by incorporating the `BTextView` version:
    
    
    void MyText::AttachedToWindow()
    {
       BTextView::AttachedToWindow()
       . . .
    }

If it doesn't, the `BTextView` won't be able to properly display the text.

See also: [`BView::AttachedToWindow()`](BView.md#BView_AttachedToWindow "AttachedToWindow\(\), AllAttached\(\)")

#### DetachedFromWindow()

`virtual void DetachedFromWindow();`

Resets the cursor to the standard hand image (`B_HAND_CURSOR`) if it's above the `BTextView` when the `BTextView` is removed from the window.

See also: [`BView::DetachedFromWindow()`](BView.md#BView_DetachedFromWindow "DetachedFromWindow\(\), AllDetached\(\)")

#### Draw()

`virtual void Draw([`BRect`](BRect.md "BRect") updateRect);`

Draws the `BTextView`.

See also: [`BView::Draw()`](BView.md#BView_Draw "Draw\(\)")

#### FrameResized()

`virtual void FrameResized(float width,  
float height);`

Overrides the [`BView`](BView.md "BView") [version](BView.md#BView_FrameResized "FrameResized\(\)") of this function to reset the ranges of the `BTextView`'s scroll bars and to update the sizes of their proportional knobs whenever the size of the `BTextView` changes.

See also: [`BView::FrameResized()`](BView.md#BView_FrameResized "FrameResized\(\)")

#### KeyDown()

`virtual void KeyDown(const char* bytes,  
int32 numBytes);`

Enters text at the current selection in response to the user's typing. This function is called from the window's message loop for every report of a key-down event—typically once for every character the user types. However, it does nothing unless the `BTextView` is the focus view and the text it contains is editable.

If the character encoded in the bytes string is an editing instruction, `KeyDown()` takes the appropriate action:

  * If the character is from one of the arrow keys (`B_UP_ARROW`, `B_LEFT_ARROW`, `B_DOWN_ARROW`, or `B_RIGHT_ARROW`), it extends the selection or moves the insertion point in the appropriate direction, depending on the modifiers

  * If the character is `B_BACKSPACE` or `B_DELETE`, it deletes the current selection—or the character preceding or following the current insertion point.

  * If the character comes from one of the paging keys (`B_HOME`, `B_END`, `B_PAGE_UP`, or `B_PAGE_DOWN`), it scrolls the display.

Otherwise, it checks whether the character was registered as unacceptable (by [`DisallowChar()`](BTextView.md#BTextView_DisallowChar "DisallowChar\(\), AllowChar\(\)")). If not disallowed, it calls the [`InsertText()`](BTextView.md#BTextView_InsertText "InsertText\(\), DeleteText\(\)") hook function to enter the character into the text and display it. Derived classes can preview about-to-be-inserted characters by overriding [`InsertText()`](BTextView.md#BTextView_InsertText "InsertText\(\), DeleteText\(\)").

See also: [`BView::KeyDown()`](BView.md#BView_KeyDown "KeyDown\(\)")

#### MessageReceived()

`virtual void MessageReceived([`BMessage`](BMessage.md "BMessage")* message);`

Augments the [`BView`](BView.md "BView") version of [`MessageReceived()`](BView.md#BView_MessageReceived "MessageReceived\(\)") to handle scripting requests, dropped data, and four editing messages—`B_CUT`, `B_COPY`, `B_PASTE`, and `B_SELECT_ALL`.

If the message was dragged and dropped on the `BTextView` and it contains `B_MIME_TYPE` data under the name "text/plain", this function inserts the new text at the point where it was dropped—but only if [`AcceptsDrop()`](BTextView.md#BTextView_AcceptsDrop "AcceptsDrop\(\), AcceptsPaste\(\)") returns `true` for the message.

This function handles `B_CUT`, `B_COPY`, `B_PASTE`, and `B_SELECT_ALL` messages by calling the [`Cut()`](BTextView.md#BTextView_Cut "Cut\(\), Copy\(\), Paste\(\), Clear\(\)"), [`Copy()`](BTextView.md#BTextView_Copy), [`Paste()`](BTextView.md#BTextView_Paste), and [`SelectAll()`](BTextView.md#BTextView_SelectAll "SelectAll\(\)") functions. A `BTextView` will get these messages, even if the application doesn't send them, when it's the focus view and the user uses the Command+x, Command+c, Command+v, and Command+a shortcuts. See "[Shortcuts and Menu Items](BTextView_Overview.md#BTextView_Overview_ShortcutsAndMenuItems "Shortcuts and Menu Items")" in the class overview for information on how to set up compatible Cut, Copy, Paste, and Select All menu items.

To inherit this functionality, `MessageReceived()` functions implemented by derived classes should be sure to call the `BTextView` version.

See also: [`AcceptsPaste()`](BTextView.md#BTextView_AcceptsPaste), [`InsertText()`](BTextView.md#BTextView_InsertText "InsertText\(\), DeleteText\(\)"), [`BView::MessageReceived()`](BView.md#BView_MessageReceived "MessageReceived\(\)"), [`BInvoker::SetMessage()`](BInvoker.md#BInvoker_SetMessage "SetMessage\(\), Message\(\), Command\(\)"), [`BInvoker::SetTarget()`](BInvoker.md#BInvoker_SetTarget "SetTarget\(\), Target\(\), IsTargetLocal\(\), Messenger\(\)")

#### MouseDown()

`virtual void MouseDown(BPoint point);`

Selects text, drags text, and positions the insertion point in response to the user's mouse actions. If the `BTextView` isn't already the focus view for its window, this function calls [`MakeFocus()`](BTextView.md#BTextView_MakeFocus "MakeFocus\(\)") to make it the focus view.

`MouseDown()` is called for each mouse-down event that occurs inside the `BTextView`'s frame rectangle.

See also: [`BView::MouseDown()`](BView.md#BView_MouseDown "MouseDown\(\)")

#### MouseMoved()

`virtual void MouseMoved([`BPoint`](BPoint.md "BPoint") point,  
uint32 transit,  
[`BMessage`](BMessage.md "BMessage")* message);`

Responds to `B_MOUSE_MOVED` messages by changing the cursor to the standard I-beam image for editing text whenever the cursor enters the view and by resetting it to the standard hand image when the cursor exits the view. The cursor is changed to an I-beam for text that is selectable or editable, but only if the `BTextView` is the current focus view in the active window. However, when the cursor moves over the current selection, this function changes it from the I-beam back to the standard hand image. This is done to indicate that it's possible to drag and drop the current selection.

If a message is being dragged to the `BTextView`, this function tests it see whether it contains textual data and tracks it to its destination.

See also: [`BView::MouseMoved()`](BView.md#BView_MouseMoved "MouseMoved\(\)"), [`AcceptsDrop()`](BTextView.md#BTextView_AcceptsDrop "AcceptsDrop\(\), AcceptsPaste\(\)")

#### Pulse()

`virtual void Pulse();`

Turns the caret marking the current insertion point on and off when the `BTextView` is the focus view in the active window. `Pulse()` is called by the system at regular intervals.

This function is first declared in the [`BView`](BView.md "BView") class.

See also: [`BView::Pulse()`](BView.md#BView_Pulse "Pulse\(\)")

#### WindowActivated()

`virtual void WindowActivated(bool flag);`

Highlights the current selection when the `BTextView`'s window becomes the active window (when `flag` is `true`)—provided that the `BTextView` is the current focus view—and removes the highlighting when the window ceases to be the active window (when `flag` is `false`).

If the current selection is empty (if it's an insertion point), it's highlighted by turning the caret on and off (blinking it).

The Interface Kit calls this function for you whenever the `BTextView`'s window becomes the active window or it loses that status.

See also: [`BView::WindowActivated()`](BView.md#BView_WindowActivated "WindowActivated\(\)"), [`MakeFocus()`](BTextView.md#BTextView_MakeFocus "MakeFocus\(\)")

* * *

### Member Functions

####  AcceptsDrop(), AcceptsPaste()

`virtual bool AcceptsDrop(const [`BMessage`](BMessage.md "BMessage")* archive);``virtual bool AcceptsPaste([`BClipboard`](BClipboard.md "BClipboard")* clipboard);`

These functions look at the data in their arguments, and return `true` if its "acceptable." The default implementations return `true` if both of the following are `true`:

  * the `BTextView` is editable

  * the message or clipboard contains `B_MIME_TYPE` data stored under the name "text/plain".

If the data isn't acceptable, the drop or paste operation is aborted.

`AcceptsDrop()` is invoked in two places: When a message is dragged over the `BTextView`, and when the message is dropped on the object. `AcceptsPaste()` is called when a `B_PASTE` message is received. If you augment these functions to accept more types, you'll also have to augment the [`MessageReceived()`](BTextView.md#BTextView_MessageReceived "MessageReceived\(\)") and [`Paste()`](BTextView.md#BTextView_Paste) functions to insert the text found in message or clipboard.

#### Archive()

`virtual status_t Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Calls the inherited version of [`Archive()`](BView.md#BView_Archive "Archive\(\)") and stores the `BTextView` in the [`BMessage`](BMessage.md "BMessage") archive.

See also: [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)"), [`Instantiate()`](BTextView.md#BTextView_Instantiate "Instantiate\(\)") static function

#### CanEndLine()

`virtual bool CanEndLine(int32 offset);`

Returns `true` if the character at offset can be the last character in a line of text, and `false` if not. Sometimes this depends on whether the next character (if there is one) can begin a line. This function is called as the `BTextView` figures out where to break lines, but only if word wrapping is turned on.

As implemented, `CanEndLine()` allows the following ASCII characters to end a line regardless of the context:
    
    
    B_SPACE =  < / &
    B_TAB   +  > *
    B_ENTER -- ^ | '\0'
    

The default implementation also understands the line-ending conventions for Chinese and Japanese. Because these languages are written without the spaces that typically end lines in other languages, lines can potentially break anywhere. However, certain characters are prohibited from ending a line and others are prohibited from beginning a new line. `CanEndLine()` prevents lines from ending either on a prohibited ending character or on the character before a prohibited beginning character.

Derived classes can override this function to apply different criteria for where lines end, possibly looking at the context of the offset character. You can also augment the current implementation so that it understands the conventions for other languages.

If you override this function to look to the left or right of the character at offset, be sure to check that you haven't stepped beyond the range of the text. For example, this version of the function makes sure that the first hyphen of a pair doesn't break a line:
    
    
    bool MyTextView::CanEndLine(int32 offset)
    {
       if ( ByteAt[offset] == '-' ) {
          if ( TextLength() - offset > 1 ) {
                if ( ByteAt[offset + 1] == '-' )
                   return false;
          }
       }
       return (_baseClass_ ::CanEndLine(offset));
    }

See also: [`SetWordWrap()`](BTextView.md#BTextView_SetWordWrap "SetWordWrap\(\), DoesWordWrap\(\)")

####  Cut(), Copy(), Paste(), Clear()

`virtual void Cut([`BClipboard`](BClipboard.md "BClipboard")* clipboard);``virtual void Copy([`BClipboard`](BClipboard.md "BClipboard")* clipboard);``virtual void Paste([`BClipboard`](BClipboard.md "BClipboard")* clipboard);``virtual void Clear();`

These functions implement the standard cut, copy, paste, and clear commands. `Cut()` and `Copy()` both copy the current selection to the specified clipboard; `Cut()` also deletes the text from the `BTextView` and removes it from the display. The text is entered in the clipboard as `B_MIME_TYPE` data under the name "text/plain". `Paste()` looks in the clipboard for just this type of data and pastes it into the text—but only if [`AcceptsPaste()`](BTextView.md#BTextView_AcceptsPaste) returns `true`. The new text replaces the current selection, or is placed at the site of the current insertion point.

If the `BTextView` supports multiple character formats, `Cut()` and `Copy()` also place a text_run_array structure describing the formats of the copied text in the clipboard—as `B_MIME_TYPE` data under the name "application/x-vnd.Be-text_run_array". If the `BTextView` that takes text from the clipboard supports multiple formats, `Paste()` looks for the text_run_array in the clipboard and sets the formats of the pasted text accordingly.

In most cases, the clipboard argument will be identical to the global `be_clipboard` object.

The `Clear()` function simply removes the currently-selected text from the `BTextView` without affecting any clipboard. If there's no selection, nothing happens.

See also: [`AcceptsPaste()`](BTextView.md#BTextView_AcceptsPaste), "[Shortcuts and Menu Items](BTextView_Overview.md#BTextView_Overview_ShortcutsAndMenuItems "Shortcuts and Menu Items")" in the overview

####  DisallowChar(), AllowChar()

`void DisallowChar(uint32 aChar);``void AllowChar(uint32 aChar);`

These functions inform the `BTextView` whether the user should be allowed to enter `aChar` into the text. By default, all characters are allowed. Call `DisallowChar()` for each character you want to prevent the `BTextView` from accepting, preferably when first setting up the object. Although declared as uint32, `aChar` must be a character encoded in a single byte; it can't be a 16-bit Unicode value or a multibyte UTF-8 string.

`AllowChar()` reverses the effect of `DisallowChar()`.

Alternatively, and for more control over the context in which characters are accepted or rejected, you can implement an [`InsertText()`](BTextView.md#BTextView_InsertText "InsertText\(\), DeleteText\(\)") function for the `BTextView`. [`InsertText()`](BTextView.md#BTextView_InsertText "InsertText\(\), DeleteText\(\)") is called for all insertions, including each character the user types, all text the user drags to the `BTextView`, and all attempts to paste from the clipboard.

#### FindWord()

`virtual void FindWord(int32 offset,  
int32* start,  
int32* finish);`

Looks for a sequence of characters that qualifies as a word—that is, a sequence that the user can double-click to select—that includes the character at `offset`. This function places the offset of the word's first character in the variable that `start` refers to and the offset following the last character in the word in the variable that `finish` refers to. If the `offset` character can't be part of a word, the `start` and `finish` offsets will be identical.

As implemented, this function allows the user to select a group of similar characters with a double-click. For example, in the following line of malformed text,
    
    
    "You what!!?"
    

it would allow the user to select the words "You" and "what," the group of spaces between the words, and the group of punctuation marks at the end.

The function also defines similar groups of Japanese characters that can be selected together.

#### GetDragParameters()

##### protected

`virtual void GetDragParameters([`BMessage`](BMessage.md "BMessage")* drag,  
[`BBitmap`](BBitmap.md "BBitmap")** bitmap,  
[`BPoint`](BPoint.md "BPoint")* point,  
[`BHandler`](BHandler.md "BHandler")** handler);`

`GetDragParameters()` is called when a drag session is initiated. The values that it retrieves (by reference in the arguments) are passed on to [`BView::DragMessage()`](BView.md#BView_DragMessage "DragMessage\(\)"). If you don't supply a bitmap (if you set `*bitmap` to `NULL`), the outline rectangle version of [`DragMessage()`](BView.md#BView_DragMessage "DragMessage\(\)") is used.

#### GetSelection()

`void GetSelection(int32* start,  
int32* finish);`

Provides the current selection by writing the offset before the first selected character into the variable referred to by `start` and the offset after the last selected character into the variable referred to by `finish`. If no characters are selected, both offsets will record the position of the current insertion point.

If the text isn't selectable, both offsets will be 0.

See also: [`Select()`](BTextView.md#BTextView_Select "Select\(\)")

#### GetTextRegion()

`void GetTextRegion(int32 start,  
int32 finish,  
[`BRegion`](BRegion.md "BRegion")* region) const;`

Calculates the region where the run of characters beginning at the `start` offset and ending at the `finish` offset would be displayed within the `BTextView`'s coordinate system, and modifies the [`BRegion`](BRegion.md "BRegion") object passed as the third argument, `region`, so that it represents that region.

See also: [`TextHeight()`](BTextView.md#BTextView_TextHeight)

####  GoToLine(), CountLines(), CurrentLine()

`void GoToLine(int32 index);``int32 CurrentLine() const;``int32 CountLines() const;`

`GoToLine()` moves the insertion point to the beginning of the line at `index`. The first line has an index of 0, the second line an index of 1, and so on. If the index is out-of-range, the insertion point is moved to the beginning of the line with the nearest in-range index—that is, to either the first or the last line.

`CurrentLine()` returns the index of the line where the first character of the selection—or the character following the insertion point—is currently located.

`CountLines()` returns how many lines of text the `BTextView` currently contains.

Like other functions that change the selection, `GoToLine()` doesn't automatically scroll the display to make the new selection visible. Call [`ScrollToSelection()`](BTextView.md#BTextView_ScrollToSelection) to be sure that the user can see the start of the selection.

####  HideTyping(), IsTypingHidden()

`void HideTyping(bool flag);``bool IsTypingHidden() const;`

The first of these functions sets whether the user can see the text in the `BTextView`; the second returns whether or not the text is currently visible. When flag's state is `true`, text contained by the `BTextView` isn't visible.

#### Highlight()

`inline void Highlight(int32 start,  
int32 finish);`

Highlights (or unhighlights) the characters between the `start` and `finish` offsets. This is the function that the `BTextView` calls to highlight and unhighlight the current selection. You don't need to call it yourself for this purpose. It's in the public API just in case you may need to highlight a range of text in some other circumstance.

If the text is not currently highlighted, this function highlights it. But if the text is already highlighted, it unhighlights it. If you highlight some text, be sure to unhighlight it before the next editorial change; the `BTextView` will not do it for you.

See also: [`Select()`](BTextView.md#BTextView_Select "Select\(\)"), [`GetTextRegion()`](BTextView.md#BTextView_GetTextRegion "GetTextRegion\(\)")

####  Insert(), Delete()

`void Insert(const char* text,  
const text_run_array* runs = NULL);` `void Insert(const char* text,  
int32 length,  
const text_run_array* runs = NULL);` `void Insert(int32 offset,  
const char* text,  
int32 length,  
const text_run_array* runs = NULL);`

`void Delete();` `void Delete(int32 start,  
int32 finish);`

`Insert()` adds `length` bytes of text to the `BTextView`—or if a length isn't specified, all the characters of the text string up to the null character that terminates it. The text is inserted at `offset`—or at the beginning of the current selection if an `offset` isn't specified. The current selection is not deleted and the insertion is not selected.

The inserted characters are displayed in the fonts and colors specified in the accompanying `runs` array, provided the `BTextView` allows multiple character formats. If multiple formats aren't allowed, the `runs` array is ignored. If multiple formats are allowed but a `runs` array isn't provided, the insertion is displayed in the font and color in force at the point of insertion. This generally means the font and color of the first character of the selection, or of the character immediately preceding the offset character.

Offsets in the `runs` array should describe the text being inserted; in other words, the first offset should be 0. See [`SetRunArray()`](BTextView.md#BTextView_SetRunArray "SetRunArray\(\), RunArray\(\)") for a description of the text_run_array structure.

`Insert()` doesn't assume responsibility for the `text` data or the `runs` array. It copies the information it needs.

`Delete()` removes the characters bounded by the `start` and `finish` offsets from the display and deletes them from the `BTextView`'s text, without copying them to the clipboard. If the `start` and `finish` offsets are the same, nothing is deleted. If offsets are not provided, `Delete()` deletes the current selection.

See also: [`SetText()`](BTextView.md#BTextView_SetText "SetText\(\)"), [`Cut()`](BTextView.md#BTextView_Cut "Cut\(\), Copy\(\), Paste\(\), Clear\(\)"), [`SetRunArray()`](BTextView.md#BTextView_SetRunArray "SetRunArray\(\), RunArray\(\)")

####  InsertText(), DeleteText()

##### protected

`virtual void InsertText(const char* text,  
int32 length,  
int32 offset,  
const text_run_array* runs = NULL);``virtual void DeleteText(int32 start,  
int32 finish);`

These protected functions are the vehicles through which the `BTextView` performs every insertion and deletion of text (with one exception). You can augment them in a subclass to take note of pending changes to the text, and to modify or prevent the change.

Don't call `Insert()`, `Delete()` or any other high-level text-manipulating function in your implementation.

`InsertText()` adds `length` bytes of text to the `BTextView`, inserting it at `offset` within the text buffer. The font and color of the inserted characters may be described by an accompanying `runs` array. If the `BTextView` doesn't support multiple character formats, the `runs` array is ignored. If multiple formats are supported but the `runs` array is `NULL`, the text is displayed in the font and color of the character preceding offset (or of the first character, if offset is 0.)

The offsets in the `runs` data structure are relative to the inserted text; that is, the first offset in the array is 0, not `offset`.

`InsertText()` is called for every insertion, except one. The exception occurs when [`SetText()`](BTextView.md#BTextView_SetText "SetText\(\)") takes text from a file; in this case the text goes directly from the file to the `BTextView`; it's not stored in a temporary buffer while `InsertText()` is called.

`DeleteText()` removes the text bounded by the `start` and `finish` offsets. It fails if the offsets don't differ, or if the `finish` offset isn't greater than the `start` offset. This function is called for every deletion.

See also: [`Insert()`](BTextView.md#BTextView_Insert "Insert\(\), Delete\(\)"), [`Delete()`](BTextView.md#BTextView_Delete)

####  LineAt(), PointAt(), OffsetAt()

`int32 LineAt(int32 offset) const;` `int32 LineAt([`BPoint`](BPoint.md "BPoint") point) const;`

`[`BPoint`](BPoint.md "BPoint") PointAt(int32 offset,  
float* height = NULL) const;`

`int32 OffsetAt(int32 offset) const;` `int32 OffsetAt([`BPoint`](BPoint.md "BPoint") point) const;`

These functions translate between coordinate values, text offsets, and line indices. `LineAt()` returns the index of the line containing the character at `offset` in the text, or the line located at the specified `point` in the `BTextView`'s coordinate system. Line indices begin at 0.

`PointAt()` returns the coordinate location of the character at `offset`. The point is the left top corner of a rectangle enclosing the character and is stated in the `BTextView`'s coordinate system. The x-coordinate of the point is the position on the baseline where the character is (or would be) drawn; its y-coordinate is the top of the line where the `offset` character is located. If a `height` argument is provided, `PointAt()` returns the height of the line by reference.

`OffsetAt()` returns the offset to the character that begins the `index` line, or to the character displayed at `point`.

####  LineHeight(), TextHeight()

`float LineHeight(int32 index = 0) const;``float TextHeight(int32 firstIndex,  
int32 lastIndex) const;`

`LineHeight()` returns the height of the line of text at `index`, or the first line if an index isn't specified. Line indices begin at 0. The height is stated in coordinate units and depends on the font. It's the sum of how far characters can ascend above and descend below the baseline, plus the amount of leading that separates lines. If more than one font is used on the line, the ascent is taken from the tallest font and the descent and leading from the deepest.

`TextHeight()` returns the height of the set of lines from `firstIndex` through `lastIndex`.

Both functions reset out-of-range indices to be in-range—that is, to the index of the first or last line.

See also: [`BFont::GetHeight()`](BFont.md#BFont_GetHeight "GetHeight\(\), font_height")

#### LineWidth()

`float LineWidth(int32 index = 0) const;`

Returns the width of the line at `index`—or, if no index is given, the width of the first line. The value returned is the sum of the widths (in coordinate units) of all the characters in the line, from the first through the last, including tabs and spaces. Line indices begin at 0.

If the `index` passed is out-of-range, it's reinterpreted to be the nearest in-range index—that is, as the index to the first or the last line.

See also: [`BFont::StringWidth()`](BFont.md#BFont_StringWidth "StringWidth\(\), GetStringWidths\(\)")

####  MakeEditable(), IsEditable()

`void MakeEditable(bool flag = true);``bool IsEditable() const;`

The first of these functions sets whether the user can edit the text displayed by the `BTextView`; the second returns whether or not the text is currently editable. Text is editable by default.

When text is editable but not selectable, the user can enter and delete text at the insertion point, but can't select text to make changes to more than one character at a time.

See also: [`MakeSelectable()`](BTextView.md#BTextView_MakeSelectable "MakeSelectable\(\) , IsSelectable\(\)")

#### MakeFocus()

`virtual void MakeFocus(bool flag = true);`

Overrides the [`BView`](BView.md "BView") version of [`MakeFocus()`](BView.md#BView_MakeFocus "MakeFocus\(\)") to highlight the current selection when the `BTextView` becomes the focus view (when `flag` is `true`) and to unhighlight it when the `BTextView` no longer is the focus view (when `flag` is `false`). However, the current selection is highlighted only if the `BTextView`'s window is the current active window.

This function is called for you whenever the user's actions make the `BTextView` become the focus view, or force it to give up that status.

See also: [`MouseDown()`](BTextView.md#BTextView_MouseDown "MouseDown\(\)")

####  MakeResizable(), IsResizable()

`void MakeResizable(bool resizable,  
[`BView`](BView.md "BView")* containerView = NULL);``bool IsResizable() const;`

`MakeResizable()` gives the `BTextView` the ability to automatically resize itself to fit its contents if the `resizable` flag is `true`, and takes away that ability if the flag is `false`. `IsResizable()` returns whether the `BTextView` is currently resizable.

The frame rectangle and text rectangle of a resizable `BTextView` automatically grow and shrink to exactly enclose all the characters entered by the user. The object should display just a single line of text (the resizing is horizontal); if the `resizable` flag is `true`, `MakeResizable()` turns off line wrapping. The text can be aligned to the left, right, or center of the text rectangle.

The `containerView` is a view that draws a border around the text (like a [`BScrollView`](BScrollView.md "BScrollView") object) and is the parent of the `BTextView`; it's the view that's resized to fit the text. The `BTextView`'s resizing mode should be such that it will be resized in tandem with the container (for example, `B_FOLLOW_LEFT_RIGHT` or `B_FOLLOW_ALL_SIDES`). However, if the `containerView` is `NULL`, as it is by default, the `BTextView` itself is resized to fit the text.

If the `resizable` flag is `false`, the `containerView` argument is ignored.

This resizing mechanism is an alternative to the automatic resizing behavior provided in the [`BView`](BView.md "BView") class. It triggers resizing on the user's entry of text, not on a change in the parent view's size. The two schemes are incompatible; the container view (or the `BTextView`, if there is no container) should not automatically resize itself when its parent is resized.

See also: [`SetAlignment()`](BTextView.md#BTextView_SetAlignment "SetAlignment\(\), Alignment\(\)")

####  MakeSelectable() , IsSelectable()

`void MakeSelectable(bool flag = true);``bool IsSelectable() const;`

The first of these functions sets whether it's possible for the user to select text displayed by the `BTextView`; the second returns whether or not the text is currently selectable. Text is selectable by default.

When text is selectable but not editable, the user can select one or more characters to copy to the clipboard, but can't position the insertion point (an empty selection), enter characters from the keyboard, or paste new text into the view.

See also: [`MakeEditable()`](BTextView.md#BTextView_MakeEditable "MakeEditable\(\), IsEditable\(\)")

#### ResolveSpecifier()

See [`BHandler::ResolveSpecifier()`](BHandler.md#BHandler_ResolveSpecifier "ResolveSpecifier\(\)")

####  ScrollToOffset(), ScrollToSelection()

`virtual void ScrollToOffset(int32 offset);``void ScrollToSelection();`

These functions scroll the text so that the character at `offset`—or the character that begins the current selection—is within the visible region of the view. If the `BTextView` is equipped with scroll bars, the [`BScrollBar`](BScrollBar.md "BScrollBar") objects are informed so they can update themselves.

See also: [`BView::ScrollTo()`](BView.md#BView_ScrollTo)

#### Select()

`virtual void Select(int32 start,  
int32 finish);`

Selects the characters from `start` up to `finish`, where `start` and `finish` are offsets into the `BTextView`'s text. If `start` and `finish` are the same, the selection will be empty (an insertion point). See "[Offsets](BTextView_Overview.md#BTextView_Overview_Offsets "Offsets")" in the class overview for a discussion of the constraints on the offset arguments.

Normally, the selection is changed by the user. This function provides a way to change it programmatically.

If the `BTextView` is the current focus view in the active window, `Select()` highlights the new selection (or displays a blinking caret at the insertion point). However, it doesn't automatically scroll the contents of the `BTextView` to make the new selection visible. Call [`ScrollToSelection()`](BTextView.md#BTextView_ScrollToSelection) to be sure that the user can see the start of the selection.

See also: [`Text()`](BTextView.md#BTextView_Text "Text\(\), GetText\(\), ByteAt\(\)"), [`GetSelection()`](BTextView.md#BTextView_GetSelection "GetSelection\(\)"), [`ScrollToSelection()`](BTextView.md#BTextView_ScrollToSelection), [`GoToLine()`](BTextView.md#BTextView_GoToLine "GoToLine\(\), CountLines\(\), CurrentLine\(\)"), [`MouseDown()`](BTextView.md#BTextView_MouseDown "MouseDown\(\)")

#### SelectAll()

`void SelectAll();`

Selects the entire text of the `BTextView`, and highlights it if the `BTextView` is the current focus view in the active window.

See also: [`Select()`](BTextView.md#BTextView_Select "Select\(\)")

####  SetAlignment(), Alignment()

`void SetAlignment(alignment where);``alignment Alignment() const;`

These functions set the way text is aligned within the text rectangle and return the current alignment. Three settings are possible:

Constant| Description  
---|---  
`B_ALIGN_LEFT`| Each line is aligned at the left boundary of the text rectangle.  
`B_ALIGN_RIGHT`| Each line is aligned at the right boundary of the text rectangle.  
`B_ALIGN_CENTER`| Each line is centered between the left and right boundaries of the text rectangle.  
  
The default is `B_ALIGN_LEFT`.

####  SetAutoindent(), DoesAutoindent()

`void SetAutoindent(bool flag);``bool DoesAutoindent() const;`

These functions set and return whether a new line of text is automatically indented the same as the preceding line. When set to `true` and the user types Return at the end of a line that begins with tabs or spaces, the new line will automatically indent past those tabs and spaces to the position of the first visible character.

The default value is `false`.

####  SetColorSpace(), ColorSpace()

`void SetColorSpace(color_space space);``color_space ColorSpace() const;`

These functions set and return the color space of the offscreen bitmap that buffers the drawing the `BTextView` does. The default color space is `B_CMAP8`.

See also: the [`BBitmap`](BBitmap.md "BBitmap") class

####  SetDoesUndo(), DoesUndo(), UndoState(), Undo(), undo_state

`void SetDoesUndo(bool sayIt);``bool DoesUndo() const;``undo_state UndoState(bool* isRedo) const;``virtual void Undo([`BClipboard`](BClipboard.md "BClipboard")* clipboard);`
    
    
    enum undo_state {}

These functions and enum comprise `BTextView`'s undo world. The operations that `BTextView` can undo are listed below. The default undo mechanism is one operation deep: Udoing undoes the previous (undo-able) operation; a second (immediate) undo redoes the operation.

You call `SetDoesUndo()` to enable or disable the undo machinery for this object. By default, undo is enabled. The ability to undo is also controlled by the object's editability ( [`MakeEditable()`](BTextView.md#BTextView_MakeEditable "MakeEditable\(\), IsEditable\(\)")), but this is a tautology, since in order to have something to undo you have to have been able to edit the object in the first place.

`DoesUndo()` tells you whether the object is "undoable" as set by `SetDoesUndo()`. It doesn't take editability into consideration.

`UndoState()` tells you what the previous action was (as represented by the constants listed below). This is the action that will be undone if the object is told to undo. The `isRedo` value that's returned by reference is set to `true` if the previous action occurred because of an undo.

Constant| Description  
---|---  
`B_UNDO_UNAVAILABLE`| Nothing to undo  
`B_UNDO_TYPING`| Text was inserted or deleted  
`B_UNDO_CUT`| Selection was cut  
`B_UNDO_PASTE`| Text was pasted  
`B_UNDO_CLEAR`| Selection was cleared  
`B_UNDO_DROP`| Message was dropped  
  
The `Undo()` hook function is called when the `BTextView` receives a `B_UNDO` message. (By default, `B_UNDO` is bound to the window's Command+z shortcut; Undo menu items need to set up the binding explicitly.) The `clipboard` argument is a pointer to the `be_clipboard`; if you're using a custom clipboard in your `BTextView` subclass, you should pass along your clipboard when you invoke the inherited version of `Undo()`.

Don't try to do too much in a subclass implementation of `Undo()`. In particular, you probably won't get too far if you're trying to "broaden" the undo tree.

####  SetFontAndColor(), GetFontAndColor()

`void SetFontAndColor(int32 start,  
int32 finish,  
const [`BFont`](BFont.md "BFont")* font,  
uint32 properties = B_FONT_ALL,  
rgb_color* color = NULL);` `void SetFontAndColor(const [`BFont`](BFont.md "BFont")* font,  
uint32 properties = B_FONT_ALL,  
rgb_color* color = NULL);`

`void GetFontAndColor(int32 offset,  
[`BFont`](BFont.md "BFont")* font,  
rgb_color* color = NULL) const;` `void GetFontAndColor([`BFont`](BFont.md "BFont")* font,  
uint32* sameProperties = B_FONT_ALL,  
rgb_color* color = NULL,  
bool* sameColor = NULL) const;`

These functions set and get the font and color used to display the text. If the `BTextView` supports multiple character formats, `SetFontAndColor()` sets the font and color of the characters bounded by the `start` and `finish` offsets. If no offsets are given, it sets the font and color of the current selection. However, if multiple character formats are not supported, `SetFontAndColor()` ignores the offsets and formats the entire text.

`SetFontAndColor()` works like [`BView`](BView.md "BView")'s [`SetFont()`](BView.md#BView_SetFont "SetFont\(\), GetFont\(\)") function. It sets the `font` to the attributes of the font [`BFont`](BFont.md "BFont") object that are enumerated by the properties mask. The mask is formed by combining the following constants:

  * `B_FONT_FAMILY_AND_STYLE`
  * `B_FONT_SIZE`
  * `B_FONT_SHEAR`
  * `B_FONT_ROTATION`
  * `B_FONT_SPACING`
  * `B_FONT_ENCODING`
  * `B_FONT_FACE`
  * `B_FONT_FLAGS`

In addition, `B_FONT_ALL` is a shorthand for all properties of the specified font. However, the `BTextView` modifies the font to ensure that:

  * Characters are not rotated.

  * Antialiasing is not disabled.

  * The spacing mode is `B_BITMAP_SPACING`.

  * The character encoding is UTF-8 (`B_UNICODE_UTF8`).

If the `font` argument is `NULL`, the font is not set and the properties mask is ignored.

The color of the characters is set by a pointer to an rgb_color structure. If the pointer is `NULL`, as it is by default, the color is not set.

`GetFontAndColor()` gets the font and color used to display the character at `offset`. It modifies the `font` [`BFont`](BFont.md "BFont") object and the `color` rgb_color structure so that they describe the font and color of the character.

If an `offset` isn't specified, `GetFontAndColor()` looks at the current selection. It provides a font and color description of the first character of the selection—or the character at the insertion point if the selection is empty. It also modifies that variable that the `sameProperties` argument refers to so that it lists all the font properties that are uniform for all characters in the selection. Similarly, it indicates, in the variable that `sameColor` refers to, whether all the characters in the selection are displayed in the same color.

####  SetMaxBytes(), MaxBytes()

`void SetMaxBytes(int32 max);``int32 MaxBytes() const;`

These functions set and return the maximum number of bytes that the `BTextView` can accept. The default is the maximum number of bytes that can be designated by a signed 32-bit integer, a number sufficiently large to accommodate all uses of a `BTextView`. Use this function only if you need to restrict the number of characters that the user can enter in a text field.

Note that these functions count bytes, not characters.

####  SetRunArray(), RunArray()

`void SetRunArray(int32 start,  
int32 finish,  
const text_run_array* runs);``text_run_array* RunArray(int32 start,  
int32 finish,  
int32* length = NULL) const;`

These functions set and return the font and color formats of all the characters bounded by the `start` and `finish` offsets. The formats are described by a text_run_array structure, which has the following fields:

Field| Description  
---|---  
int32 `count`| The number of text_run structures in the array.  
text_run `runs[1]`| A structure describing the font and color formats in effect at a particular offset in the `BTextView`'s text.  
  
The text_run structure describes a run of characters that share the same font and color formats. It has three fields:

Field| Description  
---|---  
int32 `offset`| An offset to the first byte of a character in the text buffer. The text run begins with this character; it continues until another run begins.  
BFont `font`| The font that's used to display the run of characters beginning at the specified offset.  
rgb_color `color`| The color that's used to display the run of characters beginning at the specified offset.  
  
The first offset of the first text_run in the array passed to `SetRunArray()` should be 0; the array returned by `RunArray()` also begins at offset 0.

If the `BTextView` doesn't support multiple character formats, `SetRunArray()` ignores the `start` and `finish` offsets and sets the entire text to the font and color of the first text_run in the array. Similarly, `RunArray()` returns a text_run_array with one text_run describing the entire text.

`RunArray()` returns a pointer to memory that it allocated (using `malloc()`). It puts the number of bytes that it allocated in the variable that the `length` argument points to. Although `RunArray()` allocated the memory, the caller is responsible for freeing it when the returned text_run_array is no longer needed.

`SetRunArray()` doesn't assume responsibility for the runs data it's passed; it's up to the caller to free it.

See also: [`SetFontAndColor()`](BTextView.md#BTextView_SetFontAndColor "SetFontAndColor\(\), GetFontAndColor\(\)")

####  SetStylable(), IsStylable()

`void SetStylable(bool stylable);``bool IsStylable() const;`

`SetStylable()` sets whether the `BTextView` permits multiple character formats. If the `stylable` flag is `true`, the functions that set the font and color of the text can apply to particular characters in the text buffer. If the flag is `false`, those functions apply only to the entire text. When `SetStylable()` is called to turn off support for multiple formats, all the text is reformatted in the font and color of the first character. 

`IsStylable()` returns whether multiple formats are permitted. By default, they're not. 

See also: [`SetFontAndColor()`](BTextView.md#BTextView_SetFontAndColor "SetFontAndColor\(\), GetFontAndColor\(\)") [`SetRunArray()`](BTextView.md#BTextView_SetRunArray "SetRunArray\(\), RunArray\(\)")

####  SetTabWidth(), TabWidth()

`void SetTabWidth(float width);``float TabWidth() const;`

These functions set the distance between tab stops to `width` coordinate units and return the current tab width. Tabs cannot be removed nor can they be individually set; all tabs have a uniform width. The default tab width is 28.0 coordinate units.

#### SetText()

`void SetText(const char* text,  
const text_run_array* runs = NULL);` `void SetText(const char* text,  
int32 length,  
const text_run_array* runs = NULL);` `void SetText([`BFile`](BFile.md "BFile")* file,  
int32 offset,  
int32 length,  
const text_run_array* runs = NULL);`

Removes any text currently in the `BTextView` and copies new text from a `text` buffer or from a `file` to replace it. This function copies `length` bytes of text from the buffer—or all the bytes in the buffer, up to the null character, if a `length` isn't specified. Or it copies `length` bytes from the `file` beginning at the `offset` byte. If the `text` or `file` is `NULL` or `length` is 0, it empties the `BTextView` without replacing the text.

If a `runs` text_run_array is provided, it will be used to set the font and color formats of the new text—provided that the `BTextView` permits multiple character formats. If not, the `runs` array is ignored.

The `BTextView` doesn't assume ownership of the `text` buffer, the `file`, or the `runs` array; you can delete them when `SetText()` returns.

Text taken from a file is inserted directly into the text, bypassing the [`InsertText()`](BTextView.md#BTextView_InsertText "InsertText\(\), DeleteText\(\)") function. In other words, you won't receive an [`InsertText()`](BTextView.md#BTextView_InsertText "InsertText\(\), DeleteText\(\)") notification for text taken from a file.

This function is typically used to set the text initially displayed in the view. If the `BTextView` is already attached to a window, it's updated to show its new contents.

See also: [`Text()`](BTextView.md#BTextView_Text "Text\(\), GetText\(\), ByteAt\(\)"), [`TextLength()`](BTextView.md#BTextView_TextLength "TextLength\(\)")

####  SetTextRect(), TextRect()

`void SetTextRect([`BRect`](BRect.md "BRect") rect);``[`BRect`](BRect.md "BRect") TextRect() const;`

`SetTextRect()` makes `rect` the `BTextView`'s text rectangle—the rectangle that locates where text is placed within the view. This replaces the text rectangle originally set in the `BTextView` constructor. The layout of the text is recalculated to fit the new rectangle, and the text is redisplayed.

`TextRect()` returns the current text rectangle.

See also: the `BTextView` [constructor](BTextView.md#BTextView_Constructor "BTextView\(\)")

####  SetWordWrap(), DoesWordWrap()

`void SetWordWrap(bool flag);``bool DoesWordWrap() const;`

These functions set and return whether the `BTextView` wraps lines on word boundaries, thus pushing entire words that don't fit at the end of a line to the next line. When word wrapping is turned on, the `BTextView` calls [`CanEndLine()`](BTextView.md#BTextView_CanEndLine "CanEndLine\(\)") to determine exactly where a line can break. If word wrapping is off, lines break only on a newline character (where the user types Return).

By default, word wrapping is turned on (`DoesWordWrap()` returns `true`).

See also: [`SetTextRect()`](BTextView.md#BTextView_SetTextRect "SetTextRect\(\), TextRect\(\)")

####  Text(), GetText(), ByteAt()

`const char* Text() const;``void GetText(int32 offset,  
int32 length,  
char* buffer) const;``uchar ByteAt(int32 offset) const;`

These functions reveal the text contained in the `BTextView`.

`Text()` returns a pointer to the text, which may be a pointer to an empty string if the `BTextView` is empty. The returned pointer can be used to read the text, but not to alter it (use [`SetText()`](BTextView.md#BTextView_SetText "SetText\(\)"), [`Insert()`](BTextView.md#BTextView_Insert "Insert\(\), Delete\(\)"), [`Delete()`](BTextView.md#BTextView_Delete), and other `BTextView` functions to do that).

`GetText()` copies up to `length` bytes of the text into `buffer`, beginning with the byte at `offset`, and adds a null terminator ('0'). Fewer than `length` bytes are copied if there aren't that many between the specified `offset` and the end of the text. This function doesn't make any attempt to ensure that only full character specifications are copied; it's up to the caller to make sure that a character begins at `offset` and that the last byte copied isn't in the middle of a multibyte character. The results won't be reliable if the `offset` is out-of-range.

`ByteAt()` returns the byte located at `offset`. The offset doesn't have to be to the first byte of a character.

The pointer that `Text()` returns is to the `BTextView`'s internal representation of the text. When it returns, the text string is guaranteed to be null-terminated and without gaps. However, the `BTextView` may have had to manipulate the text to get it in that condition. Therefore, there may be a performance price to pay if `Text()` is called frequently. If you're going to copy the text, it's more efficient to have `GetText()` do it for you. If you're going to index into the text, it may be more efficient to call `ByteAt()`.

The pointer that `Text()` returns may no longer be valid after the user or the program next changes the text. Even if valid, the string may no longer be null-terminated and gaps may appear.

See also: [`TextLength()`](BTextView.md#BTextView_TextLength "TextLength\(\)")

#### TextLength()

`int32 TextLength() const;`

Returns the number of bytes of text data the `BTextView` currently contains—the number of bytes in the string that [`Text()`](BTextView.md#BTextView_Text "Text\(\), GetText\(\), ByteAt\(\)") returns (not counting the null terminator).

See also: [`SetMaxBytes()`](BTextView.md#BTextView_SetMaxBytes "SetMaxBytes\(\), MaxBytes\(\)")

* * *

### Static Functions

####  FlattenRunArray(), UnflattenRunArray()

`static void* FlattenRunArray(const text_run_array* runs = NULL,  
int32* numBytes = NULL);``static text_run_array* UnflattenRunArray(const void* data,  
int32* numBytes = NULL);`

These functions flatten and unflatten a text_run_array structure so that it can be treated as an untyped stream of bytes. A text_run_array that's saved on-disk will be valid when the user reboots the machine only if it's saved as flat data. Both functions return pointers to memory they allocate (with `malloc()`). The caller is responsible for freeing the memory when it's no longer needed.

`FlattenRunArray()` flattens the runs text_run_array and returns the flat data. `UnflattenRunArray()` reconstructs a text_run_array from previously flattened data and returns a pointer to the structure.

If a `numBytes` argument is provided, both functions place the number of bytes they allocated for the data in the variable that `numBytes` refers to.

See also: [`SetRunArray()`](BTextView.md#BTextView_SetRunArray "SetRunArray\(\), RunArray\(\)")

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Returns a new `BTextView` object, allocated by new and created by the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive. However, if the archive doesn't contain data for a `BTextView` object, the return value will be `NULL`.

See also: [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`Archive()`](BTextView.md#BTextView_Archive "Archive\(\)")

* * *

### Scripting Support

The `BTextView` class implements an unnamed suite consisting of the following messages:

#### The Selection Property

The current text selection

Using two int32 offsets to the beginning and end of the selection in either the "result" or "data" arrays, these messages convey the current selection of the object in a manner identical to [`GetSelection()`](BTextView.md#BTextView_GetSelection "GetSelection\(\)") and [`Select()`](BTextView.md#BTextView_Select "Select\(\)").

Message| Specifiers| Description  
---|---|---  
`B_GET_PROPERTY`| `B_DIRECT_SPECIFIER`| Returns the current selection.  
`B_SET_PROPERTY`| `B_DIRECT_SPECIFIER`| Sets the current selection.  
  
#### The Text Property

The text in the view

The "Text" get and set messages correspond to the method [`GetText()`](BTextView.md#BTextView_GetText) and [`SetText()`](BTextView.md#BTextView_SetText "SetText\(\)"), using a C string to store the data. If the `B_SET_PROPERTY` message lacks a "data" member, the selection is deleted; otherwise, "range" bytes are inserted at offset "index." The values specifying the range are given in byte rather than character offsets. The range counts towards the end of the text, even for `B_REVERSE_RANGE_SPECIFIER`.

Message| Specifiers| Description  
---|---|---  
`B_COUNT_PROPERTY`| `B_DIRECT_SPECIFIER`| Returns the length of the text in bytes.  
`B_GET_PROPERTY`| `B_RANGE_SPECIFIER`, `B_REVERSE_RANGE_SPECIFIER`| Returns the text in the specified range in the `BTextView`.  
`B_SET_PROPERTY`| `B_RANGE_SPECIFIER`, `B_REVERSE_RANGE_SPECIFIER`| Removes or inserts text into the specified range in the `BTextView`.  
  
#### The text_run_array Property

Font and color information for the text

These messages correspond to the methods [`RunArray()`](BTextView.md#BTextView_RunArray) and [`SetRunArray()`](BTextView.md#BTextView_SetRunArray "SetRunArray\(\), RunArray\(\)"), storing the result as a `B_RAW_TYPE`. As with the "Text" property, the values specifying the range are given in byte rather than character offsets. The range counts towards the end of the text, even for `B_REVERSE_RANGE_SPECIFIER`.

Message| Specifiers| Description  
---|---|---  
`B_GET_PROPERTY`| `B_RANGE_SPECIFIER`, `B_REVERSE_RANGE_SPECIFIER`| Returns the style information for the text in the specified range in the `BTextView`.  
`B_SET_PROPERTY`| `B_RANGE_SPECIFIER`, `B_REVERSE_RANGE_SPECIFIER`| Sets the style information for the text in the specified range in the `BTextView`.  
  
* * *

### Archived Fields

The [`Archive()`](BTextView.md#BTextView_Archive "Archive\(\)") function adds the following fields to its [`BMessage`](BMessage.md "BMessage") argument:

Field| Type code| Description  
---|---|---  
`_text`| `B_STRING_TYPE`| The `BTextView`'s text.  
`_align`| `B_INT32_TYPE`| The text alignment.  
`_tab`| `B_FLOAT_TYPE`| The tab width.  
`_col_sp`| `B_INT32_TYPE`| Color space.  
`_trect`| `B_RECT_TYPE`| The text rectangle.  
`_max`| `B_INT32_TYPE`| The maximum size (a la [`SetMaxBytes()`](BTextView.md#BTextView_SetMaxBytes "SetMaxBytes\(\), MaxBytes\(\)")).  
`_sel` (array)| `B_INT32_TYPE`| index 0: selection start; index 1: selection end  
`_dis_ch` (array)| `B_RAW_TYPE`| Disallowed characters.  
`_runs`| `B_RAW_TYPE`| Flattened run array.  
`_stylable`| `B_BOOL_TYPE`| `true` == is stylable.  
`_auto_in`| `B_BOOL_TYPE`| `true` == autoindent on  
`_wrap`| `B_BOOL_TYPE`| `true` == word wrapping on  
`_nsel`| `B_BOOL_TYPE`| `true` == not selectable  
`_nedit`| `B_BOOL_TYPE`| `true` == not editable  
  
Some of these fields may not be present if the setting they represent isn't used, or is the default value. For example, if word wrapping is off, the `_wrap` field won't be found in the archive.

<!-- Classes And Methods - The Interface Kit - BStringItem -->

## BStringItem

| Derived From:| [`BListItem`](BListItem.md "BListItem")  
---|---  
Mix-in Classes:| –  
Declared In:| `interface/ListItem.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BStringItem_Overview.md "BStringItem")

  * [Constructor and Destructor](BStringItem.md#BStringItem_ConstructorDestructor)
  * [Member Functions](BStringItem.md#BStringItem_MemberFunctions)
  * [Static Functions](BStringItem.md#BStringItem_StaticFunctions)
  * [Archived Fields](BStringItem.md#BStringItem_ArchivedFields)

  
  
* * *

### Constructor and Destructor

#### BStringItem()

`BStringItem(const char* text,  
uint32 level = 0,  
bool expanded = true);` `BStringItem(BMessage* archive);`

Initializes the `BStringItem` by making a copy of the text string passed as an argument. This is the string the item will display. The level and expanded arguments are passed unchanged to the `BListItem` constructor; see that function for an explanation.

#### ~BStringItem()

`virtual ~BStringItem();`

Frees the text the item displays.

* * *

### Member Functions

#### Archive()

`virtual status_t Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

Calls the inherited version of `Archive()` and stores the `BStringItem` in the [`BMessage`](BMessage.md "BMessage") archive.

See also: [`BArchivable::Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)"), [`Instantiate()`](BStringItem.md#BStringItem_Instantiate "Instantiate\(\)") static function

#### DrawItem()

`virtual void DrawItem([`BView`](BView.md "BView")* owner,  
BRect frame,  
bool complete = false);`

Draws the text string, dimming it if the item is disabled and highlighting it if the item is selected.

See also: [`BListItem::DrawItem()`](BListItem.md#BListItem_DrawItem "DrawItem\(\)")

####  SetText(), Text()

`virtual void SetText(const char* text);``const char* Text() const;`

These functions set and return the text that the `BStringItem` draws. `SetText()` copies the string it's passed. `Text()` returns a pointer to the string owned by the `BStringItem`.

#### Update()

`virtual void Update([`BView`](BView.md "BView")* owner,  
const [`BFont`](BFont.md "BFont")* font);`

Overrides the `BListItem` version of [`Update()`](BListItem.md#BListItem_Update "Update\(\)") to recalculate the width and height of the `BStringItem` and the placement of the text. The width of the item is based on the width of the owner [`BView`](BView.md "BView"). The height and text placement are based on the owner's font. The item must be tall enough to display the string in the current font.

* * *

### Static Functions

#### Instantiate()

`static [`BArchivable`](BArchivable.md "BArchivable")* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Returns a new `BStringItem` object, allocated by new and created with the version of the constructor that takes a [`BMessage`](BMessage.md "BMessage") archive. However, if the archive message doesn't contain archived data for a BStringItem, Instantiate() returns `NULL`.

See also [`BArchivable::Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)"), [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"), [`Archive()`](BStringItem.md#BStringItem_Archive "Archive\(\)")

* * *

### Archived Fields

The [`Archive()`](BStringItem.md#BStringItem_Archive "Archive\(\)") function adds the following fields to its [`BMessage`](BMessage.md "BMessage") argument:

Field| Type code| Description  
---|---|---  
`_label`| `B_STRING_TYPE`| String text (present only if it is not `NULL`).

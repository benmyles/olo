<!-- Classes And Methods - The Support Kit - BArchivable -->

## BArchivable

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `support/Archivable.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BArchivable_Overview.md "BArchivable")

  * [Constructor and Destructor](BArchivable.md#BArchivable_ConstructorDestructor)
  * [Member Functions](BArchivable.md#BArchivable_MemberFunctions)
  * [Static Functions](BArchivable.md#BArchivable_StaticFunctions)

  
  
* * *

### Constructor and Destructor

#### BArchivable()

`BArchivable();` `BArchivable([`BMessage`](BMessage.md "BMessage")* archive);`

Does nothing.

#### ~BArchivable()

`virtual ~BArchivable();`

Does nothing.

* * *

### Member Functions

#### Archive()

`virtual status_t Archive([`BMessage`](BMessage.md "BMessage")* archive,  
bool deep = true) const;`

The default implementation adds the name of the object's class to archive's `class` field. Derived classes must override `Archive()` to augment this implementation by adding, to the [`BMessage`](BMessage.md "BMessage"), data that describes the current state of the object. Each implementation of this function should begin by incorporating the inherited version:
    
    
    /* We'll assume that MyView inherits from [BView](BView.md "BView"). */
    status_t MyView::Archive([BMessage](BMessage.md "BMessage")* archive, bool deep)
    {
       [BView](BView.md "BView")::Archive(archive, deep);
       . . .
    }

If the class can be instantiated directly from a derived class, it should also add its name to the "class" array:
    
    
    archive->AddString("class", "MyView");

The `deep` flag declares whether `Archive()` should include objects that "belong" to the archiving object. For example, a deep [`BView`](BView.md "BView") archive would include archived forms of the view's children.

`Archive()` should return `B_OK` if it's successful; otherwise, it should return `B_ERROR` or a more descriptive error code.

* * *

### Static Functions

#### Instantiate()

`static BArchivable* Instantiate([`BMessage`](BMessage.md "BMessage")* archive);`

Derived classes should implement `Instantiate()` to return a new `BArchivable` object that was constructed from the [`BMessage`](BMessage.md "BMessage") archive. For example:
    
    
    BArchivable* TheClass::Instantiate([BMessage](BMessage.md "BMessage")* archive)
    {
       if ( !validate_instantiation(archive, "TheClass") )
          return NULL;
       return new TheClass(archive);}
    

Warning

![Warning](./images/admonitions/Stop_32.png)

`Instantiate()` must return a `BArchivable*`, regardless of the actual class in which it's implemented.

This function depends on a constructor that can initialize the new object from the `archive` [`BMessage`](BMessage.md "BMessage"). See "Instantiability" **TODO** for more information.

The default implementation returns `NULL`.

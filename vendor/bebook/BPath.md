<!-- Classes And Methods - The Storage Kit - BPath -->

## BPath

| Derived From:| [`BFlattenable`](BFlattenable.md "BFlattenable")  
---|---  
Mix-in Classes:| None  
Declared In:| `storage/Path.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BPath_Overview.md "BPath")

  * [Constructor and Destructor](BPath.md#BPath_ConstructorDestructor)
  * [Member Functions](BPath.md#BPath_Member_Functions)
  * [Operators](BPath.md#BPath_Operators)

  
  
* * *

### Constructor and Destructor

#### BPath()

`BPath();` `BPath(const [`BEntry`](BEntry.md "BEntry")* entry);` `BPath(const entry_ref* ref);` `BPath(const char* path,  
const char* leaf = NULL,  
bool normalize = false);` `BPath(const [`BDirectory`](BDirectory.md "BDirectory")* dir,  
const char* leaf = NULL,  
bool normalize = false);` `BPath(const BPath& path);`

Creates a new `BPath` object that represents the path that's created from the arguments. See the analogous [`SetTo()`](BPath.md#BPath_SetTo "SetTo\(\), Unset\(\)") functions for descriptions of the flavorful constructors.

  * The default constructor does nothing; it should be followed by a call to [`SetTo()`](BPath.md#BPath_SetTo "SetTo\(\), Unset\(\)").

  * The copy constructor makes a copy of the argument's pathname.

The constructor automatically allocates memory for the object's stored pathname. The memory is freed when the object is deleted.

To check to see if an initialization was successful, call [`InitCheck()`](BPath.md#BPath_InitCheck "InitCheck\(\)").

#### ~BPath

`virtual ~BPath();`

Frees the object's pathname storage and extinguishes the object.

* * *

### Member Functions

#### Append()

`status_t Append(const char* path,  
bool normalize = false);`

Appends the pathname given by `path` to the object's current pathname. `path` must be relative. If `normalize` is `true`, the new pathname is normalized; otherwise, it's normalized only if necessary.

Note that this…
    
    
    Append("subdir/file")

…is the same as (and is implemented as):
    
    
    path.SetTo(path.Path(), "subdir/file");

The `Append()` return value is picked up from the [`SetTo()`](BPath.md#BPath_SetTo "SetTo\(\), Unset\(\)") call.

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_BAD_VALUE`.| `path` contained a leading "/", or `this` is uninitialized.  
  
See [`SetTo()`](BPath.md#BPath_SetTo "SetTo\(\), Unset\(\)") for other return values.

#### GetParent()

`status_t GetParent(BPath* path) const;`

Initializes the argument with the pathname to the parent directory of `this`. Destructive parenting is acceptable (sociologically, it's a given):
    
    
    BPath path("/boot/lbj/fido");
    
    path.GetParent(&path);

Other details…

  * `GetParent()` makes a call to [`SetTo()`](BPath.md#BPath_SetTo "SetTo\(\), Unset\(\)"), but it's guaranteed not to tickle the normalization machine.

  * You can't get the parent of "/".

Return Code| Description  
---|---  
`B_OK`.| Hello, mother.  
`B_ENTRY_NOT_FOUND`.| You tried to get the parent of "/".  
`B_BAD_VALUE`.| `path` is `NULL`.  
`B_NO_MEMORY`.| Couldn't allocate storage for the pathname.  
  
If the initialization isn't successful, the argument's [`InitCheck()`](BPath.md#BPath_InitCheck "InitCheck\(\)") is set to `B_NO_INIT`.

#### InitCheck()

`status_t InitCheck() const;`

Returns the status of the most recent construction or [`SetTo()`](BPath.md#BPath_SetTo "SetTo\(\), Unset\(\)") call.

Return Code| Description  
---|---  
`B_OK`.| The initialization was successful.  
`B_NO_INIT`.| The object is uninitialized (this includes [`Unset()`](BPath.md#BPath_Unset)).  
For other errors.| See [`SetTo()`](BPath.md#BPath_SetTo "SetTo\(\), Unset\(\)")  
  
####  Path(), Leaf()

`const char* Path() const;``const char* Leaf() const;`

These functions return the object's full path and leaf name, respectively. For example:
    
    
    BPath path("/boot/lbj/fido");
    printf("Path: %sn", path.Path());
    printf("Leaf: %sn", path.Leaf());

Produces…
    
    
    $ Path: /boot/lbj/fido
    $ Leaf: fido
    

In both cases, the returned pointers belong to the `BPath` object. When the `BPath` is deleted, the pointers go with it.

If the `BPath` isn't initialized, the functions return pointers to `NULL`.

####  SetTo(), Unset()

`SetTo(const [`BEntry`](BEntry.md "BEntry")* entry);` `SetTo(const char* path,  
const char* leaf = NULL,  
bool normalize = false);` `SetTo(const [`BDirectory`](BDirectory.md "BDirectory")* dir,  
const char* leaf = NULL,  
bool normalize = false);`

`Unset();`

The `SetTo()` function frees the pathname that the object currently holds, and re-initializes the object according to the arguments:

  * The first version concatenates the `path` and `leaf` strings (interposing a "/" if necessary). If `path` is relative, the concatenated pathname is appended to the current working directory. Note that you don't have to split your pathname into two parts to call this constructor; the optional `leaf` argument is provided simply as a convenience.

  * The second version performs a similar operation using the path of the [`BDirectory`](BDirectory.md "BDirectory") as the initial part of the pathname.

  * The third version initilizes the object with the `path` and name of the `entry`.

Regarding the `leaf` argument:

  * The `leaf` string can contain directories—it needn't be just a leaf name.

  * However, `leaf` must be a relative pathname (it can't start with "/").

If set to `true`, the `normalize` argument tells the object to normalize the new pathname. By default (`false`), the pathname is normalized only if necessary. Note that the default doesn't mean that the object absolutely won't normalize, it just won't do it if it doesn't think it's necessary. See "[Initializing and Normalizing](BPath_Overview.md#BPath_Initializing_And_Normalizing "Initializing and Normalizing")" for the full story on normalizing a pathname, including the conditions that trigger default normalization. Normalizing has no meaning with the [`BEntry`](BEntry.md "BEntry") version of `SetTo()`.

Storage for the pathname is allocated by the `BPath` object and is freed when the object is deleted (or when you re-initialize through `SetTo()`). The path and leaf arguments are copied into the allocated storage.

Other details…

  * Destructive setting is safe:
        
        /* This works... */
        path.SetTo(path.Path(), ...);

  * Currently, `SetTo()` only checks pathname and filename length if it has to normalize.

`Unset()` frees the object's pathname storage and sets the [`InitCheck()`](BPath.md#BPath_InitCheck "InitCheck\(\)") value to `B_NO_INIT`.

Return Code| Description  
---|---  
`B_OK`.| Successful initialization.  
`B_BAD_VALUE`.| `path` is `NULL`, `leaf` isn't relative (it starts with a "/"), or `dir` is uninitialized.  
`B_BAD_VALUE`.| A directory in the path doesn't exist (normalization only).  
`B_NAME_TOO_LONG`.| A pathname element is too long (normalization only).  
`B_NO_MEMORY`.| Couldn't allocate storage for the pathname.  
  
The return value is also recorded in [`InitCheck()`](BPath.md#BPath_InitCheck "InitCheck\(\)").

#### BFlattenable Functions

The following functions are implemented in accordance with the rules set down by the [`BFlattenable`](BFlattenable.md "BFlattenable") class. You never need to invoke these functions directly; they're implemented so a `BPath` can added to a [`BMessage`](BMessage.md "BMessage") (see "[Passing a BPath in a BMessage](BPath_Overview.md#BPath_Passing_A_BPath_In_A_BMessage "Passing a BPath in a BMessage")"). But in case you're interested…

#### AllowsTypeCode()

`virtual bool AllowsTypeCode(type_code code) const;`

Returns `true` if `code` is `B_REF_TYPE`, and `false` otherwise.

#### Flatten()

`virtual status_t Flatten(void* buffer,  
ssize_t size) const;`

Converts the object's pathname to an entry_ref and writes it into `buffer`. Currently, `size` is ignored.

Return Code| Description  
---|---  
`B_OK`.| Peachy.  
`B_NAME_TOO_LONG`.| The pathname is too long (> 1024 characters).  
`B_ENTRY_NOT_FOUND`.| A directory in the path doesn't exist.  
  
#### FlattenedSize()

`virtual ssize_t FlattenedSize() const;`

Returns the size of the entry_ref that represents the flattened pathname.

#### IsFixedSize()

`virtual bool IsFixedSize() const;`

Returns `false`.

#### TypeCode()

`virtual type_code TypeCode() const;`

Returns `B_REF_TYPE`.

#### Unflatten()

`virtual status_t Unflatten(type_code code,  
const void* buffer,  
ssize_t size);`

Initializes the `BPath` with the flattened entry_ref data that's found in `buffer`. The type code must be `B_REF_TYPE`.

Return Code| Description  
---|---  
`B_OK`.| Success.  
`B_BAD_VALUE`.| Wrong type code (not `B_REF_TYPE`).  
`B_ENTRY_NOT_FOUND`.| A directory in the entry_ref data doesn't exist.  
  
The `Unflatten()` return value is recorded in [`InitCheck()`](BPath.md#BPath_InitCheck "InitCheck\(\)").

* * *

### Operators

#### = (assignment)

`BPath& operator=(const BPath& path);` `BPath& operator=(const char* string);`

Initializes this with a copy of the pathname that's gotten from the argument. Also sets [`InitCheck()`](BPath.md#BPath_InitCheck "InitCheck\(\)").

####  == , != (comparison)

`bool operator==(const BPath& path) const;` `bool operator==(const char* string) const;`

`bool operator!=(const BPath& path) const;` `bool operator!=(const char* string) const;`

Compares `this`'s pathname with the pathname taken from the argument. The comparison is a simple `strcmp();` neither path is normalized or otherwise altered before the comparison is made. For example:
    
    
    BPath path("/boot/lbj/fido");
    
    chdir("/boot");
    printf("Are they equal? %dn", path == "lbj/fido");

Displays:
    
    
    $ Are they equal? 0

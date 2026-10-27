<!-- System Overview - The Storage Kit - BPath -->

## BPath

A `BPath` object represents an absolute pathname, and provides some simple path manipulation and querying functions. The primary features of the class are:

  * It allocates storage for you. When you tell your `BPath` object which pathname you want it to represent, the object allocates storage for the pathname automatically. When you delete the object, the storage is freed.

  * It always represents an absolute path. The pathname strings that you use to initialize a `BPath` can be relative, and they can include references to "." and "..". The `BPath` "normalizes" the passed-in strings to create an absolute pathname, as described in "[Initializing and Normalizing](BPath_Overview.md#BPath_Initializing_And_Normalizing "Initializing and Normalizing")".

`BPath`s are handy, but don't expect them to actually do very much: A `BPath` is just a pathname. It identifies the location of a file, but it can't manipulate the file, nor can it change the structure of the file system.

* * *

### So what do you use BPaths for?

  * You can use your `BPath` to initialize other, more powerful objects ([`BEntry`](BEntry.md "BEntry"), [`BNode`](BNode.md "BNode") and its kids). See "[Converting a BPath](BPath_Overview.md#BPath_Converting_A_BPath "Converting a BPath")".

  * `BPath`s can be passed through [`BMessage`](BMessage.md "BMessage")s. To add a `BPath` to a [`BMessage`](BMessage.md "BMessage"), you have to flatten it first: `BPath` implements [`BFlattenable`](BFlattenable.md "BFlattenable") for exactly this reason. The receiver of the [`BMessage`](BMessage.md "BMessage") can resurrect the flattened object as a `BPath` object or as an entry_ref structure. See "[Passing a BPath in a BMessage](BPath_Overview.md#BPath_Passing_A_BPath_In_A_BMessage "Passing a BPath in a BMessage")".

  * `BPath` objects are ideal for caching references to files. `BPath`s don't consume much in the way of system resources—they don't contain file descriptors, for example. So they're great for keeping track of the files that your application is interested in.

In the way that they're used, `BPath`s and entry_refs are nearly identical. In particular, entry_refs can do all three of the things listed here. Whether you use `BPath`s (pathnames in general) or entry_refs is largely a matter of taste.

* * *

### Initializing and Normalizing

You initialize a `BPath`—in other words, you establish the path that the object represents—by passing a string (or two, or a [`BDirectory`](BDirectory.md "BDirectory") and a string) to the constructor or to the [`SetTo()`](BPath.md#BPath_SetTo "SetTo\(\), Unset\(\)") function. Upon initialization, the `BPath` object concatenates the strings and then "normalizes" the passed-in strings if it has to (this emphasis is important, as we'll see in a moment). The following elements trigger normalization:

  * a relative pathname (after concatenation; e.g. `boot/lbj`)

  * the presence of "." or ".." (`/boot/lbj/../lbj/./fido`)

  * redundant slashes (`/boot//lbj`)

  * a trailing slash (`/boot/lbj/`)

During normalization, `BPath` conjures up an absolute pathname in the form

  * `/dir1/dir2/.../dirN/leaf`

It does this by applying the following rules:

  * relative pathnames are reckoned off of the current working directory

  * "." is ignored (at the head of a path, it's taken as the cwd).

  * ".." bumps up one directory level

  * redundant slashes are coalesced

  * a trailing slash is removed.

(The one exception to this final rule is `/` as a full pathname.)

There's a subtle side-effect that you get with normalization: When you normalize a pathname, all the elements in the path up to but not including the leaf must exist. In other words, a normalized `BPath` object gives you the same guarantee of existence as does an entry_ref structure. The subtlety, here, is that an unnormalized `BPath` needn't exist at all.

For example, here we create a `BPath` for a pathname that contains a non-existent directory:
    
    
    /* We'll assume that "/abc/def/" doesn't exist. */
    BPath path("/abc/def/ghi.jkl");
    
    /* Nonetheless, the BPath is successfully initialized.
    * The Path() function returns a pointer to the object's
    * pathname string.
    */
    printf("Path: %sn". path.Path());

On the command line we see:
    
    
    $ Path: /abc/def/ghi.jkl
    

But if we tickle the normalization machine…
    
    
    /* The redundant slash causes a normalization. */
    BPath path("/abc/def//ghi.jkl");

…the object is invalid:
    
    
    $ Path: (null)
    

#### Forcing Initialization

Both the constructor and the [`SetTo()`](BPath.md#BPath_SetTo "SetTo\(\), Unset\(\)") function carry an optional argument that lets you force the passed-in path to be normalized:
    
    
    /* The trailing bool forces normalization. */
    BPath path("/abc/def/ghi.jkl", true);
    printf("Path: %sn", path.Path());

In this case, the forced normalization nullifies the object:
    
    
    $Path: (null)
    

#### Normalization by Default?

Since forcing normalization makes `BPath`'s behaviour more consistent and reliable, why not always normalize? Because normalization can be expensive.

During normalization, the pathname is `stat`'d and prodded rather heavily. If you're planning on using your `BPath`'s pathname to initialize a [`BEntry`](BEntry.md "BEntry") or [`BNode`](BNode.md "BNode"), this prodding will happen again. Rather than incur the expense twice, you may want to live with unnormalized `BPath` objects, and take the normalization hit during the subsequent initialization.

#### Other Normalization Details

  * You can't force the `BPath` constructor or [`SetTo()`](BPath.md#BPath_SetTo "SetTo\(\), Unset\(\)") function to skip the normalization. If the path needs to be normalized, it will be normalized.

  * `BPath` doesn't let you ask if its pathname was normalized.

* * *

### The BPath Calling Convention

`BPath` objects are passed back to you (by reference) by a number of Storage Kit functions. However, you shouldn't find any functions that ask for a `BPath` object. This is a convention of usage:

  * If an API element returns a pathname to you, it does so in the form of a `BPath`. If it asks for a pathname from you (as an argument), it asks for a const char*.

As an example of a function that returns a `BPath` to you, recall [`BEntry`](BEntry.md "BEntry")'s [`GetPath()`](BEntry.md#BEntry_GetPath) function:
    
    
    status_t BEntry::GetPath(BPath *path)

(As an aside, this is where the auto-allocation comes in handy—because `BPath` allocates the pathname storage for you, you don't have to mess around with ugly buffer and length arguments.)

On the other hand, [`BEntry`](BEntry.md "BEntry")'s [`SetTo()`](BEntry.md#BEntry_SetTo) takes a pathname as a const char*:
    
    
    status_t BEntry::SetTo(const char *path)

If you've got a `BPath` loaded up with a pathname, you would call this function thus:
    
    
    entry.SetTo(path.Path());

The constructors and [`SetTo()`](BPath.md#BPath_SetTo "SetTo\(\), Unset\(\)") functions in (most of) the Storage Kit classes have const char* versions that can be called as shown here.

* * *

### Passing a BPath in a BMessage

Let's say you've got a `BPath` object that you want to send to some other application. To do this, you have to add it to a [`BMessage`](BMessage.md "BMessage") object through the latter's [`AddFlat()`](BMessage.md#BMessage_AddFlat) function. As an inheritor from [`BFlattenable`](BFlattenable.md "BFlattenable") the `BPath` knows how to flatten itself for just this purpose.
    
    
    BMessage msg;
    BPath path("/boot/lbj/fido");
    
    /* The check here is important, as we'll describe
    * in a moment.
    */
    if (msg.AddFlat("pathname", &path) != B_OK)
       /* handle the error */

The receiver of the message can retrieve the pathname as a `BPath` object by calling [`FindFlat()`](BMessage.md#BMessage_FindFlat):
    
    
    void MyApp::MessageReceived(BMessage *msg)
    {
       BPath path;
    
       if (msg->FindFlat("pathname", &path) != B_OK)
          /* handle the error */
       ...
    }

Alternatively, the pathname can be retrieved as an entry_ref through [`FindRef()`](BMessage.md#BMessage_FindRef):
    
    
    void MyApp::MessageReceived(BMessage *msg)
    {
       entry_ref ref;
    
       if (msg->FindRef("pathname", &ref) != B_OK)
          /* handle the error */
       ...
    }

If you want to skip all the conversion business and simply pass the pathname as a string, use [`AddString()`](BMessage.md#BMessage_AddString). The receiver, of course, would have to call [`FindString()`](BMessage.md#BMessage_FindString) to retrieve your pathname string.

#### What's Really Going On

When you add a flattened `BPath` to a [`BMessage`](BMessage.md "BMessage"), the object's pathname is turned into an entry_ref. If the message receiver asks for a `BPath` (through [`FindFlat()`](BMessage.md#BMessage_FindFlat)), the entry_ref is turned back into a `BPath` object. Therefore, it's more efficient to retrieve a flattened `BPath` as an entry_ref than it is to unflatten it as a `BPath` object.

The `BPath` to entry_ref conversion has another, more subtle implication: Adding a `BPath` through [`AddFlat()`](BMessage.md#BMessage_AddFlat) performs an implicit normalization on the data that's added to the [`BMessage`](BMessage.md "BMessage").

If the normalization fails, the [`AddFlat()`](BMessage.md#BMessage_AddFlat) function returns an error and the data isn't added to the [`BMessage`](BMessage.md "BMessage"). The original `BPath` is untouched, regardless of the result of the normalization.

* * *

### Converting a BPath

As mentioned earlier, most of the Storage Kit classes have constructors and [`SetTo()`](BPath.md#BPath_SetTo "SetTo\(\), Unset\(\)") functions that accept const char* arguments. If you want to turn your `BPath` into a [`BFile`](BFile.md "BFile") (for example), you would do this (including error checks):
    
    
    status_t err;
    
    BFile file(path.Path());
    err = InitCheck();

or
    
    
    err = file.SetTo(path.Path());

To convert a `BPath` to an entry_ref, pass the pathname to the [`get_ref_for_path()`](BEntry.md#get_ref_for_path "get_ref_for_path\(\)") function:
    
    
    entry_ref ref;
    status_t err;
    
    err = get_ref_for_path(path.Path(), &ref);

For you Node Monitor users: You can't convert directly to a node_ref structure. The quickest way from here to there is:
    
    
    node_ref nref;
    status_t err;
    
    /* We'll skip InitCheck() and catch errors in GetNodeRef(). */
    BEntry entry(path.Path());
    err = entry.GetNodeRef(&nref);

* * *

### Immutability

Remember, a `BPath` represents a pathname, not a node. It isn't "updated" when the file system changes:

  * A `BPath`'s pathname string never changes behind your back, even if the entry that it originally pointed to is renamed, moved, or deleted.

For example:
    
    
    BEntry entry;
    BPath path;
    
    /* Set a BPath, construct a BEntry from it, rename
    * the entry, and then print the BPath's pathname.
    */
    if (path.SetTo("/boot/lbj/fido") == B_OK)
       if (entry.SetTo(&path) == B_OK)
          if (entry.Rename("rover") == B_OK)
             printf("Pathname: %sn", path.Path());

We see…
    
    
    $ Pathname: /boot/lbj/fido
    

...even though the entry that the `BPath` was constructed to represent has been renamed.

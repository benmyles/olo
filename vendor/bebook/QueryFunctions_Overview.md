<!-- System Overview - The Storage Kit - Query Functions -->

## Query Functions  
  
Normally, when you want to perform a file system query, you would use the [`BQuery`](BQuery.md "BQuery") class, which is a nice, clean, object-oriented way to do it. If you have an aversion to object-oriented programming, however, or you're writing a simple C program and would rather use C functions instead, then you've come to the right place.

Be aware that, currently, you can't perform "live" queries at the C level. If you want a live query, you have to use a [`BQuery`](BQuery.md "BQuery") object.

* * *

### Opening, Reading, and Closing a Query

To begin a query, you call [`fs_open_query()`](TheStorageKit_Functions.md#fs_open_query "fs_open_query\(\)"). [`fs_open_query()`](TheStorageKit_Functions.md#fs_open_query "fs_open_query\(\)") performs a "one-shot" query: It lets you ask for all the files that fulfill certain criteria right now. This criteria is expressed as a "predicate" string. This is a formula that lists the attribute values that you're interested in. A simple predicate takes the form
    
    
    _attribute op value_
    

For example, you can look for files that are larger than 5K by supplying a string that looks like this:
    
    
    "size > 5000"
    

Simple predicates can be grouped and combined to create more complex predicates. Here we look for files named "fido" that are greater than 5K:
    
    
    "(name = fido) && (size > 5000)"
    

For a full description of the predicate format, see "[The Predicate, Attributes, and Indices](BQuery_Overview.md#BQuery_Predicate_Attributes_Indices "The Predicate, Attributes, and Indices")" in the [`BQuery`](BQuery.md "BQuery") class description, but be aware of this difference:

  * The `fs_query` functions don't let you "push" elements onto the predicate; all predicates must be expressed as strings.

Once the query has been opened, you can step through the files that fulfill the predicate through iterated calls to [`fs_read_query()`](TheStorageKit_Functions.md#fs_read_query "fs_read_query\(\)"). When all the winning files have been visited, [`fs_read_query()`](TheStorageKit_Functions.md#fs_read_query "fs_read_query\(\)") returns `NULL`.

When you've finished using your query, you must close it by using the [`fs_close_query()`](TheStorageKit_Functions.md#fs_close_query "fs_close_query\(\)") function.

* * *

### An Example

The following sample demonstrates very briefly how to perform a simple, non-live query. In this example, we're searching for all C header files on the device specified by devnum.
    
    
    void sample_query(dev_t devnum) {
       DIR*      q;
       dirent_t* d;
    
       q = fs_open_query(devnum, "name=*.h", 0);
       if (q) {
          while (d = fs_read_query(q)) {
          ...
          }
          fs_close_query(q);
       }
    }

The code opens the query by calling [`fs_open_query()`](TheStorageKit_Functions.md#fs_open_query "fs_open_query\(\)"), then, calls [`fs_read_query()`](TheStorageKit_Functions.md#fs_read_query "fs_read_query\(\)") in a loop until `NULL` is returned. Once that happens, [`fs_close_query()`](TheStorageKit_Functions.md#fs_close_query "fs_close_query\(\)") is used to close the query.

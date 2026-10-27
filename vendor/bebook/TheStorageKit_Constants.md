<!-- Classes And Methods - The Storage Kit - Constants -->

## Constants

* * *

###  Limits Constants

Declared in: `storage/StorageDefs.h`

Constant| Description  
---|---  
`B_FILE_NAME_LENGTH`| Number of characters allowed in a file name.  
`B_PATH_NAME_LENGTH`| Number of characters allowed in a path name.  
`B_ATTR_NAME_LENGTH`| Number of characters allowed in an attribute name.  
`B_MIME_TYPE_LENGTH`| Number of characters allowed in a MIME type name.  
`B_MAX_SYMLINKS`| Number of nested symlinks allowed.  
  
These constants define the maximum values for several Storage Kit related items, including file and path name strings, attribute name strings, and MIME type strings. `B_MAX_SYMLINKS` specifies how many symbolic links may be linked through each other.

* * *

###  File Open Mode Constants

Declared in: `storage/StorageDefs.h`

Constant| Description  
---|---  
`B_READ_ONLY`| Open the file with read-only access.  
`B_WRITE_ONLY`| Open the file with write-only access.  
`B_READ_WRITE`| Open the file for both reading and writing.  
`B_FAIL_IF_EXISTS`| Don't open the file if it already exists.  
`B_CREATE_FILE`| Create the file before opening it.  
`B_ERASE_FILE`| Erase the previous contents before opening the file.  
`B_OPEN_AT_END`| Open with the pointer at the end of the file.  
  
These constants are used when opening files using either the POSIX `open()` function or using the [`BFile`](BFile.md "BFile") class. They specify the mode in which the file is to be opened. For instance, if write-only access is desired, and you want the operation to fail if the file exists, you would do the following:
    
    
    fd = open("foobar.data", B_WRITE_ONLY | B_FAIL_IF_EXISTS);

* * *

###  Node Flavors

Declared in: `storage/StorageDefs.h`

Constant| Description  
---|---  
`B_FILE_NODE`| Files only.  
`B_SYMLINK_NODE`| Symbolic links only.  
`B_DIRECTORY_NODE`| Directories only.  
`B_ANY_NODE`| Matches any node.  
  
These constants are used when making a request that can be qualified based on the "flavor" of a node—in other words, whenever you wish to perform an operation on only files, directories, or symbolic links. This is used when opening a file panel, for instance, so that you can specify what types of items you want the user to be able to select.

* * *

###  Icon Sizes

Declared in: `storage/Mime.h`

Constant| Description  
---|---  
`B_LARGE_ICON`| Large (32x32) icon.  
`B_MINI_ICON`| Small (16x16) icon.  
  
These constants are used when selecting icons from a meta MIME file; they let you fetch the large and small variations of a file's icon.

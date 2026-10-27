<!-- System Overview - The Storage Kit - Index Functions -->

## Index Functions

Before a query can be performed on a file system, indices need to be established in which to search. An index contains a list of files that can be searched given a particular attribute and the desired value.

An index directory is, essentially, a list of the indices on a particular disk; it keeps track of the attributes that can be searched upon. Each disk has its own index directory. For instance, a disk might have the following indices in its index directory:

  * MAIL:subject

  * MAIL:from

  * MAIL:to

  * MAIL:priority

Each of these is the name of an index that can be queried using either the `fs_query` API or the [`BQuery`](BQuery.md "BQuery") class.

For example, if an e-mail program wishes to allow searching on the name of the sender of a message, it needs to create an index for the "sender" attribute. Once this index is established, any file that has the "sender" attribute added to it will be added to the "sender" index.

Files which had the "sender" attribute attached before the "sender" index was created, however, will not be in the index until their "sender" attribute is updated, at which time they will be added to the index. For this reason, you should consider installing your indices when your application is installed, or when it is initially launched.

Since each disk has its own index directory, if you want all disks to have your indices available, you need to create them on each device. You can do this by using the functions described here in conjunction with the `fs_info` functions.

There are three indices that are present on every disk:

Index| Description  
---|---  
name| The name of the file.  
size| The size, in bytes, of the file.  
last_modified| The date the file was last changed.  
  
You can always perform queries in these indices. Their names are reserved; you can't create or remove indices by these names.

* * *

### The Index Directory

The following sample function opens the index directory for a specified device and, in a loop, reads every entry, printing their names to standard output. This presents a list of each index available on the disk.
    
    
    void ListIndex(int32 device)
    {
       DIR *d;
       index_info info;
       struct dirent *ent;
    
       d = fs_open_index_dir(device);
       if (!d) {
          fprintf(stderr, "Unable to open index.n");
          return;
       }
    
       while (ent = fs_read_index_dir(d)) {
          printf("%sn", ent->d_name);
       }
       fs_close_index_dir(d);
    }

After calling [`fs_open_index_dir()`](TheStorageKit_Functions.md#fs_open_index_dir "fs_open_index_dir\(\)") to open the index directory for the device passed into the function (and handling the error that might occur if that function fails), a while loop iterates through all the entries in the directory, calling [`fs_read_index_dir()`](TheStorageKit_Functions.md#fs_read_index_dir "fs_read_index_dir\(\)") to obtain the desired information, then printing that information to standard output.

When there are no indices left, [`fs_read_index_dir()`](TheStorageKit_Functions.md#fs_read_index_dir "fs_read_index_dir\(\)") returns `NULL` and the while loop exits. At this point, [`fs_close_index_dir()`](TheStorageKit_Functions.md#fs_close_index_dir "fs_close_index_dir\(\)") is called to close the index directory.

* * *

### Installing and Removing Indices

To install a new index, use the [`fs_create_index()`](TheStorageKit_Functions.md#fs_create_index "fs_create_index\(\)") function. For example, to create an index for the attribute "GOLF:Handicap" on device 4 you would do this:
    
    
    fs_create_index(4, "GOLF:Handicap", B_INT16_TYPE, 0);

This creates an empty index for the golf handicap attribute, which is stored as a 16-bit integer. Once the index has been created, any file that gets a "GOLF:Handicap" attribute added or changed will be indexed.

(You usually shouldn't hard-code a device number, of course; you can obtain a device number for a specific disk by using the `fs_info` functions or `stat()`.)

If you later need to remove the index, you can do so by calling [`fs_remove_index()`](TheStorageKit_Functions.md#fs_remove_index "fs_remove_index\(\)"), like this:
    
    
    fs_remove_index(4, "GOLF:Handicap");

You should be careful when deciding to delete an index. If the user still has files around that contain indexed attributes that they want to be able to search for—using the Find panel in the Tracker, for example—they will not be able to do so after the index has been removed. So you need to decide when it is appropriate to remove indices; the choice is yours, but choose wisely, or you might annoy users.

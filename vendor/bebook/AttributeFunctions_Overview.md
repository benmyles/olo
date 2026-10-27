<!-- System Overview - The Storage Kit - Attribute Functions -->

## Attribute Functions

* * *

### An Example

Before dropping straight into the reference on the `fs_attr` API, let's have a look at a simple sample of how to use some of its features. The sample code fragment below opens the attribute directory for a file named /boot/home/dirtylaundry and scans through all the attributes in the file, fetching their names and values.
    
    
    DIR *d;
    dirent_t *ent;
    attr_info info;
    int fd;
    char *buffer;
    
    d = fs_open_attr_dir("/boot/home/dirtylaundry");
    if (d) {
       while (ent = fs_read_attr_dir(d)) {
       fd = open("/boot/home/dirtylaundry", O_RDONLY);
       fs_stat_attr(fd, ent->d_name, &info);
       buffer = (char *) malloc((size_t) info.size);
       if (buffer)
          fs_read_attr(fd, ent->d_name, info.type, 0, buffer,
                info.size);
       ...
       close(fd);
       fs_close_attr_dir(d);
    }

This snippet begins by opening the attribute directory for the file, using the [`fs_open_attr_dir()`](TheStorageKit_Functions.md#fs_open_attr_dir "fs_open_attr_dir\(\), fs_fopen_attr_dir\(\)") function. If this is successful, it returns a pointer to a directory that contains the list of attributes. A while loop is used to read into ent each attribute from the attribute directory by calling [`fs_read_attr_dir()`](TheStorageKit_Functions.md#fs_read_attr_dir "fs_read_attr_dir\(\)"). The information this call provides includes the size and type of the attribute, as well as its name.

Once we know the name of the attribute, we can obtain the type and size of the attribute by calling [`fs_stat_attr()`](TheStorageKit_Functions.md#fs_stat_attr "fs_stat_attr\(\)"); now we have all the information needed to get the value of the attribute. After allocating a buffer to contain the value of the attribute, we pass the attribute's name (`ent`->`d_name`), and the type and size (`info.type` and `info.size`) into the [`fs_read_attr()`](TheStorageKit_Functions.md#fs_read_attr "fs_read_attr\(\)") function. The value of the attribute is stored in the buffer we specify.

This sample skimps a bit on error handling; you'll do better, of course.

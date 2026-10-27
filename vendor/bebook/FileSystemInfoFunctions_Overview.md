<!-- System Overview - The Storage Kit - File System Info Functions -->

## File System Info Functions

From time to time, it can be useful to know certain information about the capabilities of the file system on a device. While the BVolume class provides you easy access to this information, it can occasionally be helpful to have more direct access to this information.

This section describes three C functions which can be used to obtain information about the file system on a device. One of these functions, [`fs_stat_dev()`](TheStorageKit_Functions.md#fs_stat_dev "fs_stat_dev\(\)"), returns this information given a device number. The other two functions, [`dev_for_path()`](TheStorageKit_Functions.md#dev_for_path "dev_for_path\(\)") and [`next_dev()`](TheStorageKit_Functions.md#next_dev "next_dev\(\)"), provide two ways to obtain a device number for use with [`fs_stat_dev()`](TheStorageKit_Functions.md#fs_stat_dev "fs_stat_dev\(\)").

Note

![Note](./images/admonitions/Info_32.png)

These functions _don't_ set `errno`.

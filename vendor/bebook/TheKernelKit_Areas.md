<!-- Classes And Methods - The Kernel Kit - Areas -->

## Areas

Declared in:| `kernel/OS.h`  
---|---  
Library:| `libroot.so`  
  
An area is a chunk of virtual memory that can be shared between threads (possibly in different teams). If your application needs to allocate large chunks of memory, or wants to share lots of data with another application, you should consider using an area.

For more on area concepts, see "[Areas Overview](TheKernelKit_Areas_Overview.md "Areas")".

For examples of creating and sharing areas, see "[Area Examples](TheKernelKit_Areas_Overview.md#Areas_Examples "Area Examples")".

* * *

### Area Functions

#### area_for()

`area_id area_for(void* addr);`

Returns the area that contains the given address (within your own team's address space). The argument needn't be the starting address of an area, nor must it start on a page boundary: If the address lies anywhere within one of your application's areas, the ID of that area is returned.

Since the address is taken to be in the local address space, the area that's returned will also be local—it will have been created or cloned by your application.

Return Code| Description  
---|---  
`B_ERROR`.| The address doesn't lie within an area.  
  
See also: [`find_area()`](TheKernelKit_Areas.md#find_area "find_area\(\)")

#### clone_area()

`area_id clone_area(const char* clone_name,  
void** clone_addr,  
uint32 clone_addr_spec,  
uint32 clone_protection,  
area_id source_area);`

Creates a new area (the clone area) that maps to the same physical memory as an existing area (the source area).

Parameter| Description  
---|---  
`clone_name`| Is the name that you wish to assign to the clone area. Area names are, at most, `B_OS_NAME_LENGTH` characters long.  
`clone_addr`| Points to a value that gives the address at which you want the clone area to start; the pointed-to value must be a multiple of `B_PAGE_SIZE` (4096). The function sets the value pointed to by `clone_addr` to the area's actual starting address—it may be different from the one you requested. The constancy of *`clone_addr` depends on the value of `clone_addr_spec`, as explained next.  
`clone_addr_spec`| Is one of four constants that describes how clone_addr is to be interpreted. The first three constants, `B_EXACT_ADDRESS`, `B_BASE_ADDRESS`, and `B_ANY_ADDRESS`, have meanings as explained under [`create_area()`](TheKernelKit_Areas.md#create_area "create_area\(\)").The fourth constant, `B_CLONE_ADDRESS`, specifies that the address of the cloned area should be the same as the address of the source area. Cloning the address is convenient if you have two (or more) applications that want to pass pointers to each other—by using cloned addresses, the applications won't have to offset the pointers that they receive. For both the `B_ANY_ADDRESS` and `B_CLONE_ADDRESS` specifications, the value that's pointed to by the `clone_addr` argument is ignored.  
`clone_protection`| Is one or both of `B_READ_AREA` and `B_WRITE_AREA`. These have the same meaning as in [`create_area()`](TheKernelKit_Areas.md#create_area "create_area\(\)"); keep in mind, as described there, that a cloned area can have a protection that's different from that of its source.  
`source_area`| Is the area_id of the area that you wish to clone. You usually supply this value by passing an area name to the [`find_area()`](TheKernelKit_Areas.md#find_area "find_area\(\)") function.  
  
The cloned area inherits the source area's locking scheme.

Usually, the source area and clone area are in two different applications. It's possible to clone an area from a source that's in the same application, but there's not much reason to do so unless you want the areas to have different protections.

If `clone_area()` clone is successful, the clone's area_id is returned. Otherwise, it returns a descriptive error code, listed below.

Return Code| Description  
---|---  
`B_BAD_VALUE`.|  Bad argument value; you passed an unrecognized constant for `addr_spec` or `lock`, the `addr` value isn't a multiple of `B_PAGE_SIZE`, you set `addr_spec` to `B_EXACT_ADDRESS` or `B_CLONE_ADDRESS` but the address request couldn't be fulfilled, or `source_area` doesn't identify an existing area.   
`B_NO_MEMORY`.|  Not enough memory to allocate the system structures that support this area (unlikely).   
`B_ERROR`.|  Some other system error prevented the area from being created.   
  
#### create_area()

`area_id create_area(const char* name,  
void** addr,  
uint32 addr_spec,  
uint32 size,  
uint32 lock,  
uint32 protection);`

Creates a new area and returns its area_id.

Parameter| Description  
---|---  
`name`|  Is the name that you wish to assign to the area. It needn't be unique. Area names are, at most, `B_OS_NAME_LENGTH` (32) characters long.   
`addr`| Points to the address at which you want the area to start. The value of *`addr` must signify a page boundary; in other words, it must be an integer multiple of `B_PAGE_SIZE` (4096). Note that this is a pointer to a pointer: *`addr`—not `addr`—should be set to the desired address; you then pass the address of `addr` as the argument, as shown below:
    
    
    /* Set the address to a page boundary. */
    char *addr = (char *)(B_PAGE_SIZE * 100);
    
    /* Pass the address of addr as the second argument. */
    create_area( "my area", &addr, ...);

The function sets the value of *`addr` to the area's actual starting address—it may be different from the one you requested. The constancy of *`addr` depends on the value of `addr_spec`, as explained next.  
`addr_spec`| Is a constant that tells the function how the *`addr` value should be applied. There are three useful address specification constants, and one that doesn't apply here:| Constant| Description  
---|---  
`B_EXACT_ADDRESS`| You want the value of *`addr` to be taken literally and strictly. If the area can't be allocated at that location, the function fails.  
`B_BASE_ADDRESS`| The area can start at a location equal to or greater than *`addr`.  
`B_ANY_ADDRESS`| The starting address is determined by the system. In this case, the value that's pointed to by `addr` is ignored (going into the function).  
`B_ANY_KERNEL_ADDRESS`| The starting address is determined by the system, and the new area will belong to the kernel's team; it won't be deleted when the application quits. In this case, the value that's pointed to by `addr` is ignored (going into the function).  
`B_CLONE_ADDRESS`| This is only meaningful to the [`clone_area()`](TheKernelKit_Areas.md#clone_area "clone_area\(\)") function.  
`size`| Is the size, in bytes, of the area. The size must be an integer multiple of `B_PAGE_SIZE` (4096). The upper limit of size depends on the available swap space (or RAM, if the area is to be locked).  
`lock`| Describes how the physical memory should be treated with regard to swapping. There are four locking constants:| Constant| Description  
---|---  
`B_FULL_LOCK`| The area's memory is locked into RAM when the area is created, and won't be swapped out.  
`B_CONTIGUOUS`| Not only is the area's memory locked into RAM, it's also guaranteed to be contiguous. This is particularly—and perhaps exclusively—useful to designers of certain types of device drivers.  
`B_LAZY_LOCK`| Allows individual pages of memory to be brought into RAM through the natural order of things and then locks them.  
`B_NO_LOCK`| Pages are never locked, they're swapped in and out as needed.  
`B_LOMEM`| This is a special constant that's used for for areas that need to be locked, contiguous, and that fit within the first 16MB of physical memory. The folks that need this constant know who they are.  
`protection`| Is a mask that describes whether the memory can be written and read. You form the mask by adding the constants `B_READ_AREA` (the area can be read) and `B_WRITE_AREA` (it can be written). The protection you describe applies only to this area. If your area is cloned, the clone can specify a different protection.  
  
If `create_area()` is successful, the new area_id number is returned. If it's unsuccessful, one of the following error constants is returned.

Return Code| Description  
---|---  
`B_BAD_VALUE`.| Bad argument value. You passed an unrecognized constant for `addr_spec` or `lock`, the `addr` or `size` value isn't a multiple of `B_PAGE_SIZE`, or you set `addr_spec` to `B_EXACT_ADDRESS` but the address request couldn't be fulfilled.  
`B_NO_MEMORY`.| Not enough memory to allocate the system structures that support this area (unlikely), not enough physical memory to support a locked area, or not enough swap space to allocate virtual memory (in other words, size is too big.)  
`B_ERROR`.| Some other system error prevented the area from being created.  
  
#### delete_area()

`status_t delete_area(area_id area);`

Deletes the designated area. If no one other area maps to the physical memory that this area represents, the memory is freed. After being deleted, the `area` value is invalid as an area identifier.

Warning

![Warning](./images/admonitions/Stop_32.png)

Currently, anybody can delete any area—the act isn't denied if, for example, the area_id argument was created by another application. This freedom will be rescinded in a later release. Until then, try to avoid deleting other application's areas.

Return Code| Description  
---|---  
`B_OK`.| The area was deleted; `area` is now invalid.  
`B_ERROR`.| `area` doesn't designate an actual area.  
  
#### find_area()

`area_id find_area(const char* name);`

Returns an area that has a name that matches the argument. Area names needn't be unique—successive calls to this function with the same argument value may not return the same area_id.

What you do with the area you've found depends on where it came from:

  * If you're finding an area that your own application created or cloned, you can use the returned ID directly.

  * If the area was created or cloned by some other application, you should immediately clone the area (unless you're doing something truly innocuous, such as simply examining the area's info structure).

Return Code| Description  
---|---  
`B_NAME_NOT_FOUND`.| The argument doesn't identify an existing area.  
  
See also: [`area_for()`](TheKernelKit_Areas.md#area_for "area_for\(\)")

####  get_area_info(), get_next_area_info(), area_info

`status_t get_area_info(area_id area,  
area_info* info);``status_t get_next_area_info(team_id team,  
int32* cookie,  
area_info* info);`
    
    
    struct {} area_info

Copies information about a particular area into the area_info structure designated by `info`. The first version of the function designates the area directly, by area_id.

The `get_next_area_info()` version lets you step through the list of a team's areas through iterated calls on the function. The `team` argument identifies the team you want to look at; a `team` value of 0 means the team of the calling thread. The `cookie` argument is a placemark; you set it to 0 on your first call, and let the function do the rest. The function returns `B_BAD_VALUE` when there are no more areas to visit:
    
    
    /* Get the area_info for every area in this team. */
    area_info info;
    int32 cookie = 0;
    
    while (get_next_area_info(0, &cookie, &info) == B_OK)
       ...

The area_info structure is:
    
    
    typedef struct area_info {
        area_id area;
        char    name[B_OS_NAME_LENGTH];
        size_t  size;
        uint32  lock;
        uint32  protection;
        team_id team;
        size_t  ram_size;
        uint32  copy_count;
        uint32  in_count;
        uint32  out_count;
        void*   address;
    } area_info;

The fields are:

Field| Description  
---|---  
`area`| Is the area_id that identifies the area.  
`name`| Is the name that was assigned to the area when it was created or cloned.  
`size`| Is the (virtual) size of the area, in bytes.  
`lock`| Is a constant that represents the area's locking scheme. This will be one of `B_FULL_LOCK`, `B_CONTIGUOUS`, `B_LAZY_LOCK`, `B_NO_LOCK`, or `B_LOMEM`.  
`protection`| Specifies whether the area's memory can be read or written. It's a combination of `B_READ_AREA` and `B_WRITE_AREA`.  
`team`| Is the team_id of the team that created or cloned this area.  
`address`| Is a pointer to the area's starting address. Keep in mind that this address is only meaningful to the team that created (or cloned) the area.  
  
The final four fields give information about the area that's useful in diagnosing system use. The fields are particularly valuable if you're hunting for memory leaks:

Field| Description  
---|---  
`ram_size`| Gives the amount of the area, in bytes, that's currently swapped in.  
`copy_count`| Is a "copy-on write" count that can be ignored—it doesn't apply to the areas that you create. The system can create copy-on-write areas (it does so when it loads the data section of an executable, for example), but you can't.  
`in_count`| Is a count of the total number of times any of the pages in the area have been swapped in.  
`out_count`| Is a count of the total number of times any of the pages in the area have been swapped out.  
Return Code| Description  
---|---  
`B_OK`.|  The area was found; `info` contains valid information.   
`B_BAD_VALUE`.| `area` doesn't identify an existing area, `team` doesn't identify an existing team, or there are no more areas to visit.   
  
#### resize_area()

`status_t resize_area(area_id area,  
size_t new_size);`

Sets the size of the designated area to `new_size`, measured in bytes. The `new_size` argument must be a multiple of `B_PAGE_SIZE` (4096).

Size modifications affect the end of the area's existing memory allocation: If you're increasing the size of the area, the new memory is added to the end of area; if you're shrinking the area, end pages are released and freed. In neither case does the area's starting address change, nor is existing data modified (except, of course, for data that's lost due to shrinkage).

Resizing affects all areas that refer to this areas physical memory. For example, if B is a clone of A, and you resize A, B will be automatically resized (if possible).

Return Code| Description  
---|---  
`B_OK`.| The area was successfully resized.  
`B_BAD_VALUE`.| `area` doesn't signify a valid area, or `new_size` isn't a multiple of `B_PAGE_SIZE`.  
`B_NO_MEMORY`.| Not enough memory to support the new portion of the area. This should only happen if you're increasing the size of the area.  
`B_ERROR`.| Some other system error prevented the area from being created.  
  
#### set_area_protection()

`status_t set_area_protection(area_id area,  
uint32 new_protection);`

Sets the given area's read and write protection. The new_protection argument is a mask that specifies one or both of the values `B_READ_AREA` and `B_WRITE_AREA`. The former means that the area can be read; the latter, that it can be written to. An area's protection only applies to access to the underlying memory through that specific area. Different area clones that refer to the same memory may have different protections.

Return Code| Description  
---|---  
`B_OK`.|  The protection was changed.   
`B_BAD_VALUE`.| `area` doesn't identify a valid area.

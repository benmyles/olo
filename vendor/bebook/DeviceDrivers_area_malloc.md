<!-- Special Topics - Device Drivers - The area_malloc Module -->

## The area_malloc Module

Declared in: `drivers/area_malloc.h`

The area_malloc module provides a means for your driver to allocate memory in areas instead of on the heap. It provides [`malloc()`](DeviceDrivers_area_malloc.md#area_malloc_malloc "malloc\(\), calloc\(\), realloc\(\)"), [`calloc()`](DeviceDrivers_area_malloc.md#area_malloc_calloc), [`realloc()`](DeviceDrivers_area_malloc.md#area_malloc_realloc), and [`free()`](DeviceDrivers_area_malloc.md#area_malloc_free "free\(\)") functions that work just like their POSIX counterparts, except they require a pool argument as their first input.

Warning

![Warning](./images/admonitions/Stop_32.png)

These functions aren't safe to call from interrupt handlers; they may block on semaphores.

The area_malloc functions are thread-safe in relation to one another, but not in relation to [`delete_pool()`](DeviceDrivers_area_malloc.md#delete_pool). Be sure you don't call [`delete_pool()`](DeviceDrivers_area_malloc.md#delete_pool) on the pool you're using until you know none of the other functions might be called. [`create_pool()`](DeviceDrivers_area_malloc.md#create_pool "create_pool\(\), delete_pool\(\)") and [`delete_pool()`](DeviceDrivers_area_malloc.md#delete_pool) are safe in relation to each other.

When the last user of the module puts it away, any remaining pools are automatically deleted.

* * *

### Module Functions

####  create_pool(), delete_pool()

`const void* create_pool(uint32 addressSpec,  
size_t size,  
uint32 lockSpec,  
uint32 protection);``status_t delete_pool(const void* poolID);`

`create_pool()` creates a new pool of memory from which to allocate. The parameters are the same as those used by [`create_area()`](TheKernelKit_Areas.md#create_area "create_area\(\)"), so you have complete control over the area's characteristics (except for its name). Returns an opaque pool idenfityer, or `NULL` if the creation failed. The ability to share resources allocated from the pool is determined by the permissions and protections used to create the area.

`delete_pool()` deletes the pool specified by the opaque `poolID` given. Any pointers returned by the other functions in the module are immediately invalid. Returns `B_OK` if the pool was deleted, otherwise `B_ERROR`.

####  malloc(), calloc(), realloc()

`void* malloc(const void* poolID,  
size_t size);``void* calloc(const void* poolID,  
size_t numMembers,  
size_t size);``void* realloc(const void* poolID,  
void* ptr,  
size_t size);`

`malloc()` allocates a block of `size` bytes and returns a pointer to it.

`calloc()` allocates a block that can contain `numMembers` items of the specified `size` and returns a poiner to it.

`realloc()` resizes the memory block pointed to by `ptr` to the indicated `size`. Resizing a block can require that the memory be relocated, so this function returns the new pointer.

Each of these operations functions in the pool specified by `poolID`.

If there's not enough memory to allocate the requested block, these functions return `NULL`.

#### free()

`void* free(const void* poolID,  
void* ptr);`

Releases the memory block pointed to by `ptr` from the pool specified by `poolID`.

* * *

### Constants

#### B_AREA_MALLOC_MODULE_NAME
    
    
    #define B_AREA_MALLOC_MODULE_NAME "generic/area_malloc/v1"

The `B_AREA_MALLOC_MODULE_NAME` constant identifies the area_malloc module; use this constant to open the module.

<!-- Classes And Methods - The Media Kit - Real Time Functions -->

## Real Time Functions

Declared in:| `media/RealtimeAlloc.h`  
---|---  
Library:| `libmedia.so`  
  
Media nodes are highly timing-sensitive creatures. The slightest delay in performing their work can cause drastic problems in media playback or recording quality. Virtual memory, normally of great benefit to users, can work against them when doing media work. A poorly-timed virtual memory hit can cause breaks in media performances.

The realtime memory allocation and locking functions provide a means for nodes to lock down their memory to prevent it from being cached to disk by the virtual memory system. This avoids situations in which the node has to pause while it or its memory is fetched back from the swap file.

The user can use the Media preference application to configure what types of nodes should use locked memory. Nodes should typically use the realtime memory allocation functions instead of `malloc()` and `free()`. [`rtm_alloc()`](TheMediaKit_RealTimeFunctions.md#rtm_alloc "rtm_alloc\(\), rtm_realloc\(\)") will automatically handle locking the memory if the [`B_MEDIA_REALTIME_ALLOCATOR`](TheMediaKit_Constants.md#B_MEDIA_REALTIME_ALLOCATOR) flag is set, so your node doesn't have to worry about it.

* * *

### Memory Pools

Real-time memory blocks are allocated in pools. Pools are locked into physical RAM if realtime allocators are turned on in the [`BMediaRoster`](BMediaRoster.md "BMediaRoster"). While pools are locked in memory, that reduces the amount of physical RAM available to other applications, so don't use real-time blocks unless it's really going to benefit your performance.

When you create a pool, you specify the total size of the pool. This size is the maximum number of bytes that can be allocated from the pool. Memory can then be allocated out of the pool by calling [`rtm_alloc()`](TheMediaKit_RealTimeFunctions.md#rtm_alloc "rtm_alloc\(\), rtm_realloc\(\)") and memory blocks in the pool can be freed by calling [`rtm_free()`](TheMediaKit_RealTimeFunctions.md#rtm_free "rtm_free\(\)").

#### The Default Pool

There's a special pool, called the default (or shared) pool. You can allocate memory blocks in this pool if you wish, but it's a scarce resource and as a general rule you should avoid it.

Instead, you should create your own pool and use that; let the media nodes and Media Kit use the default pool for its own purposes.

#### MALLOC_DEBUG

[`rtm_alloc()`](TheMediaKit_RealTimeFunctions.md#rtm_alloc "rtm_alloc\(\), rtm_realloc\(\)") responds to the value of `MALLOC_DEBUG`. If the `MALLOC_DEBUG` environment variable is set to anything other than 0, new and freed blocks will be cleared to some junk value. If it's set to an integer greater than 1 it will also do pool, heap, and block validations at opportune times. It's not perfect validation, but it's better than nothing.

* * *

### Global C Functions

####  rtm_alloc(), rtm_realloc()

`void* rtm_alloc(rtm_pool* pool,  
size_t size);``status_t* rtm_realloc(void** data,  
size_t newSize);`

`rtm_alloc()` allocates a block of memory of size bytes in the specified pool. If you specify `NULL` for `pool`, the memory is allocated in the default pool.

`rtm_realloc()` resizes the block referenced by data to be `newSize` bytes long. Since the buffer may have to move to accomodate the new size, the pointer pointed to by `data` may change. If `data` is `NULL`, this is just like calling `rtm_alloc()` on the default pool.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_BAD_VALUE`| The `data` pointer is `NULL`.  
`B_NO_MEMORY`| The pool is full.  
  
####  rtm_create_pool(), rtm_delete_pool()

`status_t rtm_create_pool(rtm_pool** outPool,  
size_t totalSize,  
const char* name = NULL);``status_t rtm_delete_pool(rtm_pool* pool);`

`rtm_create_pool()` creates a new memory pool. The pool's rtm_pool reference is stored in `outPool`. The pool is capable of holding up to `totalSize` bytes of data, and is given the specified `name`.

If `outPool` is `NULL`, the default pool is created if it doesn't already exist. If it does, `EALREADY` is returned.

`rtm_delete_pool()` deletes the specified `pool`. You can't delete the default pool; passing `NULL` as pool will return `B_BAD_VALUE`.

Note

![Note](./images/admonitions/Info_32.png)

Be sure to delete a pool once you're finished with it.

Return Code| Description  
---|---  
`B_OK`| No error.  
`EALREADY`| The default pool already exists (`rtm_create_pool()` only).  
`B_BAD_VALUE`| You can't delete the default pool.  
Other Errors| [Area](TheKernelKit_Areas.md "Areas") errors.  
  
#### rtm_default_pool()

`rtm_pool* rtm_default_pool();`

Returns a pointer to the default pool, or `NULL` if it hasn't been initialized yet.

#### rtm_free()

`status_t rtm_free(void* data);`

Frees the specified block of data.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_BAD_VALUE`| A `NULL` pointer was given.  
  
####  rtm_size_for(), rtm_phys_size_for()

`status_t rtm_size_for(void* data);``status_t rtm_phys_size_for(void* data);`

`rtm_size_for()` returns the size of the specified block of memory.

`rtm_phys_size_for()` returns the physical size of the specified block of memory. This is the maximum number of bytes the block can grow to without risking [`rtm_realloc()`](TheMediaKit_RealTimeFunctions.md#rtm_realloc) actually moving the block; the physical size is always greater than or equal to the block size.

Return Code| Description  
---|---  
`B_OK`| No error.  
`B_BAD_VALUE`| The `data` pointer is `NULL`.

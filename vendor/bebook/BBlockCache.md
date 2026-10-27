<!-- Classes And Methods - The Support Kit - BBlockCache -->

## BBlockCache

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `support/BlockCache.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BBlockCache_Overview.md "BBlockCache")

  * [Constructor and Destructor](BBlockCache.md#BBlockCache_ConstructorDestructor)
  * [Member Functions](BBlockCache.md#BBlockCache_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BBlockCache()

`BBlockCache(size_t count,  
size_t size,  
uint32 type);`

Creates a new memory block pool, allocating memory for `count` blocks, each controlling `size` bytes of memory. `type` is either:

Constant| Description  
---|---  
`B_OBJECT_CACHE`| Memory is managed through `new` and `delete`.  
`B_MALLOC_CACHE`| Memory is managed through `malloc()` and `free()`.  
  
#### ~BBlockCache()

`~BBlockCache();`

Frees any unused memory in the object's block pool. Memory that was retrieved through `Get()` (and that hasn't been returned through `Save()`) is not deallocated.

* * *

### Member Functions

#### Get()

`void* Get(size_t size);`

Retrieves a block of memory of the given `size` and returns it directly. If `size` is the same as the `size` argument you passed to the constructor, the memory returned will be taken from the object's cache. Otherwise, it's allocated using either `new` or `malloc()` as requested in the constructor. When you're done with the memory, you can either deallocate it yourself, or return it to the `BBlockCache` object by calling `Save()`.

#### Save()

`void Save(void* pointer,  
size_t size);`

Returns, to the `BBlockCache` object, `size` bytes of memory pointed to by `pointer`. If the memory was taken from the object's pool, the memory is returned to the pool. Otherwise, it's deallocated. In either case, the caller is freed of responsibility for deallocating the memory.

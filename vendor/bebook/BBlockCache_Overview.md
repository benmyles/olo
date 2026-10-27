<!-- System Overview - The Support Kit - BBlockCache -->

## BBlockCache

The [`BBlockCache`](BBlockCache.md "BBlockCache") class pre-allocates a pool of fixed-length blocks of memory. You call [`Get()`](BBlockCache.md#BBlockCache_Get "Get\(\)") to retrieve a block of memory from the pool, and [`Save()`](BBlockCache.md#BBlockCache_Save "Save\(\)") to return it. [`BBlockCache`](BBlockCache.md "BBlockCache") objects are useful in applications that frequently allocate and deallocate fixed-sized objects, and in the implementations of a class' new and delete operators.

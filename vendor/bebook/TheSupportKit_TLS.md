<!-- Classes And Methods - The Support Kit - Thread Local Storage -->

## Thread Local Storage

Declared in:| `support/TLS.h`  
---|---  
Library:| `libbe.so`  
  
* * *

### Thread Local Storage Functions

#### tls_allocate()

`int32 tls_allocate();`

Creates and returns a new TLS variable. Each thread in your application will have its own copy of this variable; setting the value of the variable (through [`tls_set()`](TheSupportKit_TLS.md#tls_set "tls_set\(\), tls_get\(\)")) in one thread won't affect its value in any other thread. You should only allocate a TLS variable once per team—you don't have to allocate it in each thread.

Important

![Important](./images/admonitions/Warning_32.png)

The memory for the variable's data isn't allocated by `tls_allocate()`; see [`tls_set()`](TheSupportKit_TLS.md#tls_set "tls_set\(\), tls_get\(\)") for more information on TLS memory allocation).

#### tls_address()

`void* tls_address(int32 tlsVariable);`

Returns the address of the memory that holds the value that's referred to by `tlsVariable`. `tlsVariable` is a TLS variable that must have been created through [`tls_allocate()`](TheSupportKit_TLS.md#tls_allocate "tls_allocate\(\)"). The value returned is valid for the calling thread only.

####  tls_set(), tls_get()

`void tls_set(int32 tlsVariable,  
void* data);``void* tls_get(int32 tlsVariable);`

`tls_set()` sets `tlsVariable` to refer to `data` within the context of the calling thread. `data`, which must already be allocated, can point to data of any size or type. The data needn't be the same type or size for each thread; for example, a given TLS variable `hodgePodge` can refer to a string in one thread, an integer in another, an object in another, and so on.

`tls_get()` returns a pointer to the data referred to by `tlsVariable` within the context of the calling thread. If `tlsVariable` hasn't been allocated ([`tls_allocate()`](TheSupportKit_TLS.md#tls_allocate "tls_allocate\(\)")), or if its value hasn't been set (`tls_set()`), the function returns `NULL`.

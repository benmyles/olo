<!-- System Overview - The Support Kit - Thread Local Storage -->

## Thread Local Storage

The Thread Local Storage (TLS) functions let you create global variables that refer to different data depending on the thread from which the variables are referenced. In essence, TLS variables let you extend the data that's associated with a given thread. This can be particularly important when you're porting code that wasn't designed for a multi-threaded system.

To create a TLS variable, you call [`tls_allocate()`](TheSupportKit_TLS.md#tls_allocate "tls_allocate\(\)"):
    
    
    int32 myName = tls_allocate();
    int32 myStatus = tls_allocate();

You only allocate a given TLS variable once (i.e. you don't allocate the same variable in each thread). Typically, you would call [`tls_allocate()`](TheSupportKit_TLS.md#tls_allocate "tls_allocate\(\)") from an app-initialization routine.

To set and retrieve the value of a TLS variable, you call [`tls_set()`](TheSupportKit_TLS.md#tls_set "tls_set\(\), tls_get\(\)") and [`tls_get()`](TheSupportKit_TLS.md#tls_get):
    
    
    void SetName(const char *name) {
       tls_set(myName, (void *)name);
    }
    
    void SetStatus(int32 state) {
       tls_set(myStatus, (void *)(int32 *)&state);
    }
    
    void Report() {
       if (tls_get(myStatus) != B_OK) {
          printf("Error in %sn", tls_get(myName));
    }

The values that [`tls_set()`](TheSupportKit_TLS.md#tls_set "tls_set\(\), tls_get\(\)") and [`tls_get()`](TheSupportKit_TLS.md#tls_get) set and return are thread-specific; in the examples above, each thread has its own values for `myName` and `myStatus`. Note that [`tls_set()`](TheSupportKit_TLS.md#tls_set "tls_set\(\), tls_get\(\)") and [`tls_get()`](TheSupportKit_TLS.md#tls_get) operate in the context of the calling thread only—you can't set or retrieve a TLS value for some other thread.

Warning

![Warning](./images/admonitions/Stop_32.png)

You never access a TLS variable (i.e. the value returned by [`tls_allocate()`](TheSupportKit_TLS.md#tls_allocate "tls_allocate\(\)")) directly. You may only use it as an argument to the other TLS functions.

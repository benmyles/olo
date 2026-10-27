<!-- System Overview - The Storage Kit - Functions -->

## Functions

The next four sections document the straight C interface to attributes, indices, queries, and file system information. Technically, these functions are part of the Kernel Kit—their definitions live in header files in be/kernel, and their code is in `libroot.so`.

These functions use a global error variable (an integer), called `errno`, to register errors. You can look at the `errno` value directly in your code after a file system function fails. Alternatively, you can use the `errno()` function which prints, to standard error, its argument followed by a system-generated string that describes the current state of `errno`.

  * Each thread maintains its own `errno` variable.

  * `errno` is only set if there's an error—it never indicates success.

  * `errno` is never cleared. If call A fails and then you call B, C, and D, `errno` will still record the error from A.

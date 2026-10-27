<!-- System Overview - The Support Kit - BMallocIO -->

## BMallocIO

[`BMallocIO`](BMallocIO.md "BMallocIO") represents a buffer of dynamically allocated memory. A [`BMallocIO`](BMallocIO.md "BMallocIO") object allocates the buffer when you first call [`Write()`](BPositionIO.md#BPositionIO_Write "Write\(\), WriteAt\(\)") or [`WriteAt()`](BMallocIO.md#BMallocIO_WriteAt "WriteAt\(\)"). On subsequent calls, [`BMallocIO`](BMallocIO.md "BMallocIO") makes sure enough memory is allocated to hold all the data you intend to write, reallocating it if necessary. Memory is allocated in multiples of a block size that you can set.

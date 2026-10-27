<!-- System Overview - The Network Kit - BNetBuffer -->

## BNetBuffer

The [`BNetBuffer`](BNetBuffer.md "BNetBuffer") class provides an easy way to construct network buffers consisting of any sort of data, for use by the [`BNetEndpoint`](BNetEndpoint.md "BNetEndpoint") class.

Once you've created a [`BNetBuffer`](BNetBuffer.md "BNetBuffer"), you can append data to it by using a series of functions designed to add various types of data. For example, to create a buffer and place the long integer 2 followed by the string "This is a test." in it, you could do this:
    
    
    BNetBuffer buffer(512);
    buffer.AppendInt32(2);
    buffer.AppendString("This is a test.");

The [`AppendInt32()`](BNetBuffer.md#BNetBuffer_AppendInt32) function automatically handles conversion of the value into network byte order, as do all of the `AppendXXX()` functions for integer values (16-bit, 32-bit, and 64-bit, signed or unsigned). Likewise, the `RemoveXXX()` functions peel data out of a buffer, and they too are endian-aware.

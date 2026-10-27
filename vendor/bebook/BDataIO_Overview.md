<!-- System Overview - The Support Kit - BDataIO -->

## BDataIO

[`BDataIO`](BDataIO.md "BDataIO") is an abstract class that defines protocols for objects that can [`Read()`](BDataIO.md#BDataIO_Read "Read\(\)") and [`Write()`](BDataIO.md#BDataIO_Write "Write\(\)") data. [`BPositionIO`](BPositionIO.md "BPositionIO"), [`BDataIO`](BDataIO.md "BDataIO")'s main subclass, declares an additional set of functions ([`ReadAt()`](BPositionIO.md#BPositionIO_ReadAt), [`WriteAt()`](BPositionIO.md#BPositionIO_WriteAt), [`Seek()`](BPositionIO.md#BPositionIO_Seek "Seek\(\), Position\(\)"), and [`Position()`](BPositionIO.md#BPositionIO_Position)) for objects that can keep track of the current position in the I/O buffer; it implements [`Read()`](BDataIO.md#BDataIO_Read "Read\(\)") and [`Write()`](BDataIO.md#BDataIO_Write "Write\(\)") in terms of these other functions.

Neither [`BDataIO`](BDataIO.md "BDataIO") nor [`BPositionIO`](BPositionIO.md "BPositionIO") declares any data members, nor do they implement the functions in the protocols they declare. It's up to derived classes to implement them based on the properties of the particular kinds of data sources/repositories they represent.

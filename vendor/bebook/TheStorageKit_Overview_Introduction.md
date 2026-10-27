<!-- System Overview - The Storage Kit - Introduction -->

## Introduction

The Storage Kit is a set of C++ classes and C functions that let you access the file system; in particular, the kit lets you…

  * navigate the file hierarchy

  * read and write files

  * monitor changes to specific files (the Node Monitor)

  * ask for a set of files based on their characteristics (queries and attributes)

The basic functionality of the kit—such as reading and writing files—is also provided by the Be-supported POSIX functions (`open()`, `read()`, `write()`, `close()`, and so on) such that if you don't want to use the Storage Kit, you don't have to. But keep in mind that the Storage Kit's classes are not covers for POSIX functions. The POSIX functions and the Storage Kit classes are separate branches that dissolve into the same implementation—the one isn't implemented in terms of the other. POSIX is straight C, so it avoids the C++ overhead; but language efficiency aside, the POSIX functions aren't innately faster than the Storage Kit class functions.

The Storage Kit includes some additional C functions (beyond the standard POSIX implementation) so that almost everything that can be done from the C++ level can also be done in C, and vice versa. There are a few exceptions, notably these two:

  * You can't create a file system index from the C++ classes.

  * You can't create a live query with the C functions.

The next few sections provide some basic Storage Kit concepts, but most of the interesting stuff is in the class descriptions. The C functions that "imitate" the C++ layer are described at the end of the chapter.

Note

![Note](./images/admonitions/Info_32.png)

The standard POSIX functions are not documented in this book. Refer to an authorized POSIX pamphlet if you're interested in learning more about these functions.

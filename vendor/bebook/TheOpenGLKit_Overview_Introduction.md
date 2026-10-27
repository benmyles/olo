<!-- System Overview - The OpenGL Kit - Introduction -->

## Introduction

The OpenGL Kit provides an interface between your BeOS application and the OpenGL graphics library, which is provided with BeOS. The one class in the OpenGL Kit, [`BGLView`](BGLView.md "BGLView"), lets you display graphics rendered using OpenGL on a computer running the BeOS.

Note

![Note](./images/admonitions/Info_32.png)

Before Release 4.5, the OpenGL Kit also had a `BGLScreen` class. `BGLScreen` functionality has been subsumed by [`BGLView`](BGLView.md "BGLView"). Old `BGLScreen` code will still run, but new OpenGL Kit code must use [`BGLView`](BGLView.md "BGLView") objects only.

The [`BGLView`](BGLView.md "BGLView") class is used to create a view within a window that contains OpenGL-rendered data. Derived from [`BView`](BView.md "BView"), it adds functions for locking and unlocking the OpenGL context associated with the view, as well as for copying pixel data into and out of the graphics buffer, and swapping the front and back buffers.

If you want to use OpenGL graphics in a [`BDirectWindow`](BDirectWindow.md "BDirectWindow"), create your [`BDirectWindow`](BDirectWindow.md "BDirectWindow") and attach a [`BGLView`](BGLView.md "BGLView") to it.

* * *

### OpenGL On BeOS

BeOS has included an OpenGL implementation since the first Preview Release for PowerPC processors. This implementation of OpenGL is complete, and GLU is supported as well. The optional AUX and GLUT libraries, however, aren't supported at this time.

Also, the current implementation of OpenGL on BeOS supports only 32-bit graphic buffers. Your [`BGLView`](BGLView.md "BGLView") or BGLScreen can be in any graphics mode you want, but the graphics buffer offscreen is always 32-bit.

Complete descriptions of the features and use of OpenGL are beyond the scope of this book; however, you can get full documentation of OpenGL as well as sample code at the OpenGL web site at <http://www.opengl.org>.

There are also some sample OpenGL programs that have already been ported to BeOS available for download on the Be web site; visit http://www.be.com/developers/topics/opengl.html.

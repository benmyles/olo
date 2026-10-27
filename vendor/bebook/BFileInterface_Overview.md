<!-- System Overview - The Media Kit - BFileInterface -->

## BFileInterface

A node that can read data from, or write data to, a disk file should derive from [`BFileInterface`](BFileInterface.md "BFileInterface") in order to allow applications to easily specify what file the node should work with. The node will then be called upon by the Media Server to try to identify, and possibly work with, unknown files.

Your node can't just derive from [`BFileInterface`](BFileInterface.md "BFileInterface"); it must also derive from at least one of [`BBufferConsumer`](BBufferConsumer.md "BBufferConsumer") and [`BBufferProducer`](BBufferProducer.md "BBufferProducer"), depending on its needs.

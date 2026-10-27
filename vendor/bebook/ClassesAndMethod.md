<!-- Classes And Methods -->

# Classes And Methods

  * [The Application Kit](TheApplicationKit.md)
  * [The Device Kit](TheDeviceKit.md)
  * [The Game Kit](TheGameKit.md)
  * [The Input Server](TheInputServer.md)
  * [The Interface Kit](TheInterfaceKit.md)
  * [The Kernel Kit](TheKernelKit.md)
  * [The Mail Kit](TheMailKit.md)
  * [The Media Kit](TheMediaKit.md)
  * [The Midi Kit](TheMidiKit.md)
  * [The Network Kit](TheNetworkKit.md)
  * [The OpenGL Kit](TheOpenGLKit.md)
  * [The Storage Kit](TheStorageKit.md)
  * [The Support Kit](TheSupportKit.md)
  * [The Translation Kit](TheTranslationKit.md)

Classes and Methods provides detailed information on the class methods, global functions provided by the kits

Each section details the following information

Constructor
    

When you create a instance of a class this method is automatically called to perform the task.

Destructor
    

When you delete an instance of a class or the class variable goes out of scope this method is called automatically by the system.

Hook functions
    

Hook functions are the BeOS version of 'Don't call us, we'll call you.' These methods are called by the system when various events occur. All you need to do is provide your own version of these methods if the standard version does not do what you require.

Member functions
    

The normal methods you call in your own code.

Global Variables
    

Global convenience variables.

[Archive fields](TheSupportKit_Archiving.md "Archiving")
    

Archive fields are added to a [`BMessage`](BMessage.md "BMessage") before it is archived to store the current state of an object instance so that it can be rebuilt at a later time.

[Scripting Suites and Properties](TheApplicationKit_Scripting.md "Scripting")
    

If a class can be scripted then the suites and properties of the class will be described here.

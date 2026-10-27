<!-- System Overview - The Media Kit - BControllable -->

## BControllable  
  
A node whose behavior can be controlled by the user should be derived from [`BControllable`](BControllable.md "BControllable"), as well as from whatever other interface classes the node might be derived from. Deriving from [`BControllable`](BControllable.md "BControllable") lets the node publish information about the parameters that can be adjusted and how they relate to each other.

A client application can use the published information to build a user interface for the node or pass the information through to a system routine that will build the interface.

A node can also have the ability to start its own control panel under outside control, which can take advantage of special knowledge of the node.

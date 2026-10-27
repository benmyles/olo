<!-- Classes And Methods - The Media Kit - BSmallBuffer -->

## BSmallBuffer

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `media/SmallBuffer.h`  
Library:| `libmedia.so`  
Allocation:| Constructor only  
[Class Overview](BSmallBuffer_Overview.md "BSmallBuffer")

  * [Constructor and Destructor](BSmallBuffer.md#BSmallBuffer_ConstructorDestructor)
  * [Static Functions](BSmallBuffer.md#BSmallBuffer_StaticFunctions)

  
  
* * *

### Constructor and Destructor

#### BSmallBuffer()

##### protected

`BSmallBuffer();`

Constructor. Creates a small buffer that can hold up to [`SmallBufferSizeLimit()`](BSmallBuffer.md#BSmallBuffer_SmallBufferSizeLimit "BroadcastTimeWarp\(\)") bytes.

* * *

### Static Functions

#### BroadcastTimeWarp()

`size_t* SmallBufferSizeLimit();`

Returns the maximum number of bytes a `BSmallBuffer` can contain.

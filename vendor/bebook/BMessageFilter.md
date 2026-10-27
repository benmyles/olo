<!-- Classes And Methods - The Application Kit - BMessageFilter -->

## BMessageFilter

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `app/MessageFilter.h`  
Library:| `libbe.so`  
Allocation:| Constructor only  
[Class Overview](BMessageFilter_Overview.md "BMessageFilter")

  * [Constructor and Destructor](BMessageFilter.md#BMessageFilter_ConstructorDestructor)
  * [Hook Functions](BMessageFilter.md#BMessageFilter_HookFunctions)
  * [Member Functions](BMessageFilter.md#BMessageFilter_MemberFunctions)
  * [Operators](BMessageFilter.md#BMessageFilter_Operators)
  * [Defined Types](BMessageFilter.md#BMessageFilter_DefinedTypes)

  
  
* * *

### Constructor and Destructor

#### BMessageFilter()

`BMessageFilter(message_delivery delivery,  
message_source source,  
uint32 command,  
filter_hook filter = NULL);` `BMessageFilter(message_delivery delivery,  
message_source source,  
filter_hook filter = NULL);` `BMessageFilter(uint32 command,  
filter_hook filter = NULL);` `BMessageFilter(const BMessageFilter& object);` `BMessageFilter(const BMessageFilter* object);`

Creates and returns a new `BMessageFilter`. The first three arguments define the types of messages that the object wants to see:

Parameter| Description  
---|---  
`delivery`| Specifies how the message must arrive: drag-and-drop (`B_DROPPED_DELIVERY`), programmatically (`B_PROGRAMMED_DELIVERY`), or either (`B_ANY_DELIVERY`). The default is `B_ANY_DELIVERY`.  
`source`| Specifes whether the sender of the message must be local vis-a-vis this app (`B_LOCAL_SOURCE`), remote (`B_REMOTE_SOURCE`), or either (`B_ANY_SOURCE`). The default is `B_ANY_SOURCE`.  
`command`| Is a command constant. If supplied, the `what` value of the incoming message must match this value.  
  
Messages that don't fit the definition won't be sent to the object's filter function.

The `filter` argument is a pointer to a [`filter_hook`](BMessageFilter.md#filter_hook "filter_hook, filter_result") function. This is the function that's invoked when a message needs to be examined (see [`filter_hook`](BMessageFilter.md#filter_hook "filter_hook, filter_result") for the protocol). You don't have to supply a [`filter_hook`](BMessageFilter.md#filter_hook "filter_hook, filter_result") function; instead, you can implement `BMessageFilter`'s [`Filter()`](BMessageFilter.md#BMessageFilter_Filter "Filter\(\)") function in a subclass.

For more information, refer to the description of the member [`Filter()`](BMessageFilter.md#BMessageFilter_Filter "Filter\(\)") function.

#### ~BMessageFilter()

`virtual ~BMessageFilter()();`

Does nothing.

* * *

### Hook Functions

#### Filter()

`virtual [filter_result](BMessageFilter.md#filter_result) Filter([`BMessage`](BMessage.md "BMessage")* message,  
[`BHandler`](BHandler.md "BHandler")** target);`

Implemented by derived classes to examine an arriving message just before it's dispatched. The first two arguments are the `message` that's being considered, and the proposed [`BHandler`](BHandler.md "BHandler") `target`. You can alter the contents of the message, and alter or even replace the handler. If you replace the handler, the new handler must belong to the same looper as the original. The new handler is given an opportunity to filter the message before it's dispatched.

The return value must be one of these two values:

Constant| Description  
---|---  
`B_DISPATCH_MESSAGE`.| The `message` and `handler` are passed (by the caller) to the looper's [`DispatchMessage()` ](BLooper.md#BLooper_DispatchMessage "DispatchMessage\(\)") function.  
`B_SKIP_MESSAGE`.| The message goes no further—it's immediately thrown away by the caller.  
  
The default version of this function returns `B_DISPATCH_MESSAGE`.

It's possible to call your `Filter()` function yourself (i.e. outside the message-passing mechanism), but keep in mind that it's the caller's responsibility to interpret the return value.

Rather than implement the function, you can supply the `BMessageFilter` with a [`filter_hook`](BMessageFilter.md#filter_hook "filter_hook, filter_result") callback when you construct the object. If you do both, the [`filter_hook`](BMessageFilter.md#filter_hook "filter_hook, filter_result") (and not `Filter()`) will be invoked when the object is asked to examine a message.

* * *

### Member Functions

####  Command(), FiltersAnyCommand()

`uint32 Command() const;``bool FiltersAnyCommand() const;`

`Command()` returns the command constant (the [`BMessage`](BMessage.md "BMessage") `what` value) that an arriving message must match for the filter to apply. `FiltersAnyCommand()` returns `true` if the filter applies to all messages, and `false` if it's limited to a specific command.

Because all command constants are valid, including negative numbers and 0, `Command()` returns a reliable result only if `FiltersAnyCommand()` returns `false`.

#### Looper()

`[`BLooper`](BLooper.md "BLooper")* Looper() const;`

Returns the [`BLooper`](BLooper.md "BLooper") whose messages this object filters, or `NULL` if the `BMessageFilter` hasn't yet been assigned to a [`BHandler`](BHandler.md "BHandler") or [`BLooper`](BLooper.md "BLooper"). To attach a `BMessageFilter` to a looper or handler, use [`BLooper::AddCommonFilter()`](BLooper.md#BLooper_AddCommonFilter "AddCommonFilter\(\), RemoveCommonFilter\(\), SetCommonFilterList\(\), CommonFilterList\(\)") or [`BHandler::AddFilter()`](BHandler.md#BHandler_AddFilter).

####  MessageDelivery(), MessageSource()

`message_delivery MessageDelivery() const;``message_source MessageSource() const;`

These functions return constants, set when the `BMessageFilter` object was constructed, that describe the categories of messages that can be filtered. `MessageDelivery()` returns a constant that specifies how the message must be delivered (`B_DROPPED_DELIVERY`, `B_PROGRAMMED_DELIVERY`, or `B_ANY_DELIVERY`). `MessageSource()` returns how the source of the message is constrained (`B_LOCAL_SOURCE`, `B_REMOTE_SOURCE`, or `B_ANY_SOURCE`).

* * *

### Operators

#### = (copy)

`BMessageFilter operator=(const BMessageFilter& from);`

Copies the filtering criteria and `filter_hook` pointer (if any) from the right-side object into the left-side object.

* * *

### Defined Types

####  filter_hook, filter_result

`filter_result (*filter_hook)([`BMessage`](BMessage.md "BMessage")* message,  
[`BHandler`](BHandler.md "BHandler")** target,  
BMessageFilter* messageFilter);`
    
    
    typedef enum filter_result {
        B_SKIP_MESSAGE,
        B_DISPATCH_MESSAGE
    }

`filter_hook` defines the protocol for message-filtering functions. The first two arguments are the `message` that's being considered, and the proposed [`BHandler`](BHandler.md "BHandler") target. You can alter the contents of the message, and alter or even replace the handler. If you replace the handler, the new handler must belong to the same looper as the original. The new handler is given an opportunity to filter the message before it's dispatched.

`messageFilter` is a pointer to the object on whose behalf this function is being called; you mustn't delete this object. More than one `BMessageFilter` can use the same `filter_hook` function.

The return value of type filter_result must be one of these two values:

Constant| Description  
---|---  
`B_DISPATCH_MESSAGE`.| The `message` and `handler` are passed (by the caller) to the looper's [`DispatchMessage()`](BLooper.md#BLooper_DispatchMessage "DispatchMessage\(\)") function.  
`B_SKIP_MESSAGE`.| The message goes no further–it's immediately thrown away by the caller.  
  
It's possible to call your filter function yourself (i.e. outside the message-passing mechanism), but keep in mind that it's the caller's responsibility to interpret the return value.

You supply a `BMessageFilter` with a `filter_hook` function when you constuct the object. Alternatively, you can subclass `BMessageFilter` and provide an implementation of [`Filter()`](BMessageFilter.md#BMessageFilter_Filter "Filter\(\)"). If you do both, the `filter_hook` (and not [`Filter()`](BMessageFilter.md#BMessageFilter_Filter "Filter\(\)")) will be invoked when the object is asked to examine a message.

####  message_delivery
    
    
    typedef enum message_delivery {
        B_ANY_DELIVERY,
        B_DROPPED_DELIVERY,
        B_PROGRAMMED_DELIVERY
    }

This type enumerates the delivery criteria for filtering a message.

See also: The `BMessageFilter` [constructor](BMessageFilter.md#BMessageFilter_Constructor "BMessageFilter\(\)")

####  message_source
    
    
    typedef enum message_source {
        B_ANY_SOURCE,
        B_REMOTE_SOURCE,
        B_LOCAL_SOURCE
    }

This type enumerates the source criteria for filtering a message.

See also: The `BMessageFilter` [constructor](BMessageFilter.md#BMessageFilter_Constructor "BMessageFilter\(\)")

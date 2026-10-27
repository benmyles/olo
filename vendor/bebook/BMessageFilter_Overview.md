<!-- System Overview - The Application Kit - BMessageFilter -->

## BMessageFilter

A [`BMessageFilter`](BMessageFilter.md "BMessageFilter") is a message-screening function that you "attach" to a [`BLooper`](BLooper.md "BLooper") or [`BHandler`](BHandler.md "BHandler"). The message filter sees messages just before they're dispatched (i.e. just before [`BLooper::DispatchMessage()`](BLooper.md#BLooper_DispatchMessage "DispatchMessage\(\)")), and can modify or reject the message, change the message's designated handler, or whatever else it wants to do—the implementation of the filter function isn't restricted.

To define a message filter, you have to provide a message-filtering function. You do this by implementing the [`Filter()`](BMessageFilter.md#BMessageFilter_Filter "Filter\(\)") hook function in a [`BMessageFilter`](BMessageFilter.md "BMessageFilter") subclass, or by supplying a [`filter_hook`](BMessageFilter.md#filter_hook "filter_hook, filter_result") function to the [`BMessageFilter`](BMessageFilter.md "BMessageFilter") constructor. Only one filter function per object is called. If you implement [`Filter()`](BMessageFilter.md#BMessageFilter_Filter "Filter\(\)") and provide a `filter_hook` function, the `filter_hook` will win.

To attach a message filter to a looper, call [`BLooper::AddCommonFilter()`](BLooper.md#BLooper_AddCommonFilter "AddCommonFilter\(\), RemoveCommonFilter\(\), SetCommonFilterList\(\), CommonFilterList\(\)"). To add it to a handler, call [`BHandler::AddFilter()`](BHandler.md#BHandler_AddFilter). Looper filters see all incoming messages; handler filters see only those messages that are targetted for that particular handler.

A [`BLooper`](BLooper.md "BLooper") or [`BHandler`](BHandler.md "BHandler") can have more than one message filter. Furthermore, a looper can have two sets of filters: a looper set and a handler set (keep in mind that [`BLooper`](BLooper.md "BLooper") is derived from [`BHandler`](BHandler.md "BHandler")). Looper filters are applied before handler filters.

A [`BMessageFilter`](BMessageFilter.md "BMessageFilter") object can be assigned to only one [`BHandler`](BHandler.md "BHandler") or [`BLooper`](BLooper.md "BLooper") at a time.

Warning

![Warning](./images/admonitions/Stop_32.png)

The [`BMessageFilter`](BMessageFilter.md "BMessageFilter") class is intended to be used as part of the system-defined messaging system. If you try to use one outside this system, your results may not be what you expect.

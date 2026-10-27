<!-- System Overview - The Application Kit - BHandler -->

## BHandler

A [`BHandler`](BHandler.md "BHandler") object responds to messages that are handed to it by a [`BLooper`](BLooper.md "BLooper"). The [`BLooper`](BLooper.md "BLooper") tells the [`BHandler`](BHandler.md "BHandler") about a message by invoking the [`BHandler`](BHandler.md "BHandler")'s [`MessageReceived()` ](BHandler.md#BHandler_MessageReceived "MessageReceived\(\)") function.

* * *

### The Handler List

To be eligible to get messages from a [`BLooper`](BLooper.md "BLooper"), a [`BHandler`](BHandler.md "BHandler") must be in the [`BLooper`](BLooper.md "BLooper")'s list of eligible handlers (as explained in the [`BLooper`](BLooper.md "BLooper") class). The list of eligible handlers is ordered; if the "first" handler doesn't want to respond to a message that it has received, it simply calls the inherited version of [`MessageReceived()`](BHandler.md#BHandler_MessageReceived "MessageReceived\(\)") and the message will automatically be handed to the object's "next" handler. (System messages are not handed down the list.) The [`BLooper`](BLooper.md "BLooper") that all these [`BHandler`](BHandler.md "BHandler")s belong to is always the last the last handler in the list ([`BLooper`](BLooper.md "BLooper") inherits from [`BHandler`](BHandler.md "BHandler")).

A [`BHandler`](BHandler.md "BHandler")'s next handler assignment can be changed through [`SetNextHandler()`](BHandler.md#BHandler_SetNextHandler "SetNextHandler\(\), NextHandler\(\)").

* * *

### Targets

You can designate a target [`BHandler`](BHandler.md "BHandler") for most messages. The designation is made when calling [`BLooper`](BLooper.md "BLooper")'s [`PostMessage()`](BLooper.md#BLooper_PostMessage "PostMessage\(\)") function or when constructing the [`BMessenger`](BMessenger.md "BMessenger") object that will send the message. Messages that a user drags and drops are targeted to the object (a [`BView`](BView.md "BView")) that controls the part of the window where the message was dropped. The messaging mechanism eventually passes the target [`BHandler`](BHandler.md "BHandler") to [`DispatchMessage()`](BLooper.md#BLooper_DispatchMessage "DispatchMessage\(\)") , so that the message can be delivered to its designated destination.

* * *

### Filtering

Messages can be filtered before they're dispatched; that is, you can define a function that will look at the message before the target [`BHandler`](BHandler.md "BHandler")'s hook function is called. The filter function is associated with a [`BMessageFilter`](BMessageFilter.md "BMessageFilter") object, which records the criteria for calling the function.

Filters that should apply only to messages targeted to a particular [`BHandler`](BHandler.md "BHandler") are assigned to the [`BHandler`](BHandler.md "BHandler") by [`SetFilterList()`](BHandler.md#BHandler_SetFilterList "SetFilterList\(\), FilterList\(\), AddFilter\(\), RemoveFilter\(\)") or [`AddFilter()`](BHandler.md#BHandler_AddFilter). Filters that might apply to any message a [`BLooper`](BLooper.md "BLooper") dispatches, regardless of its target, are assigned by the parallel [`BLooper`](BLooper.md "BLooper") functions, [`SetCommonFilterList()`](BLooper.md#BLooper_SetCommonFilterList) and [`AddCommonFilter()`](BLooper.md#BLooper_AddCommonFilter "AddCommonFilter\(\), RemoveCommonFilter\(\), SetCommonFilterList\(\), CommonFilterList\(\)"). See those functions and the [`BMessageFilter`](BMessageFilter.md "BMessageFilter") class for details.

* * *

### Notifiers and Observers

A [`BHandler`](BHandler.md "BHandler") can be a notifier. A notifier is a handler that maintains one or more states and notifies interested parties when those states change. Each state is idenfified by a 32-bit `what` code. Interested parties, called observers, can register to monitor changes in one or more states by calling [`StartWatching()`](BHandler.md#BHandler_StartWatching "StartWatching\(\), StartWatchingAll\(\), StopWatching\(\), StopWatchingAll\(\)") and specifying the `what` code of the state they want to be notified of changes to.

This notification occurs when the [`BHandler`](BHandler.md "BHandler") calls [`SendNotices()`](BHandler.md#BHandler_SendNotices "SendNotices\(\)"); it's the handler's job to call [`SendNotices()`](BHandler.md#BHandler_SendNotices "SendNotices\(\)") whenever a state changes, to ensure that observers are kept informed of the changes. The [`BHandler`](BHandler.md "BHandler") passes to [`SendNotices()`](BHandler.md#BHandler_SendNotices "SendNotices\(\)") a message template to be sent to the observers.

When a notification is sent, observers receive a [`B_OBSERVER_NOTICE_CHANGE`](TheApplicationKit_MessageConstants.md#B_OBSERVER_NOTICE_CHANGE "B_OBSERVER_NOTICE_CHANGE") message with an int32 field [`B_OBSERVER_NOTICE_CHANGE`](TheApplicationKit_MessageConstants.md#B_OBSERVER_NOTICE_CHANGE "B_OBSERVER_NOTICE_CHANGE") that contains the `what` code of the state that changed, and a `B_OBSERVE_ORIGINAL_WHAT` field that contains the `what` value that was in the template [`BMessage`](BMessage.md "BMessage").

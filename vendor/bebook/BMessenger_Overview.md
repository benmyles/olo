<!-- System Overview - The Application Kit - BMessenger -->

## BMessenger

A [`BMessenger`](BMessenger.md "BMessenger") represents and sends messages to a message target, where the target is a [`BLooper`](BLooper.md "BLooper") and, optionally, a specific [`BHandler`](BHandler.md "BHandler") within that looper. The target can live in the same application as the [`BMessenger`](BMessenger.md "BMessenger") (a local target), or it can live in some other application (a remote target).

[`BMessenger`](BMessenger.md "BMessenger")'s most significant function is [`SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)"), which sends its argument [`BMessage`](BMessage.md "BMessage") to the target.

Note

![Note](./images/admonitions/Info_32.png)

For a local target, [`SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)") is roughly equivalent, in terms of efficiency, to posting a message directly to the [`BMessenger`](BMessenger.md "BMessenger")'s target (i.e [`BLooper::PostMessage()`](BLooper.md#BLooper_PostMessage "PostMessage\(\)")).

The global [`be_app_messenger`](BApplication.md#BApplication_be_app_messenger "be_app_messenger") [`BMessenger`](BMessenger.md "BMessenger") pointer, which targets [`be_app`](BApplication.md#BApplication_be_app "be_app")'s main message loop, is automatically initialized for you when you create your [`BApplication`](BApplication.md "BApplication") object. You can use it wherever [`BMessenger`](BMessenger.md "BMessenger")s are called for.

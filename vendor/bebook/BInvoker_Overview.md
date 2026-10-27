<!-- System Overview - The Application Kit - BInvoker -->

## BInvoker  
  
[`BInvoker`](BInvoker.md "BInvoker") is a convenience class that bundles up everything you need to create a handy message-sending package.

The [`BInvoker`](BInvoker.md "BInvoker") contains:

  * a [`BMessage`](BMessage.md "BMessage")

  * a [`BMessenger`](BMessenger.md "BMessenger") (that identifies a target handler), and

  * an optional [`BHandler`](BHandler.md "BHandler") that handles replies.

You set these ingredients, invoke [`Invoke()`](BInvoker.md#BInvoker_Invoke "Invoke\(\), InvokeNotify\(\)"), and off goes the message to the target. Replies are sent to the reply handler (`be_app` by default).

[`BInvoker`](BInvoker.md "BInvoker") uses [`BMessenger::SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)") to send its messages. The invocation is asynchronous, and there's no time limit on the reply.

[`BInvoker`](BInvoker.md "BInvoker") is mostly used as a mix-in class. A number of classes in the [Interface Kit](TheInterfaceKit.md "The Interface Kit") notably [`BControl`](BControl.md "BControl") derive from [`BInvoker`](BInvoker.md "BInvoker").

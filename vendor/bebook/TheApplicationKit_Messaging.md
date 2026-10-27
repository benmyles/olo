<!-- System Overview - The Application Kit - Messaging -->

## Messaging

The Application Kit provides a message-passing system lets your application send messages to and receives messages from other applications (including the Be-defined servers and apps), and from other threads in your own application.

The primary messaging classes are:

[`BMessage`](BMessage.md "BMessage")
    

Represents a message.

[`BHandler`](BHandler.md "BHandler")
    

Defines hook functions that are called to handle in-coming messages.

[`BLooper`](BLooper.md "BLooper")
    

Runs a loop that receives in-coming messages and figures out which [`BHandler`](BHandler.md "BHandler") should handle them.

[`BMessenger`](BMessenger.md "BMessenger")
    

Represents a message's destination (a combination of [`BLooper`](BLooper.md "BLooper") and [`BHandler`](BHandler.md "BHandler")), whether it's local or remote. The object is most useful for sending messages to other applications—you don't need it for local calls.

The other messaging classes are:

[`BMessageQueue`](BMessageQueue.md "BMessageQueue")
    

Is a FIFO that holds a [`BLooper`](BLooper.md "BLooper")'s in-coming messages.

[`BMessageFilter`](BMessageFilter.md "BMessageFilter")
    

Is a device that can examine and (potentially) reject or re-route in-coming messages.

[`BInvoker`](BInvoker.md "BInvoker")
    

Is a convenience class that lets you treat a message and its target (the [`BHandler`](BHandler.md "BHandler") that will handle the message) as a single object.

[`BMessageRunner`](BMessageRunner.md "BMessageRunner")
    

Lets you send the same message over and over, at regular intervals.

The rest of this chapter looks at…

  * The essential features of the four fundamental classes. ( "[Features of the Fundamental Classes](TheApplicationKit_Messaging.md#TheApplicationKit_Messaging_Features "Features of the Fundamental Classes")")

  * How a [`BLooper`](BLooper.md "BLooper") decides which [`BHandler`](BHandler.md "BHandler") should handle an in-coming message. ("[From Looper to Handler](TheApplicationKit_Messaging.md#TheApplicationKit_Messaging_FromLooperToHandler "From Looper to Handler")")

  * The different methods for sending messages and receiving replies. ("[Sending a Message](TheApplicationKit_Messaging.md#TheApplicationKit_Messaging_SendingAMessage "Sending a Message")").

and describes how the classes fit together in the messaging system with an emphasis on what you can do in your application to take part.

* * *

### Features of the Fundamental Classes

Looked at collectively, the four fundamental messaging classes comprise a huge chunk of API. Fortunately, the essential part of this API is pretty small; that's what we're going to look at here.

#### The BMessage Class

In the [`BMessage`](BMessage.md "BMessage") class, there's one essential data member, and two essential functions:

  * The [`what`](BMessage.md#BMessage_what "what") data member is an arbitrary uint32 value that describes (symbolically) what the message is about. You can set and examine `what` directly—you don't have to use functions to get to it. The `what` value is called the object's command constant. The BeOS defines some number of command constants (such as [`B_QUIT_REQUESTED`](TheApplicationKit_MessageConstants.md#B_QUIT_REQUESTED "B_QUIT_REQUESTED"), and [`B_MOUSE_DOWN`](TheInterfaceKit_MessageConstants.md#B_MOUSE_DOWN "B_MOUSE_DOWN")), but you'll also be creating constants of your own. Keep in mind that the constant's value is meaningless—it's just a code that identifies the "intent" of the message (and it's only meaningful if the receiver recognizes the constant).

  * The two essential functions are [`AddData()`](BMessage.md#BMessage_AddData "AddData\(\), AddBool\(\), AddInt8\(\), AddInt16\(\), AddInt32\(\), AddInt64\(\), AddFloat\(\), AddDouble\(\), AddString\(\), AddPoint\(\), AddRect\(\), AddRef\(\), AddMessage\(\), AddMessenger\(\), AddPointer\(\), AddFlat\(\)") and [`FindData()`](BMessage.md#BMessage_FindData "FindData\(\), FindBool\(\), FintInt8\(\), FindInt16\(\), FindInt32\(\), FindInt64\(\), FindFloat\(\), FindDouble\(\), FindString\(\), FindPoint\(\), FindRect\(\), FindRef\(\), FindMessage\(\), FindMessenger\(\), FindPointer\(\), FindFlat\(\)"). These functions add data to a message you're about to send, and retrieve it from a message you just received. A [`BMessage`](BMessage.md "BMessage") can hold any amount of data; each data item (or "field") is identified by name, type, and index. For example, you can ask a message for the third boolean value named "IsEnabled" that it contains. In general, you use type-specific functions such as `Add/FindString()` and `Add/FindInt32()` rather than `Add/FindData()`. The query we just posed would actually look like this:
        
        /* The args are: name, index, value (returned by
           reference) */
        bool returnValue;
        aMessage.FindBool("IsEnabled", 2, &returnValue);

In summary, a [`BMessage`](BMessage.md "BMessage") contains (1) a command constant and (2) a set of data fields. Every [`BMessage`](BMessage.md "BMessage") that's used in the messaging system must have a command constant, but not every object needs to have data fields. (Other parts of the BeOS use [`BMessage`](BMessage.md "BMessage")s for their data only. The [`BClipboard`](BClipboard.md "BClipboard") object, for example, ignores a [`BMessage`](BMessage.md "BMessage")'s command constant.)

Note

![Note](./images/admonitions/Info_32.png)

When discussing system-generated [`BMessage`](BMessage.md "BMessage") objects, we refer to the object by its command constant. For example, "a `B_MOUSE_DOWN`" means "a [`BMessage`](BMessage.md "BMessage") that has `B_MOUSE_DOWN` as its command constant".

Notice that a [`BMessage`](BMessage.md "BMessage") doesn't know how to send itself. However, as we'll see later, it does know how to reply to its sender once it's in the hands of the recipient.

#### The BLooper Class

[`BLooper`](BLooper.md "BLooper")'s role is to receive messages and figure out what to do with them. There are four parts to this job, embodied in these functions:

  * Every [`BLooper`](BLooper.md "BLooper") spawns a thread in which it runs a message loop. It's in this thread that the object receives messages. But you have to kick the [`BLooper`](BLooper.md "BLooper") to get it to run; you do this by calling the [`Run()`](BLooper.md#BLooper_Run "Run\(\)") function. When you're done with the obejct—when you no longer need it to receive messages—you call [`Quit()`](BLooper.md#BLooper_Quit "Quit\(\)")

  * Although you never invoke it directly, [`DispatchMessage()`](BLooper.md#BLooper_DispatchMessage "DispatchMessage\(\)") is the guts of the message loop. All messages that the looper receives show up in individual invocations of [`DispatchMessage()`](BLooper.md#BLooper_DispatchMessage "DispatchMessage\(\)"). The function decides where the message should go next, which is mostly a matter of deciding whether (1) the message should be handled by a system-defined hook function, or (2) passed to [`BHandler`](BHandler.md "BHandler")'s [`MessageReceived()`](BLooper.md#BLooper_MessageReceived "MessageReceived\(\)") function (which we'll talk about in a moment). Three other important aspects of [`DispatchMessage()`](BLooper.md#BLooper_DispatchMessage "DispatchMessage\(\)") are…

    1. It runs in the [`BLooper`](BLooper.md "BLooper")'s message thread (or message loop); this is a separate thread that the object spawns specifically to receive in-coming messages.

    2. Individual [`DispatchMessage()`](BLooper.md#BLooper_DispatchMessage "DispatchMessage\(\)") invocations are synchronous with regard to the loop. In other words, when a message shows up, [`DispatchMessage()`](BLooper.md#BLooper_DispatchMessage "DispatchMessage\(\)") is called and runs to completion before the next message can be processed. (Messages that show up while [`DispatchMessage()`](BLooper.md#BLooper_DispatchMessage "DispatchMessage\(\)") is busy aren't lost—they're queued up in a [`BMessageQueue`](BMessageQueue.md "BMessageQueue") object.)

    3. It's fully implemented by [`BLooper`](BLooper.md "BLooper") (and kit classes derived from [`BLooper`](BLooper.md "BLooper")). You should rarely need to override it in your application.

  * The [`PostMessage()`](BLooper.md#BLooper_PostMessage "PostMessage\(\)") function delivers a message to the [`BLooper`](BLooper.md "BLooper"). In other words, it invokes [`DispatchMessage()`](BLooper.md#BLooper_DispatchMessage "DispatchMessage\(\)") in the looper's message thread. You call [`PostMessage()`](BLooper.md#BLooper_PostMessage "PostMessage\(\)") directly in your code. For example, here we create a [`BMessage`](BMessage.md "BMessage") and post it to our [`BApplication`](BApplication.md "BApplication") object ([`BApplication`](BApplication.md "BApplication") inherits from [`BLooper`](BLooper.md "BLooper")):
        
        /* This form of the BMessage constructor sets the command constant. */
        BMessage *myMessage = new BMessage(YOUR_CONSTANT_HERE);
        be_app->PostMessage(myMessage)

The [`BApplication`](BApplication.md "BApplication") and [`BWindow`](BWindow.md "BWindow") classes inherit from [`BLooper`](BLooper.md "BLooper").

#### The BHandler Class

[`BHandler`](BHandler.md "BHandler") objects are called upon to handle the messages that a [`BLooper`](BLooper.md "BLooper") receives. A [`BHandler`](BHandler.md "BHandler") depends on two essential function:

[`MessageReceived()`](BHandler.md#BHandler_MessageReceived "MessageReceived\(\)")
    

Is the function that a [`BLooper`](BLooper.md "BLooper") calls to dispatch an in-coming message to the [`BHandler`](BHandler.md "BHandler") (the [`BMessage`](BMessage.md "BMessage") is passed as the function's only argument). This is a hook function that a [`BHandler`](BHandler.md "BHandler") subclass implements to handle the different types of messages that it expects to receive. Most implementations examine the message's command constant and go from there. A typical outline looks like this:
    
    
    void MyHandler::MessageReceived(BMessage* message)
    {
       /* Examine the command constant. */
       switch ( message->what ) {
    
       case YOUR_CONSTANT_HERE:
          /* Call a function that handles this sort of message. */
          HandlerFunctionA();
          break;
    
       case ANOTHER_CONSTANT_HERE:
          /* ditto */
          HandlerFunctionB();
          break;
    
       default:
          /* Messages that your class doesn't recognize must
           * be passed on to the base class implementation. */
          _baseClass_ ::MessageReceived(message);
          break;
       }
    }

[`BLooper::AddHandler()`](BLooper.md#BLooper_AddHandler "AddHandler\(\), RemoveHandler\(\), HandlerAt\(\), CountHandlers\(\), IndexOf\(\)")
    

Is [`BHandler`](BHandler.md "BHandler")'s other essential function and is defined by [`BLooper`](BLooper.md "BLooper"). This function adds the (argument) [`BHandler`](BHandler.md "BHandler") object to the (invoked-upon) [`BLooper`](BLooper.md "BLooper")'s list of candidate handlers (its handler chain). If a [`BHandler`](BHandler.md "BHandler") wants to handle messages that are received by a [`BLooper`](BLooper.md "BLooper"), it must first be added to the [`BLooper`](BLooper.md "BLooper")'s handler chain.

[`BLooper`](BLooper.md "BLooper") inherits from [`BHandler`](BHandler.md "BHandler"), and automatically adds itself to its own handler chain. This means that a [`BLooper`](BLooper.md "BLooper") can handle the messages that it receives—this is the default behaviour for most messages. We'll examine this issue in depth later in this chapter.

The other classes that inherit from [`BHandler`](BHandler.md "BHandler") are [`BView`](BView.md "BView") and [`BShelf`](BShelf.md "BShelf") (both in the Interface Kit).

#### The BMessenger Class

A [`BMessenger`](BMessenger.md "BMessenger")'s most important feature is that it can send a message to a remote application. To do this takes two steps, which point out the class' essential features:

  * You identify the application that you want to send a message to (the "target") in the [`BMessenger`](BMessenger.md "BMessenger") constructor. An application is identified by its app signature (a MIME string).

  * The [`SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)") function sends a message to the target.

[`BMessenger`](BMessenger.md "BMessenger")s can also be used to target local looper/handler pairs.

* * *

### From Looper to Handler

A [`BLooper`](BLooper.md "BLooper") pops a message from its message queue and, within its [`DispatchMessage()`](BLooper.md#BLooper_DispatchMessage "DispatchMessage\(\)") function, dispatches the message by invoking a [`BHandler`](BHandler.md "BHandler") function. But (1) which [`BHandler`](BHandler.md "BHandler") and (2) which function?

#### Finding a Handler

First, let's answer the "which [`BHandler`](BHandler.md "BHandler")" question. To determine which [`BHandler`](BHandler.md "BHandler") should handle an in-coming message, a [`BLooper`](BLooper.md "BLooper") follows these steps:

  1. Does the [`BMessage`](BMessage.md "BMessage") target a specific [`BHandler`](BHandler.md "BHandler")? Both the [`BLooper::PostMessage()`](BLooper.md#BLooper_PostMessage "PostMessage\(\)") and [`BMessenger::SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)") functions provide additional arguments that let you specify a target handler that you want to have handle the message (you can also set the target in the [`BMessenger`](BMessenger.md "BMessenger") constructor). If a [`BHandler`](BHandler.md "BHandler") is specified, the [`BMessage`](BMessage.md "BMessage") will show up in that object's [`MessageReceived()`](BHandler.md#BHandler_MessageReceived "MessageReceived\(\)") function (or it will invoke a message-mapped hook function, as explained below).

  2. Does the [`BLooper`](BLooper.md "BLooper") designate a preferred handler? Through its [`SetPreferredHandler()`](BLooper.md#BLooper_SetPreferredHandler "SetPreferredHandler\(\), PreferredHandler\(\)") function, a [`BLooper`](BLooper.md "BLooper") can designate one of the objects in its handler chain as its preferred handler, and passes all messages to that object.

  3. The [`BLooper`](BLooper.md "BLooper") handles the [`BMessage`](BMessage.md "BMessage") itself. If there's no target handler or preferred handler designation, the [`BLooper`](BLooper.md "BLooper") handles the message itself—in other words, the message is passed to the [`BLooper`](BLooper.md "BLooper")'s own [`MessageReceived()`](BLooper.md#BLooper_MessageReceived "MessageReceived\(\)") function (or message-mapped hook).

We should mention here that both the [`BApplication`](BApplication.md "BApplication") and the [`BWindow`](BWindow.md "BWindow") class fine-tune this decision process a bit. However, the meddling that they do only applies to system messages (described below). The messages that you define yourself (i.e. the command constants that your application defines) will always follow the message dispatching path described here.

Note

![Note](./images/admonitions/Info_32.png)

If you look at the [`DispatchMessage()`](BLooper.md#BLooper_DispatchMessage "DispatchMessage\(\)") protocol, you'll notice that it has a [`BMessage`](BMessage.md "BMessage") and a [`BHandler`](BHandler.md "BHandler") as arguments. In other words, the "which handler" decision described here is actually made before [`DispatchMessage()`](BLooper.md#BLooper_DispatchMessage "DispatchMessage\(\)") is called. In general, this is an implementation detail that you shouldn't worry about. If you want to think that [`DispatchMessage()`](BLooper.md#BLooper_DispatchMessage "DispatchMessage\(\)") makes the decision—and we've done nothing to disabuse you of this notion—go ahead and think it.

#### Finding a Function

As described above, a [`BLooper`](BLooper.md "BLooper") passes a [`BMessage`](BMessage.md "BMessage") to a [`BHandler`](BHandler.md "BHandler") by invoking the latter's [`MessageReceived()`](BHandler.md#BHandler_MessageReceived "MessageReceived\(\)") function. This is true of all messages that you create yourself, but it isn't true of certain messages that the system defines and sends. These system-generated messages (or system messages)—particularly those that report user events such as `B_MOUSE_DOWN` or `B_APP_ACTIVATED` \- invoke specific hook functions.

For example, when the user presses a key, a `B_KEY_DOWN` message is sent to the active [`BWindow`](BWindow.md "BWindow") object.. From within its [`DispatchMessage()`](BLooper.md#BLooper_DispatchMessage "DispatchMessage\(\)") function, [`BWindow`](BWindow.md "BWindow") invokes the [`MouseDown()`](BView.md#BView_MouseDown "MouseDown\(\)") function of the [`BView`](BView.md "BView") that currently holds keyboard focus. (When a [`BView`](BView.md "BView") is made the focus of keyboard events, its window promotes it to preferred handler.)

So the question of "which function" is fairly simple: If the [`BMessage`](BMessage.md "BMessage") is a system message that's mapped to a hook function, the hook function is invoked. If it's not mapped to a hook function, the [`BHandler`](BHandler.md "BHandler")'s [`MessageReceived()`](BHandler.md#BHandler_MessageReceived "MessageReceived\(\)") function is invoked.

Each chapter contains a list of system messages and the hook functions that they're mapped to. Note that not all system messages are mapped to hook functions; some of them do show up in [`MessageReceived()`](BHandler.md#BHandler_MessageReceived "MessageReceived\(\)").

#### Inheritance and the Handler Chain

Let's look at [`MessageReceived()`](BHandler.md#BHandler_MessageReceived "MessageReceived\(\)") again. It was asserted, in a code snippet shown earlier, that a typical [`MessageReceived()`](BHandler.md#BHandler_MessageReceived "MessageReceived\(\)") implementation should include an invocation of the base class' version of the function:
    
    
    void MyHandler::MessageReceived(BMessage* message)
    {
       switch ( message->what ) {
    
       /* Command constants that you handle go here. */
    
       default:
          /* Messages that your class doesn't recognize must be passed
          * on to the base class implementation. */
          _baseClass_ ::MessageReceived(message);
          break;
       }
    }

This isn't just a good idea—it's an essential part of the messaging system. Forwarding the message to the base class does two things: It lets messages (1) pass up the class hierarchy, and (2) pass along the handler chain (in that order).

Passing up the class hierarchy is mostly straight-forward—it's no different for the [`MessageReceived()`](BHandler.md#BHandler_MessageReceived "MessageReceived\(\)") function than it is for any other function. But what happens at the top of the hierarchy—at the [`BHandler`](BHandler.md "BHandler") class itself—adds a small wrinkle. [`BHandler`](BHandler.md "BHandler")'s implementation of [`MessageReceived()`](BHandler.md#BHandler_MessageReceived "MessageReceived\(\)") looks for the next handler in the [`BLooper`](BLooper.md "BLooper")'s handler chain and invokes that object's [`MessageReceived()`](BHandler.md#BHandler_MessageReceived "MessageReceived\(\)") function.

* * *

### Sending a Message

There are two functions that send messages to distinct recipients:

[`BLooper::PostMessage()`](BLooper.md#BLooper_PostMessage "PostMessage\(\)")
    

Can be used if the target (the [`BLooper`](BLooper.md "BLooper") that the [`BLooper::PostMessage()`](BLooper.md#BLooper_PostMessage "PostMessage\(\)") function is invoked upon) lives in the same application as the message sender.

[`BMessenger::SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)")
    

Lets you send messages to remote applications. The [`BMessenger`](BMessenger.md "BMessenger") object acts as a proxy for the remote app. ([`SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)") can also be used to send a message to a local [`BLooper`](BLooper.md "BLooper"), for reasons that we'll discuss later.)

#### The PostMessage() Function

You can post a message if the recipient [`BLooper`](BLooper.md "BLooper") is in your application:
    
    
    myLooper->PostMessage(new BMessage(DO_SOMETHING), targetHandler);

As shown here, you can specify the handler that you want to have handle a posted message. The only requirement is that the [`BHandler`](BHandler.md "BHandler") must belong to the [`BLooper`](BLooper.md "BLooper").

If the handler argument is `NULL`, the message is handled by the looper's _preferred handler_
    
    
    myLooper->PostMessage(new BMessage(DO_SOMETHING), NULL);

By using the default handler, you let the looper decide who should handle the message.

Note

![Note](./images/admonitions/Info_32.png)

The creator of the [`BMessage`](BMessage.md "BMessage") retains ownership and is responsible for deleting it when it's no longer needed.

#### The SendMessage() Function

If you want to send a message to another application, you have to use [`BMessenger`](BMessenger.md "BMessenger")'s [`SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)") function. First, you construct a [`BMessenger`](BMessenger.md "BMessenger") object that identifies the remote app by signature…
    
    
    BMessenger messenger("application/x-some-app");

…and then you invoke [`SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)"):
    
    
    messenger.SendMessage(new BMessage(DO_SOMETHING));

Note

![Note](./images/admonitions/Info_32.png)

The creator of the [`BMessage`](BMessage.md "BMessage") retains ownership and is responsible for deleting it when it's no longer needed.

#### Handling a Reply

Every [`BMessage`](BMessage.md "BMessage") that you send identifies the application from which it was sent. The recipient of the message can reply to the message whether you (the sender) expect a reply or not. By default, reply messages are handled by your [`BApplication`](BApplication.md "BApplication") object. If you want reply messages to be handled by some other [`BHandler`](BHandler.md "BHandler"), you specify the object as a final argument to the [`PostMessage()`](BLooper.md#BLooper_PostMessage "PostMessage\(\)") or [`SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)") call:
    
    
    myLooper->PostMessage(new BMessage(DO_SOMETHING), targetHandler,
    replyHandler);
    /* and */
    myMessenger.SendMessage(&message, replyHandler);

The reply is sent asynchronously with regard to the [`PostMessage()`](BLooper.md#BLooper_PostMessage "PostMessage\(\)") / [`SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)") function.

[`SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)") (only) lets you ask for a reply message that's handed back synchronously in the [`SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)") call itself:
    
    
    BMessage reply;
    myMessenger.SendMessage(&message, &reply);

[`SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)") doesn't return until a reply is received. A default message is created and returned if the recipient doesn't respond quickly enough.

#### Receiving a Message

[`BMessage`](BMessage.md "BMessage")'s [`SendReply()`](BMessage.md#BMessage_SendReply "SendReply\(\)") function has the same syntax as [`SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)"), so it's possible to ask for a synchronous reply to a message that is itself a reply,
    
    
    BMessage message(READY);
    BMessage reply;
    theMessage->SendReply(&message, &reply);
    if ( reply->what != B_NO_REPLY ) {
       . . .
    }

or to designate a [`BHandler`](BHandler.md "BHandler") for an asynchronous reply to the reply:
    
    
    theMessage->SendReply(&message, someHandler);

In this way, two applications can maintain an ongoing exchange of messages.

#### Handler Associations

To be notified of an arriving message, a [`BHandler`](BHandler.md "BHandler") must "belong" to the [`BLooper`](BLooper.md "BLooper"); it must have been added to the [`BLooper`](BLooper.md "BLooper")'s list of eligible handlers. The list can contain any number of objects, but at any given time a [`BHandler`](BHandler.md "BHandler") can belong to only one [`BLooper`](BLooper.md "BLooper").

Handlers that belong to the same [`BLooper`](BLooper.md "BLooper") can be chained in a linked list. If an object can't respond to a message, the system passes the message to its next handler.

[`BLooper`](BLooper.md "BLooper")'s [`AddHandler()`](BLooper.md#BLooper_AddHandler "AddHandler\(\), RemoveHandler\(\), HandlerAt\(\), CountHandlers\(\), IndexOf\(\)") function sets up the looper-handler association; [`BHandler`](BHandler.md "BHandler")'s [`SetNextHandler()`](BHandler.md#BHandler_SetNextHandler "SetNextHandler\(\), NextHandler\(\)") sets the handler-handler link.

* * *

### Message Filters

The [`BMessageFilter`](BMessageFilter.md "BMessageFilter") class lets create filtering functions that examine and re-route (or reject) incoming messages before they're processed by a [`BLooper`](BLooper.md "BLooper"). Message filters can also be applied to individual [`BHandler`](BHandler.md "BHandler") objects.

* * *

### Message Protocols

Both the source and the destination of a message must agree upon its format—the command constant and the names and types of data fields. They must also agree on details of the exchange—when the message can be sent, whether it requires a response, what the format of the reply should be, what it means if an expected data item is omitted, and so on.

None of this is a problem for messages that are used only within an application; the application developer can keep track of the details. However, protocols must be published for messages that communicate between applications. You're urged to publish the specifications for all messages your application is willing to accept from outside sources and for all those that it can package for delivery to other applications.

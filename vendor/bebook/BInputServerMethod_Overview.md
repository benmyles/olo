<!-- System Overview - The Input Server - BInputServerMethod -->

## BInputServerMethod

[`BInputServerMethod`](BInputServerMethod.md "BInputServerMethod") is a base class for input methods; these are instances of [`BInputServerMethod`](BInputServerMethod.md "BInputServerMethod") that act as an interface between the user and languages using character sets that can't be easily represented on standard keyboards, such as the Japanese input method that comes with BeOS.

Input methods generally handle `B_KEY_DOWN` messages in their [`Filter()`](BInputServerFilter.md#BInputServerFilter_Filter "Filter\(\)") function (see [`BInputServerFilter`](BInputServerFilter.md "BInputServerFilter")), keeping some sort of state around to translate these standard keyboard messages into new `B_KEY_DOWN` messages representing another character set. An input method can handle any input event, they're not limited to keyboard events.

Warning

![Warning](./images/admonitions/Stop_32.png)

Writing an input method is an involved process, even though the `BInputServerMethod` protocol is relatively simple. If you're working on an input method, please feel free to contact Be Developer Technical Support (devsupport@be.com) for additional information.

* * *

### Input Method Events

Input methods insert `B_INPUT_METHOD_EVENT` messages (using their [`EnqueueMessage()`](BInputServerMethod.md#BInputServerMethod_EnqueueMessage "EnqueueMessage\(\)") function) into the Input Server's event stream. These messages let [`BView`](BView.md "BView") subclasses work together with your input method to create a seamless experience for the user.

Each `B_INPUT_METHOD_EVENT` message contains a `be:opcode` field (an int32 value) indicating the kind of event:

Constant| Description  
---|---  
`B_INPUT_METHOD_STARTED`| Indicates that a new input transaction has begun. Add a [`BMessenger`](BMessenger.md "BMessenger") in the `be:reply_to` field; the receiver of the message will use this messenger to communicate with you during the transaction.  
`B_INPUT_METHOD_STOPPED`| Indicates that the transaction is over.  
  
In between the `B_INPUT_METHOD_STARTED` and `B_INPUT_METHOD_STOPPED` messages, you'll send `B_INPUT_METHOD_CHANGED` and `B_INPUT_METHOD_LOCATION_REQUEST` messages as the transaction proceeds.

`B_INPUT_METHOD_CHANGED` does most of the work in an input transaction; add the following important fields:

Entry| Type| Description  
---|---|---  
`be:string`| `B_STRING_TYPE`| The text the user is currently entering; the receiver will display it at the current insertion point. [`BTextView`](BTextView.md "BTextView") also highlights the text in blue to show that it's part of a transitory transaction.  
`be:selection`| `B_INT32_TYPE`| A pair of B_INT32_TYPE offsets into the `be:string` if part of `be:string` is current selected. [`BTextView`](BTextView.md "BTextView") highlights this selection in red instead of drawing it in blue.  
`be:clause_start`| `B_INT32_TYPE`| Zero or more offsets into the `be:string` for handling languages (such as Japanese) that separate a sentence or phrase into numerous clauses. An equal number of `be:clause_start` and `be:clause_end` pairs delimit these clauses; [`BTextView`](BTextView.md "BTextView") separates the blue/red highlighting wherever there is a clause boundary.  
`be:clause_end`| `B_INT32_TYPE`| Zero or more offsets into `be:string`; there must be as many `be:clause_end` entries as there are `be:clause_start`.  
`be:confirmed`| `B_BOOL_TYPE`| True when the user has entered and "confirmed" the current string and wishes to end the transaction. [`BTextView`](BTextView.md "BTextView") unhighlights the blue/red text and waits for a `B_INPUT_METHOD_STOPPED` (to close the transaction) or another `B_INPUT_METHOD_CHANGED` (to start a new transaction immediately).  
  
`B_INPUT_METHOD_LOCATION_REQUEST` is the input method's way of asking for the on-screen location of each character in `be:string`. This information can be used by the input method to pop up additional windows giving the user an opportunity to select characters from a list or anything else that makes sense. When you send a `B_INPUT_METHOD_LOCATION_REQUEST`, the receiver will reply to the `be:reply_to` messenger (that you sent in your `B_INPUT_METHOD_STARTED` message) with a `B_INPUT_METHOD_EVENT` message, filling in the following fields:

Entry| Type| Description  
---|---|---  
`be:opcode`| `B_INT32_TYPE`| Set to `B_INPUT_METHOD_LOCATION_REQUEST`.  
`be:location_reply`| `B_POINT_TYPE`| The co-ordinates of each character (there should be one `be:location_reply` for every character in `be:string`) relative to the display (not your view or your window).  
`be:height_reply`| `B_FLOAT_TYPE`| The height of each character in `be:string`.  
  
* * *

### Creating

To create a new input method, you must:

  * Create a subclass of [`BInputServerMethod`](BInputServerMethod.md "BInputServerMethod")

  * Implement the `instantiate_input_method()` C function to create an instance of your [`BInputServerMethod`](BInputServerMethod.md "BInputServerMethod") subclass

  * Compile the class and function as an add-on

  * Install the add-on in one of the input method directories

At boot time (or whenever the Input Server is restarted; see "Dynamic Loading"), the Input Server loads the add-ons it finds in the input method directories. For each add-on it finds, the Server invokes `instantiate_input_method()` to get a pointer to the add-on's `BInputServerMethod` object. After constructing the object, the Server calls `InitCheck()` to give the add-on a chance to bail out if the constructor failed.

* * *

### Installing an Input Method

The input server looks for input methods in the "input_server/methods" subdirectories of `B_BEOS_ADDONS_DIRECTORY`, `B_COMMON_ADDONS_DIRECTORY`, and `B_USER_ADDONS_DIRECTORY`.

  * You can install your input devices in the latter two directories—i.e. those under `B_COMMON_ADDONS_DIRECTORY`, and `B_USER_ADDONS_DIRECTORY`.

  * The `B_BEOS_ADDONS_DIRECTORY` is reserved for add-ons that are supplied by the BeOS.

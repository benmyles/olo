<!-- System Overview - The Application Kit - BMessageRunner -->

## BMessageRunner

The [`BMessageRunner`](BMessageRunner.md "BMessageRunner") class provides a handy mechanism for automatically sending an arbitrary message to a [`BMessenger`](BMessenger.md "BMessenger") at specified intervals. The application that creates the [`BMessageRunner`](BMessageRunner.md "BMessageRunner") can specify the message, the [`BMessenger`](BMessenger.md "BMessenger") to send the message to, how often to send the message, and how many times it should be sent.

The system roster handles actually dispatching the messages to the appropriate [`BMessenger`](BMessenger.md "BMessenger")s at the desired time intervals; this class simply acts as an intermediary through which your application asks the roster to schedule sending the messages.

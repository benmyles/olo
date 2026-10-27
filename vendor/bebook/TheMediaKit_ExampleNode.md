<!-- System Overview - The Media Kit - A BMediaEventLooper Example -->

## A BMediaEventLooper Example

Implementing a node using the [`BMediaEventLooper`](BMediaEventLooper.md "BMediaEventLooper") class to handle the scheduling issues makes your job much easier. So much so, in fact, that Be recommends that you not try to bypass [`BMediaEventLooper`](BMediaEventLooper.md "BMediaEventLooper"). This section provides an example of creating a node using [`BMediaEventLooper`](BMediaEventLooper.md "BMediaEventLooper") to handle the control port and event queueing.

When you implement a node using [`BMediaEventLooper`](BMediaEventLooper.md "BMediaEventLooper"), you have to derive from multiple classes: [`BMediaEventLooper`](BMediaEventLooper.md "BMediaEventLooper"), of course, and [`BControllable`](BControllable.md "BControllable") if you want your node to be user-configurable. Then you need to also derive from [`BBufferConsumer`](BBufferConsumer.md "BBufferConsumer") and/or [`BBufferProducer`](BBufferProducer.md "BBufferProducer"), depending on whether you create or receive buffers. And if your node can provide a time source, you would also need to derive from [`BTimeSource`](BTimeSource.md "BTimeSource").

[`BMediaEventLooper`](BMediaEventLooper.md "BMediaEventLooper") doesn't eliminate your obligation to implement the pure virtual functions in all the classes you derive from, and you may still have to implement some or all of the other virtual functions as well. All [`BMediaEventLooper`](BMediaEventLooper.md "BMediaEventLooper") gives you is automatic queueing of received messages, and automatic management of your control port. This still eliminates a lot of repetitive and uninteresting work, and makes your job a lot easier.

Let's look at an example node. This node derives from [`BBufferConsumer`](BBufferConsumer.md "BBufferConsumer"), [`BControllable`](BControllable.md "BControllable"), and [`BMediaEventLooper`](BMediaEventLooper.md "BMediaEventLooper"), and logs media events to a disk file. The node's latency, among other parameters, is user-configurable, so you can use this node to simulate different loads on the system.

This section only shows key portions of the sample node. If you'd like to play with the full source code, you can download it from the Be web site at ftp://ftp.be.com/pub/samples/media_kit/LoggingConsumer.zip.

* * *

### The Constructor and Destructor

Let's start at the very beginning (a very good place to start). The constructor has to perform the initialization for all the superclasses:

#### Constructor
    
    
    LoggingConsumer::LoggingConsumer(const entry_ref &logFile)
       :BMediaNode("LoggingConsumer"),
        BBufferConsumer(B_MEDIA_UNKNOWN_TYPE),
        BControllable(),
        BMediaEventLooper(),
        mLogRef(logFile), mWeb(NULL),
        mLateBuffers(0),
        mLatency(50 * 1000),      // default to 50 milliseconds
        mSpinPercentage(0.10),    // default to spinning 10% of total latency
        mPriority(B_REAL_TIME_PRIORITY),
        mLastLatencyChange(0),
        mLastSpinChange(0),
        mLastPrioChange(0)
    {
       mLogger = new LogWriter(logFile);
    }

The constructors for [`BMediaNode`](BMediaNode.md "BMediaNode"), [`BBufferConsumer`](BBufferConsumer.md "BBufferConsumer"), [`BControllable`](BControllable.md "BControllable"), and [`BMediaEventLooper`](BMediaEventLooper.md "BMediaEventLooper") are all called through to here, to perform the standard initialization for each of these superclasses. In addition, an assortment of local variables are initialized.

Finally, the thread that handles actually writing into the log file is started; this is done by the `LogWriter` class' constructor. This class will be used to access the log file. We won't get into any specifics of how the `LogWriter` class works. Suffice it to say that the `LogWriter` has some public variables that affect the messages it logs to the file, and a `Log()` function that actually writes a new entry into the log file.

#### Destructor
    
    
    LoggingConsumer::~LoggingConsumer() {
       BMediaEventLooper::Quit();
       SetParameterWeb(NULL);
       mWeb = NULL;
       delete mLogger;
    }

The destructor, shown above, begins by stopping the [`BMediaEventLooper`](BMediaEventLooper.md "BMediaEventLooper")'s control loop. This is done by calling [`BMediaEventLooper::Quit()`](BMediaEventLooper.md#BMediaEventLooper_Quit "Quit\(\)"). Next we delete the parameter web by calling [`BControllable::SetParameterWeb()`](BControllable.md#BControllable_SetParameterWeb "SetParameterWeb\(\), Web\(\)"); we set the parameter web to `NULL` instead of deleting it ourselves because this lets the node handle cleanup—the node deletes the web for us in this case. We then set our local pointer to the web to `NULL` so we know it's been deleted.

Once all that's been done, we delete the logging thread. This is done last to avoid possible race conditions that could arise if an event arrives that would trigger a write to the log. By waiting until the [`BMediaEventLooper`](BMediaEventLooper.md "BMediaEventLooper") has been quit, we can avoid this potential problem.

* * *

### BMediaNode Functions

We also need to implement some functions from the [`BMediaNode`](BMediaNode.md "BMediaNode") class. Other than [`BMediaNode::AddOn()`](BMediaNode.md#BMediaNode_AddOn "AddOn\(\)"), [`BMediaEventLooper`](BMediaEventLooper.md "BMediaEventLooper") provides default implementations for these functions. In many cases, you can just use the default implementations. We'll look at one custom implementation, so you can see how it's done.

#### AddOn()
    
    
    BMediaAddOn* LoggingConsumer::AddOn(int32 *) const {
       return NULL;
    }

The [`BMediaNode::AddOn()`](BMediaNode.md#BMediaNode_AddOn "AddOn\(\)") function's job is to return a pointer to the [`BMediaAddOn`](BMediaAddOn.md "BMediaAddOn") object that instantiated the node. In this case, we're contained within an application, so we return `NULL`. But if the node were created from a media node add-on, we'd return a pointer to that object here.

* * *

### Other BMediaNode Functions
    
    
    void LoggingConsumer::SetRunMode(run_mode mode) {
       log_message logMsg;
       logMsg.now = TimeSource()->Now();
       logMsg.runmode.mode = mode;
       mLogger->Log(LOG_SET_RUN_MODE, logMsg);
    
       BMediaEventLooper::SetRunMode(mode);
    }

The [`BMediaNode::SetRunMode()`](BMediaNode.md#BMediaNode_SetRunMode) function handles changing the run mode of the node. The [`BMediaEventLooper`](BMediaEventLooper.md "BMediaEventLooper") class handles this automatically, but you can augment the implementation if you need to.

In this case, as we do in all the other [`BMediaNode`](BMediaNode.md "BMediaNode") functions we implement, we simply log the call, then defer to [`BMediaEventLooper`](BMediaEventLooper.md "BMediaEventLooper")'s implementation.

We call `TimeSource`()->`Now`() to get the current time, which is inserted into the log along with a description of the type of event that occurred.

The other [`BMediaNode`](BMediaNode.md "BMediaNode") functions we implement work similarly, logging the request, then deferring to the [`BMediaEventLooper`](BMediaEventLooper.md "BMediaEventLooper") implementation.

* * *

### BControllable Functions

The [`BControllable`](BControllable.md "BControllable") functions we implement allow our node to be user-configurable; these functions are called when someone needs to know the current value of one of the parameters, or when the value of a parameter needs to be changed.

#### GetParameterValue()
    
    
    status_t LoggingConsumer::GetParameterValue(int32 id,
             bigtime_t* last_change, void* value, size_t* ioSize) {
       log_message logMsg;
       logMsg.now = TimeSource()->Now();
       logMsg.param.id = id;
       mLogger->Log(LOG_GET_PARAM_VALUE, logMsg);
    
       if (*ioSize < sizeof(float)) return B_ERROR;
    
       switch (id) {
       case LATENCY_PARAM:
          *last_change = mLastLatencyChange;
          *((float*) value) = mLatency/1000;  // the BParameter reads milliseconds,
                                              // not microseconds
          *ioSize = sizeof(float);
          break;
    
       case CPU_SPIN_PARAM:
          *last_change = mLastSpinChange;
          *((float*) value) = mSpinPercentage;
          *ioSize = sizeof(float);
          break;
    
       case PRIORITY_PARAM:
          *last_change = mLastPrioChange;
          *((int32*) value) = mPriority;
          *ioSize = sizeof(int32);
          break;
    
       default:
          return B_ERROR;
       }
    
       return B_OK;
    }

The [`BControllable::GetParameterValue()`](BControllable.md#BControllable_GetParameterValue "GetParameterValue\(\), SetParameterValue\(\)") function is called to obtain the current value for one of the node's configurable parameters. The parameter is specified by an ID number in the `id` argument. `value` points to a memory buffer in which the value should be stored, and `ioSize` indicates the size of the buffer. Our job is to store the time at which the indicated parameter last changed into `last_change`, the new value into the buffer pointed at by `value`, and the actual size of the returned value into `ioSize`.

Our logging node begins by logging the request to the log file.

Then the real implementation begins by checking to be sure the space provided by the caller is big enough for the result. In this node, all values are four bytes, so this is easy to check, but in your node, you might have to check on a parameter-by-parameter basis.

Then, based on the `id`, the result is filled out, based on values we've cached from the last change to the parameter values (these would be set when the web receives the appropriate messages indicating that the values have changed). We'll look at the functioning of the [`BParameterWeb`](BParameterWeb.md "BParameterWeb") shortly.

#### SetParameterValue()
    
    
    void LoggingConsumer::SetParameterValue(int32 id,
             bigtime_t performance_time, const void* value, size_t size) {
       log_message logMsg;
       logMsg.now = TimeSource()->Now();
       logMsg.param.id = id;
       mLogger->Log(LOG_SET_PARAM_VALUE, logMsg);
    
       // if it's one of our parameters, enqueue a "set parameter" event for
       // handling at the appropriate time
       switch (id) {
       case LATENCY_PARAM:
       case CPU_SPIN_PARAM:
       case PRIORITY_PARAM:
          {
             media_timed_event event(performance_time,
                   BTimedEventQueue::B_PARAMETER, (void*) value,
                   BTimedEventQueue::B_NO_CLEANUP, size, id, NULL);
             EventQueue()->AddEvent(event);
          }
          break;
    
       default:      // do nothing for other parameter IDs
          break;
       }
       return;
    }

[`BControllable::SetParameterValue()`](BControllable.md#BControllable_SetParameterValue) is called when a request is made to change the value of a parameter. We begin, as usual, by logging the request.

The meat of this function is the switch statement, which enqueues a [media_timed_event](BTimedEventQueue.md#media_timed_event "media_timed_event") representing the request. We instantiate a new media_timed_event with the performance time at which the change is to take place, and all the other parameters needed when changing a parameter's value.

This is then inserted into the event queue by calling [`BTimedEventQueue::AddEvent()`](BTimedEventQueue.md#BTimedEventQueue_AddEvent "AddEvent\(\)") on the [`BMediaEventLooper`](BMediaEventLooper.md "BMediaEventLooper")'s queue, which is returned by [`BMediaEventLooper::EventQueue()`](BMediaEventLooper.md#BMediaEventLooper_EventQueue "EventQueue\(\), RealTimeQueue\(\)"). When the time specified by `performance_time` arrives, this will be dequeued and sent along to [`BMediaEventLooper::HandleEvent()`](BMediaEventLooper.md#BMediaEventLooper_HandleEvent "HandleEvent\(\)") automatically.

The event is given the type `BTimedEventQueue`::`B_PARAMETER`. This type is used for parameter change events.

* * *

### BBufferConsumer Functions

Next we come to the [`BBufferConsumer`](BBufferConsumer.md "BBufferConsumer") functions that we have to implement. These functions actually handle arriving buffers, and negotiations with producers.

#### HandleMessage()
    
    
    status_t LoggingConsumer::HandleMessage(int32 message, const void *data,
             size_t size) {
       log_message logMsg;
       logMsg.now = TimeSource()->Now();
       mLogger->Log(LOG_HANDLE_MESSAGE, logMsg);
    
       return B_ERROR;
    }

`HandleMessage()` should never be called if you're using a [`BMediaEventLooper`](BMediaEventLooper.md "BMediaEventLooper") (if it's called, a message was received that can't be handled at this inheritance level), so we log it and return an error.

#### AcceptFormat()
    
    
    status_t LoggingConsumer::AcceptFormat(const media_destination &dest,
             media_format* format) {
       log_message logMsg;
       logMsg.now = TimeSource()->Now();
       mLogger->Log(LOG_ACCEPT_FORMAT, logMsg);
    
       if (dest != mInput.destination)
           return B_MEDIA_BAD_DESTINATION;
    
       return B_OK;
    }

`AcceptFormat()` is called to confirm that a specified format is acceptable for a given destination. Your implementation needs to look at the format and make this decision, reporting an error if the format is unacceptable.

The example implementation logs the call, then checks to be sure the destination is in fact valid. In this example node, there's only one input allowed, so this is checked easily. If you have an array or linked list of inputs, you'll need to check them all. If the destination isn't recognized, return `B_MEDIA_BAD_DESTINATION`.

Return `B_OK` if the format is acceptable. In this case, all formats are acceptable.

#### GetNextInput()
    
    
    status_t LoggingConsumer::GetNextInput(int32* cookie,
             media_input* out_input) {
       if (0 == *cookie) {
          mInput.format.type = B_MEDIA_UNKNOWN_TYPE;  // accept any format
          *out_input = mInput;
          *cookie = 1;
          return B_OK;
       }
       else return B_BAD_INDEX;
    }

The `GetNextInput()` function is used to iterate through all the inputs your node provides. You should fill `out_input` with the a copy of the requested input's [media_input](TheMediaKit_DefinedTypes.md#media_input "media_input") structure. The cookie is used to specify which input is to be returned; the caller specifies a pointer to 0 the first time `GetNextInput()` is called, and you can set this value appropriately to keep track of where you are scanning your list of inputs.

In this case, there's only one input available, so we return `B_BAD_INDEX` if *`cookie` is nonzero (which indicates that the end of the list has been reached). Otherwise the `out_input` is filled in, and the `cookie` is changed to 1 to indicate that the first input has been scanned. `B_OK` is returned to indicate success.

#### DisposeInputCookie()
    
    
    void LoggingConsumer::DisposeInputCookie(int32 /*cookie*/ ) {
       /* handle disposing of your cookie here, if necessary */
    }

If your cookie isn't a simple integer value, but is actually a pointer, you should dispose of it in your `DisposeInputCookie()` implementation. In this case, we do nothing, since the cookie is an integer.

#### BufferReceived()
    
    
    void LoggingConsumer::BufferReceived(BBuffer* buffer) {
       bigtime_t bufferStart = buffer->Header()->start_time;
       bigtime_t now = TimeSource()->Now();
       bigtime_t how_early = bufferStart-EventLatency()-SchedulingLatency()-now;
    
       log_message logMsg;
       logMsg.now = now;
       logMsg.buffer_data.start_time = bufferStart;
       logMsg.buffer_data.offset = how_early;
       mLogger->Log(LOG_BUFFER_RECEIVED, logMsg);
    
       if (B_MEDIA_PARAMETERS == buffer->Header()->type) {
          ApplyParameterData(buffer->Data(), buffer->SizeUsed());
          buffer->Recycle();
       }
       else {
          status_t err;
          media_timed_event event(buffer->Header()->start_time,
                   BTimedEventQueue::B_HANDLE_BUFFER,
                   buffer, BTimedEventQueue::B_RECYCLE_BUFFER);
          err = EventQueue()->AddEvent(event);
    
          if (err) buffer->Recycle();
       }
    }

`BufferReceived()` is called when buffers arrive on an active connection. Our primary task is to enqueue them for processing at the appropriate time. We begin by logging the arrival of the buffer, including information about the time at which it arrived and how early it arrived.

`B_MEDIA_PARAMETERS` type buffers have to be handled specially (each parameter change in the buffer has its own performance time listed), so we check the buffer's header to see if it's a `B_MEDIA_PARAMETERS` buffer. If it is, we call `ApplyParameterData()` to apply the parameter changes, and the buffer is immediately recycled so it can be reused.

If it's any other type of buffer, a new [media_timed_event](BTimedEventQueue.md#media_timed_event "media_timed_event") is created to reference the new buffer. The event is of type `BTimedEventQueue`::`B_HANDLE_BUFFER`, and the cleanup mode is `BTimedEventQueue`::`B_RECYCLE_BUFFER` so that the buffer will automatically be recycled after the event is processed. Then the new event is enqueued.

If an error occurred trying to enqueue the buffer, it's not in the queue, so we need to recycle it ourselves by calling [`BBuffer::Recycle()`](BBuffer.md#BBuffer_Recycle "Recycle\(\)"). If we didn't do this, we'd leak buffers, which is an embarrassing problem for which there's no known cure. So take proper precautions.

#### ProducerDataStatus()
    
    
    void LoggingConsumer::ProducerDataStatus(
             const media_destination & for_whom, int32 status,
             bigtime_t at_performance_time) {
       log_message logMsg;
       logMsg.now = TimeSource()->Now();
       logMsg.data_status.status = status;
       mLogger->Log(LOG_PRODUCER_DATA_STATUS, logMsg);
    
       if (for_whom == mInput.destination) {
          media_timed_event event(at_performance_time,
                BTimedEventQueue::B_DATA_STATUS, &mInput,
                BTimedEventQueue::B_NO_CLEANUP, status, 0, NULL);
          EventQueue()->AddEvent(event);
       }
    }

`ProducerDataStatus()` is called when the upstream producer's status changes such that it will begin or stop sending buffers. This lets your node optimize its performance based on whether or not buffers are expected. After logging the call, the request is enqueued. Note that it's only queued if the specified destination, `for_whom`, matches one of the node's actual inputs. If it doesn't, the request should be ignored.

The event is given the type `BTimedEventQueue`::`B_DATA_STATUS`, and no cleanup is required.

#### GetLatencyFor()
    
    
    status_t LoggingConsumer::GetLatencyFor(const media_destination &for_whom,
             bigtime_t* out_latency, media_node_id* out_timesource) {
       if (for_whom != mInput.destination) return B_MEDIA_BAD_DESTINATION;
    
       *out_latency = mLatency;
       *out_timesource = TimeSource()->ID();
       return B_OK;
    }

`GetLatencyFor()`'s job is to report the latency for a particular destination. Make sure the destination is a valid one by checking all your inputs to see if any of them have that destination; if none match, return `B_MEDIA_BAD_DESTINATION`.

Otherwise, you should add up the algorithmic and downstream latency (but not the processing and scheduling latencies) for the input, and store that value in `out_latency`, and the node's time source in `out_timesource`, before returning `B_OK`. In our example node, the latency is `mLatency`, which is a user-configurable parameter.

The reason you don't need to include processing and scheduling latency in the result here is this: processing latency is compensated for by [`BMediaEventLooper`](BMediaEventLooper.md "BMediaEventLooper") beginning processing buffers one buffer's worth ahead of time up the chain. Scheduling latency is compensated for in each node.

In this case, the node has no downstream latency (it's not a producer), so the only latency that counts is our internal latency, which is `mLatency`. This is a user-configurable option in this node.

#### Connected()
    
    
    status_t LoggingConsumer::Connected(
          const media_source& producer,
          const media_destination& where,
          const media_format& with_format,
          media_input* out_input) {
       log_message logMsg;
    
       logMsg.now = TimeSource()->Now();
       mLogger->Log(LOG_CONNECTED, logMsg);
    
       if (where != mInput.destination) return B_MEDIA_BAD_DESTINATION;
    
       // calculate my latency here, because it may depend on buffer sizes/durations, then
       // tell the BMediaEventLooper how early we need to get the buffers
       SetEventLatency(mLatency);
    
       // record useful information about the connection, and return success
       mInput.source = producer;
       *out_input = mInput;
       return B_OK;
    }

[`BBufferConsumer::Connected()`](BBufferConsumer.md#BBufferConsumer_Connected "Connected\(\)") is called when a connection between the specified producer, and your node's destination where is established. The connection is logged to the log file.

If the destination isn't valid, `B_MEDIA_BAD_DESTINATION` is returned. Otherwise, the connection's latency is computed and [`BMediaEventLooper::SetEventLatency()`](BMediaEventLooper.md#BMediaEventLooper_SetEventLatency "SetEventLatency\(\), EventLatency\(\)") is called to tell the [`BMediaEventLooper`](BMediaEventLooper.md "BMediaEventLooper") what the latency is. This is an important step, since the [`BMediaEventLooper`](BMediaEventLooper.md "BMediaEventLooper") class handles scheduling, it needs to know the latency!

In this case, we don't do much in the way of computing the latency; we have a known user-configurable value, `mLatency`, but in a real-world node, you need to compute this as accurately as you reasonably can.

Finally, important information about the connection is recorded into the input (the [media_source](TheMediaKit_DefinedTypes.md#media_source "media_source"), `producer`, for example), and we stuff a pointer to the input into `out_input`.

#### Disconnected()
    
    
    void LoggingConsumer::Disconnected(
             const media_source& producer,
             const media_destination& where) {
       log_message logMsg;
       logMsg.now = TimeSource()->Now();
       mLogger->Log(LOG_DISCONNECTED, logMsg);
    
       ::memset(&mInput, 0, sizeof(mInput));
    }

[`BBufferConsumer::Disconnected()`](BBufferConsumer.md#BBufferConsumer_Disconnected "Disconnected\(\)") is called when a connection between the source producer and the destination where is terminated. We log the call to the log file, then perform whatever tasks are necessary on disconnecting the nodes.

In this case, we zero out the [media_input](TheMediaKit_DefinedTypes.md#media_input "media_input") record used by the connection. Your node may have other tasks to perform, depending on your implementation.

#### FormatChanged()
    
    
    status_t LoggingConsumer::FormatChanged(
             const media_source &producer,
             const media_destination &consumer,
             int32 change_tag,
             const media_format& format) {
       log_message logMsg;
       logMsg.now = TimeSource()->Now();
       mLogger->Log(LOG_FORMAT_CHANGED, logMsg);
    
       return B_OK;
    }

[`BBufferConsumer::FormatChanged()`](BBufferConsumer.md#BBufferConsumer_FormatChanged) is called whenever the [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") used for buffers transmitted on the connection between the given producer and consumer changes. This is called in response to your node calling [`BBufferConsumer::RequestFormatChange()`](BBufferConsumer.md#BBufferConsumer_RequestFormatChange "RequestFormatChange\(\), FormatChanged\(\)"); once the requested change has been made, this is called to let you know that it happened. `change_tag` matches the change tag specified returned by `RequestFormatChange()`, and the format indicates the negotiated new [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") for the connection.

Once your `FormatChanged()` function returns, future buffers will be in the new format, so you should prepare for the change at this point. In this case, we don't actually care about the contents of the received buffers, so we do nothing but log that the change occurred and return `B_OK`.

#### SeekTagRequested()
    
    
    status_t LoggingConsumer::SeekTagRequested(
             const media_destination& destination,
             bigtime_t in_target_time,
             uint32 in_flags,
             media_seek_tag* out_seek_tag,
             bigtime_t* out_tagged_time,
             uint32* out_flags) {
       log_message logMsg;
       logMsg.now = TimeSource()->Now();
       mLogger->Log(LOG_SEEK_TAG, logMsg);
    
       return B_OK;
    }

[`BBufferConsumer::SeekTagRequested()`](BBufferConsumer.md#BBufferConsumer_SeekTagRequested "SeekTagRequested\(\)") is called by the Media Server to obtain a seek tag corresponding to the specified `target_time`. Your node needs to cache these tags which are embedded in buffers it receives so it can return them when this function is called.

In this example, we don't support seek tags, so the request is logged and `B_OK` is returned. For more information, see [`BBufferConsumer::SeekTagRequested()`](BBufferConsumer.md#BBufferConsumer_SeekTagRequested "SeekTagRequested\(\)") and "[Seek Tags](BBufferProducer_Overview.md#BBufferProducer_Overview_SeekTags "Seek Tags")".

* * *

### BMediaEventLooper Functions

The node's core is the implementation of the various [`BMediaEventLooper`](BMediaEventLooper.md "BMediaEventLooper") virtual functions.

#### NodeRegistered()
    
    
    void LoggingConsumer::NodeRegistered() {
       log_message logMsg;
       logMsg.now = TimeSource()->Now();
       mLogger->Log(LOG_REGISTERED, logMsg);
    
       SetPriority(mPriority);
       Run();
    
       mInput.destination.port = ControlPort();
       mInput.destination.id = 0;
       mInput.node = Node();
       ::strcpy(mInput.name, "Logged input");
    
       mWeb = build_parameter_web();
       SetParameterWeb(mWeb);
    }

Called when [`BMediaRoster::RegisterNode()`](BMediaRoster.md#BMediaRoster_RegisterNode "RegisterNode\(\), UnregisterNode\(\)") is called, [`BMediaEventLooper::NodeRegistered()`](BMediaEventLooper.md#BMediaEventLooper_NodeRegistered "NodeRegistered\(\)")'s job is to set up the newly-registered node.

In this example, we begin by logging the call, then we set the thread's priority by calling [`BMediaEventLooper::SetPriority()`](BMediaEventLooper.md#BMediaEventLooper_SetPriority "SetPriority\(\), Priority\(\)"), then we run the control thread by calling [`BMediaEventLooper::Run()`](BMediaEventLooper.md#BMediaEventLooper_Run "Run\(\)"). This thread will process events and pass them to our [`BMediaEventLooper::HandleEvent()`](BMediaEventLooper.md#BMediaEventLooper_HandleEvent "HandleEvent\(\)") function at the appropriate times.

Once that's done, we can initialize our inputs. In this case, we only have one, but your node might have multiple inputs. The input's [media_destination](TheMediaKit_DefinedTypes.md#media_destination "media_destination")'s port is set to [`BMediaNode::ControlPort()`](BMediaNode.md#BMediaNode_ControlPort "ControlPort\(\)"). This tells the Media Kit which port to send messages to in order to communicate with the input. The destination's ID is set to a node-defined value—0 in this case, but your node should use a different value for each input you support. And the input's node is set to the result of [`Node()`](BMediaNode.md#BMediaNode_Node "Node\(\)"). We also set the input's name. This name should be unique for every input.

Finally, the node's parameter web is constructed by calling `build_parameter_web()`, and we call [`BControllable::SetParameterWeb()`](BControllable.md#BControllable_SetParameterWeb "SetParameterWeb\(\), Web\(\)") to establish the web.

#### Start(), Stop(), Seek(), TimeWarp()

These functions are called when the corresponding events occur. Usually you don't have to implement these, as your HandleEvent() implementation will receive an appropriate message when the time comes to handle them. In fact, the only reason to implement these at all is if you want to know when one of these events is inserted into the queue.

If you do choose to implement them, you should call through to the base function. Let's look at the implementation of `Start()` as an example:
    
    
    void LoggingConsumer::Start(bigtime_t performance_time) {
       log_message logMsg;
       logMsg.now = TimeSource()->Now();
       mLogger->Log(LOG_START, logMsg);
    
       BMediaEventLooper::Start(performance_time);
    }

This code logs the request, then calls through to [`BMediaNode::Start()`](BMediaNode.md#BMediaNode_Start "Start\(\)"). This will enqueue the request. We'll see how this is handled in `HandleEvent()` below.

#### HandleEvent()

This is the real meat of the node. `HandleEvent()` is called whenever the [`BMediaEventLooper`](BMediaEventLooper.md "BMediaEventLooper") detects that it's time for a queued event to be processed. You should implement the function to handle these events as they're dequeued. The specifics of your implementation will vary depending on what your node does.
    
    
    void LoggingConsumer::HandleEvent(const media_timed_event *event,
             bigtime_t /* lateness */, bool /* realTimeEvent */) {
       log_message logMsg;
       logMsg.now = TimeSource()->Now();
       mLogger->Log(LOG_HANDLE_EVENT, logMsg);

First the call is logged, as is the routine for our example. Next, as we see below, comes a long switch statement that checks for and handles the various event types our node supports.
    
    
       switch (event->type) {
       case BTimedEventQueue::B_HANDLE_BUFFER:
          {
             BBuffer* buffer=const_cast<BBuffer*>((BBuffer*) event->pointer);
             if (buffer) {
                media_header* hdr = buffer->Header();
                if (hdr->destination == mInput.destination.id) {
                   bigtime_t now = TimeSource()->Now();
                   bigtime_t perf_time = hdr->start_time;
    
                   bigtime_t how_early = perf_time - mLatency - now;
    
                   logMsg.buffer_data.start_time = perf_time;
                   logMsg.buffer_data.offset = how_early;
                   logMsg.buffer_data.mode = RunMode();
                   mLogger->Log(LOG_BUFFER_HANDLED, logMsg);
    
                   if ((RunMode() != B_OFFLINE) &&   // lateness doesn't matter in
                                                             // offline mode...
    
                      (RunMode() != B_RECORDING) &&  // ...or in recording mode
    
                      (how_early < 0)) {
                      mLateBuffers++;
                      NotifyLateProducer(mInput.source, -how_early,
    perf_time);
                   }
                   else {
                      // replace this with appropriate code for your node
                      bigtime_t spin_start = ::system_time();
                      bigtime_t spin_now = spin_start;
                      bigtime_t usecToSpin = bigtime_t(mSpinPercentage /
    100.0 * mLatency);
                      while (spin_now - spin_start < usecToSpin) {
                         for (long k = 0; k < 1000000; k++) { /* intentionally blank */ }
                         spin_now = ::system_time();
                      }
                   }
    
                   if ((B_OFFLINE == RunMode()) && (B_DATA_AVAILABLE ==
    mProducerDataStatus)) {
                      status_t err = RequestAdditionalBuffer(mInput.source,
    buffer);
                      if (err) {
                         logMsg.error.error = err;
                         mLogger->Log(LOG_ERROR, logMsg);
                      }
                   }
                }
                else {
                   /* wrong destination! */
                }
    
                buffer->Recycle();
             }
          }
          break;

The first case is the `BTimedEventQueue`::`B_HANDLE_BUFFER` event. This event is received when it's time to handle an incoming buffer.

The buffer is obtained by casting the event's `pointer` field into a [`BBuffer`](BBuffer.md "BBuffer") pointer. If it's `NULL`, nothing is done. Otherwise, the buffer's header is grabbed and stashed into `hdr`. If the destination is invalid, we do nothing (this should never happen, but better safe than sorry).

If the destination is good, it's time to begin processing the buffer. We compute how early the buffer arrived by subtracting the latency and the current performance time from the buffer's performance time.

Then the buffer's performance is logged (this step is of course specific to this particular example node).

If the buffer is late (`how_early` is negative) and we're in neither `B_OFFLINE` nor `B_RECORDING` mode, we ignore the buffer and notify the producer that the buffer was late by calling [`BBufferConsumer::NotifyLateProducer()`](BBufferConsumer.md#BBufferConsumer_NotifyLateProducer "NotifyLateProducer\(\)"). This lets the producer adjust its performance to attempt to avoid future buffers from arriving late. We allow tardiness in `B_OFFLINE` and `B_RECORDING` modes because lateness doesn't matter in these cases.

If the buffer is on time, we process the buffer. In this example, we just waste some processing time, rougly corresponding to our latency. In a real node, you'd replace this code with code that handles the buffer, whether by displaying it to the screen or playing it through speakers, or whatever might be appropriate.

If the node is in `B_OFFLINE` mode, and the cached producer data status is `B_DATA_AVAILABLE` (meaning the producer has more buffers to send us), we must call [`BBufferConsumer::RequestAdditionalBuffer()`](BBufferConsumer.md#BBufferConsumer_RequestAdditionalBuffer "RequestAdditionalBuffer\(\)"). to tell the producer we're ready for another buffer. If we fail to call this, performance will suffer greatly. If `RequestAdditionalBuffer()` fails, we log the error.

Once processing of the buffer is done, we call [`BBuffer::Recycle()`](BBuffer.md#BBuffer_Recycle "Recycle\(\)") to make it available for future use.

The next possible event is the `BTimedEventQueue`::`B_PARAMETER`, which indicates a change to the parameters' values.
    
    
       case BTimedEventQueue::B_PARAMETER:
          {
             size_t dataSize = size_t(event->data);
             int32 param = int32(event->bigdata);
             logMsg.param.id = param;
    
             if (dataSize >= sizeof(float)) switch (param) {
             case LATENCY_PARAM:
                {
                   float value = *((float*) event->user_data);
                   mLatency = bigtime_t(value* 1000);
                   mLastLatencyChange = logMsg.now;
    
                   SetEventLatency(mLatency);
    
                   SendLatencyChange(mInput.source, mInput.destination,
    EventLatency() + SchedulingLatency());
                   BroadcastNewParameterValue(logMsg.now, param, &value,
    sizeof(value));
    
                   logMsg.param.value = value;
                   mLogger->Log(LOG_SET_PARAM_HANDLED, logMsg);
                }
                break;

If the latency parameter is changed, we record the new latency in `mLatency`, and the time at which the change occurred in `mLastLatencyChange`, and we call [`BMediaEventLooper::SetEventLatency()`](BMediaEventLooper.md#BMediaEventLooper_SetEventLatency "SetEventLatency\(\), EventLatency\(\)") to let the control thread know about the latency change.

In addition, the new latency is sent to the producer by calling [`BBufferConsumer::SendLatencyChange()`](BBufferConsumer.md#BBufferConsumer_SendLatencyChange "SendLatencyChange\(\)"). This lets the producer alter its behavior as appropriate to avoid running amok (or falling behind, depending on the new latency). And [`BControllable::BroadcastNewParameterValue()`](BControllable.md#BControllable_BroadcastNewParameterValue "BroadcastNewParameterValue\(\)") is called to share the change to the parameter's value with anyone that's watching for changes.

Finally, we log the change to disk.
    
    
             case CPU_SPIN_PARAM:
                {
                   float value = *((float*) event->user_data);
                   mSpinPercentage = value;
                   mLastSpinChange = logMsg.now;
                   BroadcastNewParameterValue(logMsg.now, param, &value,
    sizeof(value));
                   logMsg.param.value = value;
                   mLogger->Log(LOG_SET_PARAM_HANDLED, logMsg);
                }
                break;

Similarly, if the CPU spin parameter is changed, we record the new value and the time at which the change occurred, then broadcast the change to the world and log the change. Note that since this doesn't correspond to anything [`BMediaEventLooper`](BMediaEventLooper.md "BMediaEventLooper") cares about, we have no additional work to do.
    
    
             case PRIORITY_PARAM:
                {
                   mPriority = *((int32*) event->user_data);
                   SetPriority(mPriority);
    
                   mLastPrioChange = logMsg.now;
                   BroadcastNewParameterValue(logMsg.now, param, &mPriority,
    sizeof(mPriority));
                   logMsg.param.value = (float) mPriority;
                   mLogger->Log(LOG_SET_PARAM_HANDLED, logMsg);
                }
                break;

When the priority parameter is changed, we call [`BMediaEventLooper::SetPriority()`](BMediaEventLooper.md#BMediaEventLooper_SetPriority "SetPriority\(\), Priority\(\)") to tell the control thread about its new priority. You must never directly change the thread's priority, since the priority affects the functioning of the node, and there are other changes that have to be made based upon the change.

The new value is saved, along with the change time, and the value is broadcast and the log entry is recorded to disk.
    
    
             default:
                mLogger->Log(LOG_INVALID_PARAM_HANDLED, logMsg);
                break;
             }
          }
          break;

Here we deal with the case where a nonexistent parameter was changed. This error condition is logged to disk. You should deal gracefully with this possibility, although it should never happen.

The next event type that needs to be handled is `BTimedEventQueue`::`B_START`, which is received when it's time to start processing buffers:
    
    
       case BTimedEventQueue::B_START:
          mLogger->Log(LOG_START_HANDLED, logMsg);
          break;

In this example node, we simply log the start request. Your node should insert whatever code is necessary to prepare to process buffers. If your node is a producer, you should start sending buffers at this point. Be sure to call [`BBufferProducer::SendDataStatus()`](BBufferProducer.md#BBufferProducer_SendDataStatus "SendDataStatus\(\)") to let the consumer know that you're sending buffers.

The next event type is `BTimedEventQueue`::`B_STOP`, which is received when it's time to stop processing buffers:
    
    
       case BTimedEventQueue::B_STOP:
          mLogger->Log(LOG_STOP_HANDLED, logMsg);
          EventQueue()->FlushEvents(0, BTimedEventQueue::B_ALWAYS, true,
                BTimedEventQueue::B_HANDLE_BUFFER);
          break;

Stopping your node implies that any buffers you've received to date that haven't been processed yet should be ignored, so the event queue is flushed of al `BTimedEventQueue`::`B_HANDLE_BUFFER` events. If you have other tasks that need to be performend when your node is stopped, this is the place to handle them.

Note that if your node is a producer, you should call [`BBufferProducer::SendDataStatus()`](BBufferProducer.md#BBufferProducer_SendDataStatus "SendDataStatus\(\)") here to let the consumer know that no more buffers will be arriving.

The `BTimedEventQueue`::`B_SEEK` request is received when it's time to perform a seek operation on the node's media.
    
    
       case BTimedEventQueue::B_SEEK:
          mLogger->Log(LOG_SEEK_HANDLED, logMsg);
          break;

Handle the seek operation here. In this node, we just log the seek request.

The `BTimedEventQueue`::`B_WARP` request is received when it's time to perform a time warp operation on the node'.
    
    
       case BTimedEventQueue::B_WARP:
          mLogger->Log(LOG_WARP_HANDLED, logMsg);
          break;

In this example, we just log the request.

The `BTimedEventQueue`::`B_DATA_STATUS` event is received when the producer's [`BBufferProducer::SendDataStatus()`](BBufferProducer.md#BBufferProducer_SendDataStatus "SendDataStatus\(\)") function is called. Obviously you only need to handle this event if you're a consumer:
    
    
       case BTimedEventQueue::B_DATA_STATUS:
          mProducerDataStatus = event->data;
          logMsg.data_status.status = event->data;
          mLogger->Log(LOG_DATA_STATUS_HANDLED, logMsg);
          break;

In our node, we record the producer's current data status in the member variable `mProducerDataStatus`. We use this information to determine whether or not we're expecting buffers. Your node might wish to alter its priority or perform other optimizations if the producer stops sending buffers, or increase priority when buffers start arriving again. We also log the change.
    
    
       default:
          logMsg.unknown.what = event->type;
          mLogger->Log(LOG_HANDLE_UNKNOWN, logMsg);
          break;
       }
    }

Finally, we log any messages received that we don't understand. In general you can ignore messages your node isn't prepared to handle.

* * *

### Creating a Parameter Web

A parameter web describes to the Media Kit the various user-configurable options that might interest them, and how they should appear in a user interface. This information is used by the media theme to render the interface that the user sees when configuring the node via the preference panel instantiated by calling [`BBufferProducer::BMediaRoster::StartControlPanel()`](BMediaRoster.md#BMediaRoster_StartControlPanel "StartControlPanel\(\)").

Creating a [`BParameterWeb`](BParameterWeb.md "BParameterWeb") is simple. You begin by creating the [`BParameterGroup`](BParameterGroup.md "BParameterGroup")s that logically group related parameters, then you insert parameters into each group by calling the appropriate [`BParameterGroup`](BParameterGroup.md "BParameterGroup") functions for the various types of parameters that can be created.

[`BBufferProducer::BParameterGroup::MakeNullParameter()`](BParameterGroup.md#BParameterGroup_MakeNullParameter "MakeNullParameter\(\)") can be used to create a label or other non-configurable control that might appear in the interface.

[`BBufferProducer::BParameterGroup::MakeContinuousParameter()`](BParameterGroup.md#BParameterGroup_MakeContinuousParameter "MakeContinuousParameter\(\)") creates a parameter with a floating-point value, which is usually set using a slider control.

[`BParameterGroup::MakeDiscreteParameter()`](BParameterGroup.md#BParameterGroup_MakeDiscreteParameter "MakeDiscreteParameter\(\)") creates a parameter with a set of discrete possible values. These are usually displayed as pop-up menus, although some themes might use radio buttons or lists.
    
    
    static BParameterWeb* build_parameter_web() {
       BParameterWeb* web = new BParameterWeb;
    
       BParameterGroup* mainGroup = web->MakeGroup("LoggingConsumer
          Parameters");
    
       BParameterGroup* group = mainGroup->MakeGroup("Latency Control");
    
       BParameter* nullParam = group->MakeNullParameter(INPUT_NULL_PARAM,
          B_MEDIA_NO_TYPE, "Latency", B_GENERIC);
    
       BParameter* latencyParam =
          group->MakeContinuousParameter(LATENCY_PARAM, B_MEDIA_NO_TYPE, "",
             B_GAIN, "msec", 5, 100, 5);
    
       nullParam->AddOutput(latencyParam);
       latencyParam->AddInput(nullParam);
    
       group = mainGroup->MakeGroup("CPU Percentage");
       nullParam = group->MakeNullParameter(CPU_NULL_PARAM, B_MEDIA_NO_TYPE,
          "CPU Spin Percentage", B_GENERIC);
    
       BContinuousParameter* cpuParam =
          group->MakeContinuousParameter(CPU_SPIN_PARAM, B_MEDIA_NO_TYPE, "",
             B_GAIN, "percent", 5, 80, 5);
    
       nullParam->AddOutput(cpuParam);
       cpuParam->AddInput(nullParam);
    
       group = mainGroup->MakeGroup("Priority");
       nullParam = group->MakeNullParameter(PRIO_NULL_PARAM, B_MEDIA_NO_TYPE,
          "Thread Priority", B_GENERIC);
    
       DiscreteParameter* prioParam =
          group->MakeDiscreteParameter(PRIORITY_PARAM, B_MEDIA_NO_TYPE, "",
             B_GENERIC);
    
       prioParam->AddItem(5, "B_LOW_PRIORITY");
       prioParam->AddItem(10, "B_NORMAL_PRIORITY");
       prioParam->AddItem(15, "B_DISPLAY_PRIORITY");
       prioParam->AddItem(20, "B_URGENT_DISPLAY_PRIORITY");
       prioParam->AddItem(100, "B_REAL_TIME_DISPLAY_PRIORITY");
       prioParam->AddItem(110, "B_URGENT_PRIORITY");
       prioParam->AddItem(120, "B_REAL_TIME_PRIORITY");
    
       return web;
    }

Notice the use of the [`BDiscreteParameter::AddItem()`](BDiscreteParameter.md#BDiscreteParameter_AddItem "AddItem\(\)") function to add the discrete values to the priority parameter. Each item has a value and a label that's displayed in the user interface. In this example, these correspond to the various thread priorities and their names.

The user interface provided by this node lets the user configure the behavior of the node, including the thread priority, how busy the CPU should be while a buffer is "processed," and the latency of buffer processing.

Obviously your node will have different parameters (and may have none, in which case you wouldn't even derive from [`BControllable`](BControllable.md "BControllable"), and wouldn't need a [`BParameterWeb`](BParameterWeb.md "BParameterWeb") at all).

This function is called from the [`BMediaEventLooper::NodeRegistered()`](BMediaEventLooper.md#BMediaEventLooper_NodeRegistered "NodeRegistered\(\)") implementation above. NodeRegistered() then calls SetParameterWeb() to establish the web for use by StartControlPanel() and other functions that use the web.

This node's web, as interpreted by the default system theme in Release 4.5, looks like this:

* * *

### Producer-specific Issues

There are obviously some additional virtual functions you need to implement if your node derives from [`BBufferProducer`](BBufferProducer.md "BBufferProducer").

#### Connect()

Your [`BBufferProducer::Connect()`](BBufferProducer.md#BBufferProducer_Connect "Connect\(\)") implementation should call [`BMediaEventLooper::SetEventLatency()`](BMediaEventLooper.md#BMediaEventLooper_SetEventLatency "SetEventLatency\(\), EventLatency\(\)") to establish your total latency. This value, which is your internal latency plus the downstream latency, is used by [`BMediaEventLooper`](BMediaEventLooper.md "BMediaEventLooper") to determine when to pop events off the queue for you to process. By keeping this value up-to-date, you can improve performance. This code might resemble the following:
    
    
       /* calculate processing latency */
    
       bigtime_t latency = calculate_buffer_latency();
       latency += estimate_max_scheduling_latency();
    
       /* calculate downstream latency */
    
       bigtime_t downstream;
       media_node_id timesource;
       FindLatencyFor(output.destination, &downstream, &timesource);
    
       bigtime_t totalLatency = latency + downstream;
       SetEventLatency(totalLatency);

* * *

### The Rest of the Story…

The `LogWriter` class' implementation isn't critical to this example, and is cleverly ignored in the name of saving space. If you're curious, you can reference the complete sample code for the LoggingApplication, at ftp://ftp.be.com/pub/samples/media_kit/LoggingConsumer.zip.

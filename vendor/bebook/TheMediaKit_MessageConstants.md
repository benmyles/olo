<!-- Classes And Methods - The Media Kit - Message Constants -->

## Message Constants

The Media Kit has a number of message formats. The following table lists the messages (by their 'what' constants) and describes their intents. The subsequent sections list the fields that are contained in the messages.

* * *

### Notification Messages

Declared in: `media/MediaDefs.h`

Messages with these 'what' are sent by the media server to any application that's registered to receive notifications by calling [`BMediaRoster::StartWatching()`](BMediaRoster.md#BMediaRoster_StartWatching "StartWatching\(\), StopWatching\(\)").

Constant| Description  
---|---  
[`B_MEDIA_WILDCARD`](TheMediaKit_MessageConstants.md#B_MEDIA_WILDCARD "B_MEDIA_WILDCARD")| Used to match any notification (used only in calls to [`StartWatching()`](BMediaRoster.md#BMediaRoster_StartWatching "StartWatching\(\), StopWatching\(\)") and [`StopWatching()`](BMediaRoster.md#BMediaRoster_StopWatching)).  
[`B_MEDIA_NODE_CREATED`](TheMediaKit_MessageConstants.md#B_MEDIA_NODE_CREATED "B_MEDIA_NODE_CREATED, B_MEDIA_NODE_DELETED")| A new node has been created  
[`B_MEDIA_NODE_DELETED`](TheMediaKit_MessageConstants.md#B_MEDIA_NODE_DELETED)| A node has been deleted. (Not sent in R4.5.)  
[`B_MEDIA_CONNECTION_MADE`](TheMediaKit_MessageConstants.md#B_MEDIA_CONNECTION_MADE "B_MEDIA_CONNECTION_MADE, B_MEDIA_CONNECTION_BROKEN")| Not sent in R4.5.  
[`B_MEDIA_CONNECTION_BROKEN`](TheMediaKit_MessageConstants.md#B_MEDIA_CONNECTION_BROKEN)| Not sent in R4.5.  
[`B_MEDIA_BUFFER_CREATED`](TheMediaKit_MessageConstants.md#B_MEDIA_BUFFER_CREATED "B_MEDIA_BUFFER_CREATED, B_MEDIA_BUFFER_DELETED")| Not sent in R4.5.  
[`B_MEDIA_BUFFER_DELETED`](TheMediaKit_MessageConstants.md#B_MEDIA_BUFFER_DELETED)| Not sent in R4.5.  
[`B_MEDIA_FORMAT_CHANGED`](TheMediaKit_MessageConstants.md#B_MEDIA_FORMAT_CHANGED "B_MEDIA_FORMAT_CHANGED")| Not sent in R4.5.  
[`B_MEDIA_PARAMETER_CHANGED`](TheMediaKit_MessageConstants.md#B_MEDIA_PARAMETER_CHANGED "B_MEDIA_PARAMETER_CHANGED, B_MEDIA_NEW_PARAMETER_VALUE")| Indicates that a parameter in a [`BParameterWeb`](BParameterWeb.md "BParameterWeb") has changed.  
[`B_MEDIA_NEW_PARAMETER_VALUE`](TheMediaKit_MessageConstants.md#B_MEDIA_NEW_PARAMETER_VALUE)|  Indicates that a parameter in a BParameterWeb has changed. Unlike the B_MEDIA_PARAMETER_CHANGED notification, this includes the new value.  
[`B_MEDIA_WEB_CHANGED`](TheMediaKit_MessageConstants.md#B_MEDIA_WEB_CHANGED "B_MEDIA_WEB_CHANGED")| Indicates that a node's [`BParameterWeb`](BParameterWeb.md "BParameterWeb") has changed.  
[`B_MEDIA_NODE_STOPPED`](TheMediaKit_MessageConstants.md#B_MEDIA_NODE_STOPPED "B_MEDIA_NODE_STOPPED")| Indicates that a node has stopped.  
[`B_MEDIA_FLAVORS_CHANGED`](TheMediaKit_MessageConstants.md#B_MEDIA_FLAVORS_CHANGED "B_MEDIA_FLAVORS_CHANGED")| Indicates that flavors have changed.  
`B_MEDIA_DEFAULT_CHANGED`| Not sent in R4.5.  
`B_MEDIA_TRANSPORT_STATE`| Not sent in R4.5.  
  
#### B_MEDIA_WILDCARD

Purpose:| Constant only.  
---|---  
Source:| Your application.  
Target:| The media server.  
Hook:|   
  
When calling [`BMediaRoster::StartWatching()`](BMediaRoster.md#BMediaRoster_StartWatching "StartWatching\(\), StopWatching\(\)") or [`BMediaRoster::StopWatching()`](BMediaRoster.md#BMediaRoster_StopWatching), you can use this constant to match all media messages. This message has no actual format.

(No Be-defined fields)

####  B_MEDIA_NODE_CREATED, B_MEDIA_NODE_DELETED

Purpose:| Deliverable  
---|---  
Source:| The media server.  
Target:| Target specified to [`BMediaRoster::StartWatching()`](BMediaRoster.md#BMediaRoster_StartWatching "StartWatching\(\), StopWatching\(\)").  
Hook:|   
  
Sent to indicate that a node has been created or deleted in the media system.

Field| Type code| Description  
---|---|---  
`media_node_id`| `B_INT32_TYPE`| The ID of the [media_node](TheMediaKit_DefinedTypes.md#media_node "media_node") that was created or deleted.  
  
####  B_MEDIA_CONNECTION_MADE, B_MEDIA_CONNECTION_BROKEN

Purpose:| Deliverable  
---|---  
Source:| The media server.  
Target:| Target specified to [`BMediaRoster::StartWatching()`](BMediaRoster.md#BMediaRoster_StartWatching "StartWatching\(\), StopWatching\(\)").  
Hook:| –  
  
Sent to indicate that a connection between media nodes has been made or broken.

The `B_MEDIA_CONNECTION_MADE` message has three data fields:

Field| Type code| Description  
---|---|---  
`be:output`| `B_RAW_TYPE`| A [media_output](TheMediaKit_DefinedTypes.md#media_output "media_output") structure describing the connection's output.  
`be:input`| `B_RAW_TYPE`| A [media_input](TheMediaKit_DefinedTypes.md#media_input "media_input") structure describing the connection's input.  
`be:format`| `B_RAW_TYPE`| A [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") structure describing the format of the data that flows across the connection.  
  
And `B_MEDIA_CONNECTION_BROKEN` has two.

Field| Type code| Description  
---|---|---  
`destination`| `B_RAW_TYPE`| A [media_destination](TheMediaKit_DefinedTypes.md#media_destination "media_destination") structure describing the destination of the connection that's been broken.  
`source`| `B_RAW_TYPE`| A [media_source](TheMediaKit_DefinedTypes.md#media_source "media_source") structure describing the source of the connection that's been broken.  
  
####  B_MEDIA_BUFFER_CREATED, B_MEDIA_BUFFER_DELETED

Purpose:| Deliverable  
---|---  
Source:| The media server.  
Target:| Target specified to [`BMediaRoster::StartWatching()`](BMediaRoster.md#BMediaRoster_StartWatching "StartWatching\(\), StopWatching\(\)").  
Hook:| –  
  
Sent to indicate that a media buffer has been created or deleted.

`B_MEDIA_BUFFER_CREATED`'s message has the following fields:

Field| Type code| Description  
---|---|---  
`clone_info`| `B_RAW_TYPE`| An area_info structure describing the buffer's location in memory. This is an array, one entry per buffer created.  
  
`B_MEDIA_BUFFER_DELETED` looks like this:

Field| Type code| Description  
---|---|---  
`media_buffer_id`| `B_INT32_TYPE`| The buffer ID number of the buffer being deleted. This is an array, one entry per buffer deleted.  
  
#### B_MEDIA_FORMAT_CHANGED

Purpose:| Deliverable  
---|---  
Source:| The media server.  
Target:| Target specified to [`BMediaRoster::StartWatching()`](BMediaRoster.md#BMediaRoster_StartWatching "StartWatching\(\), StopWatching\(\)").  
Hook:|   
  
Sent by the media server to indicate that the format used on a particular connection has changed.

Field| Type code| Description  
---|---|---  
`be:source`| `B_RAW_TYPE`| A [media_source](TheMediaKit_DefinedTypes.md#media_source "media_source") structure describing the source of the connection whose format changed.  
`be:destination`| `B_RAW_TYPE`| A [media_source](TheMediaKit_DefinedTypes.md#media_source "media_source") structure describing the source of the connection whose format changed.  
`be:format`| `B_RAW_TYPE`| A [media_format](TheMediaKit_DefinedTypes.md#media_format "media_format") structure describing the new format.  
  
####  B_MEDIA_PARAMETER_CHANGED, B_MEDIA_NEW_PARAMETER_VALUE

Purpose:| Deliverable  
---|---  
Source:| The media server.  
Target:| Target specified to [`BMediaRoster::StartWatching()`](BMediaRoster.md#BMediaRoster_StartWatching "StartWatching\(\), StopWatching\(\)").  
Hook:|   
  
Sent by the media server to indicate that the value of a parameter has changed. `B_MEDIA_PARAMETER_CHANGED` only tells you which parameter changed ([media_node](TheMediaKit_DefinedTypes.md#media_node "media_node") and parameter ID). `B_MEDIA_NEW_PARAMETER_VALUE` also tells you which parameter changed, and what the new value is.

The `B_MEDIA_PARAMETER_CHANGED` message looks like this:

Field| Type code| Description  
---|---|---  
`be:node`| `B_RAW_TYPE`| A [media_node](TheMediaKit_DefinedTypes.md#media_node "media_node") structure indicating which node's parameter web has changed.  
`be:parameter`| `B_INT32_TYPE`| The ID number of the parameter whose value has changed.  
  
The `B_MEDIA_NEW_PARAMETER_VALUE` message is:

Field| Type code| Description  
---|---|---  
`node`| `B_RAW_TYPE`| A [media_node](TheMediaKit_DefinedTypes.md#media_node "media_node") structure indicating which node is reporting a changed parameter value.  
`parameter`| `B_INT32_TYPE`| The parameter ID of the changed parameter.  
`when`| `B_INT64_TYPE`| The performance time at which the change took effect. `value` `B_RAW_TYPE` The parameter's new value.  
  
#### B_MEDIA_WEB_CHANGED

Purpose:| Deliverable  
---|---  
Source:| The media server.  
Target:| Target specified to [`BMediaRoster::StartWatching()`](BMediaRoster.md#BMediaRoster_StartWatching "StartWatching\(\), StopWatching\(\)").  
Hook:|   
  
Sent by the media server to indicate that a particular node's [`BParameterWeb`](BParameterWeb.md "BParameterWeb") has.

Field| Type code| Description  
---|---|---  
`node`| `B_RAW_TYPE`| A [media_node](TheMediaKit_DefinedTypes.md#media_node "media_node") structure indicating which node's parameter web has changed.  
  
#### B_MEDIA_NODE_STOPPED

Purpose:| Deliverable  
---|---  
Source:| The media server.  
Target:| Target specified to [`BMediaRoster::StartWatching()`](BMediaRoster.md#BMediaRoster_StartWatching "StartWatching\(\), StopWatching\(\)").  
Hook:|   
  
Indicates that a media node has stopped.

#### B_MEDIA_FLAVORS_CHANGED

Purpose:| Deliverable  
---|---  
Source:| The media server.  
Target:| Target specified to [`BMediaRoster::StartWatching()`](BMediaRoster.md#BMediaRoster_StartWatching "StartWatching\(\), StopWatching\(\)").  
Hook:| –  
  
Sent by the media server to indicate that the flavors supported by a particular add-on have changed.

Field| Type code| Description  
---|---|---  
`be:addon_id`| `B_INT32_TYPE`| The add-on ID of the add-on whose flavors have changed  
`be:new_count`| `B_INT32_TYPE`| How many new flavors have been added.  
`be:gone_count`| `B_INT32_TYPE`| How many flavors have been removed.

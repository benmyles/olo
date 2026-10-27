<!-- Classes And Methods - The Storage Kit - Message Constants -->

## Message Constants

* * *

### File Panel Messages

The file panel produces three messages: `B_REFS_RECEIVED`, `B_SAVE_REQUESTED`, and `B_CANCEL`. The first of these was discussed [here](TheApplicationKit_MessageConstants.md#B_REFS_RECEIVED "B_REFS_RECEIVED"). It's produced when the user picks files to open from the panel. The other two messages are described below.

  * [`B_CANCEL`](TheStorageKit_MessageConstants.md#B_CANCEL "B_CANCEL")

  * [`B_SAVE_REQUESTED`](TheStorageKit_MessageConstants.md#B_SAVE_REQUESTED "B_SAVE_REQUESTED")

#### B_CANCEL

A cancel notification is sent whenever a file panel is hidden. This includes the Cancel button being clicked, the panel being closed, and the panel being hidden after an open or a save.

Field| Type code| Description  
---|---|---  
`old_what`| `B_INT32_TYPE`| The "previous" what value. This is only useful (and dependable) if you supplied the [`BFilePanel`](BFilePanel.md "BFilePanel") with your own [`BMessage`](BMessage.md "BMessage"): The `what` from your message is moved to the `old_what` field. If you didn't supply a [`BMessage`](BMessage.md "BMessage"), you should ignore this field (it could contain garbage).  
`source`| `B_POINTER_TYPE`| A pointer to the [`BFilePanel`](BFilePanel.md "BFilePanel") object.  
  
#### B_SAVE_REQUESTED

The file panel produces this message when the user asks the application to save a document. It has two data fields:

Field| Type code| Description  
---|---|---  
`directory`| `B_REF_TYPE`| An entry_ref referring to the directory where the document should be saved.  
`name`| `B_STRING_TYPE`| The file name under which the document should be saved.  
  
* * *

### Query Messages

#### B_QUERY_UPDATE

Purpose:| Deliverable  
---|---  
Source:| The system.  
Target:| App-defined.  
Hook:|   
  
Sent when the results of a live query change: If a new file meets the query requirements, [`B_ENTRY_CREATED`](The_Node_Monitor.md#B_ENTRY_CREATED "B_ENTRY_CREATED") is sent. If a file that previously passed the query requirements no longer does, [`B_ENTRY_REMOVED`](The_Node_Monitor.md#B_ENTRY_REMOVED "B_ENTRY_REMOVED") is sent.

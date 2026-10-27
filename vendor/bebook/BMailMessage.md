<!-- Classes And Methods - The Mail Kit - BMailMessage -->

## BMailMessage

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `mail/E-mail.h`  
Library:| `libmail.so`  
Allocation:| –  
[Class Overview](BMailMessage_Overview.md "BMailMessage")

  * [Constructor and Destructor](BMailMessage.md#BMailMessage_ConstructorDestructor)
  * [Member Functions](BMailMessage.md#BMailMessage_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BMailMessage()

`BMailMessage();`

Creates and returns a new `BMailMessage` object, which is empty. You need to call other functions defined by this class to fill out the message.

#### BMailMessage()

`BMailMessage();`

Destroys the `BMailMessage`, even if the object's fields are "dirty." For example, if you create a new `BMailMessage` object with the intention of sending a message, fill out some or all of the fields, and then delete the object, the object is destroyed without being sent.

* * *

### Member Functions

#### AddContent()

`status_t AddContent(const char* text,  
int32 length,  
uint32 encoding = B_ISO1_CONVERSION,  
bool replace = false);` `status_t AddContent(const char* text,  
int32 length,  
const char* encoding,  
bool replace = false);`

Adds the specified text (which contains length characters) to the `BMailMessage` object's content. The text's encoding is specified by the `encoding` parameter, either directly or by pointer.

If `replace` is `true`, any existing text is deleted before the new content is added; otherwise, the specified text is appended to the end of the existing message content.

Return Code| Description  
---|---  
`B_OK`.| The content was changed without error.  
`B_ERROR`.| Unable to add the new content.  
  
#### AddEnclosure()

`status_t AddEnclosure(entry_ref* enclosure_ref,  
bool replace = false);` `status_t AddEnclosure(const char* path,  
bool replace = false);` `status_t AddEnclosure(const char* mime_type,  
void* data,  
int32 length,  
bool replace = false);`

Adds an attachment to the message. The first two forms of `AddEnclosure()` add a file to the message, given either an entry_ref pointer or a pathname. The third form adds a block of memory (of the given `length`) to the message as an enclosure, with the specified MIME type.

If `replace` is `true`, any existing attachments—including the body of the message—are removed before the new one is added; otherwise the new enclosure is added, leaving previous attachments intact.

If you specify `true` for `replace`, not only will all existing enclosures be discarded, but so will the content of the message body itself.

Return Code| Description  
---|---  
`B_OK`.| The content was changed without error.  
`B_ERROR`.| Unable to add the new enclosure.  
  
#### AddHeaderField()

`status_t AddHeaderField(const char* field_name,  
const char* field_str,  
bool replace = false);`

Adds a header field to the `BMailMessage` object. The value of the field whose name is specified by `field_name` is set to the string specified by `field_str`.

If `replace` is `true`, all existing header fields of the specified name are deleted before adding the new header field; if `replace` is `false`, a new header whose field is named `field_name` is added.

Return Code| Description  
---|---  
`B_OK`.| The content was changed without error.  
`B_ERROR`.| Unable to add the new header field.  
  
#### Send()

`status_t Send(bool send_now = false,  
bool remove_when_sent = false);`

Queues the message for transmission. If `send_now` is `true`, the message is sent immediately; otherwise, it is placed in the queue to be sent the next time [`check_for_mail()`](TheMailDaemon_Functions.md#check_for_mail "check_for_mail\(\)") is called or the mail daemon performs an automatic mail check.

If the `remove_when_sent` argument is `true`, the message will be deleted from the user's disk drive after it has been sent; otherwise, it will be saved for posterity.

Return Code| Description  
---|---  
`B_OK`.| The message was queued successfully  
`B_MAIL_NO_RECIPIENT`.| There needs to be either a "To" or "Bcc" field in the message.

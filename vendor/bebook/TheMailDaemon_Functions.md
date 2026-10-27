<!-- Classes And Methods - The Mail Kit - The Mail Daemon Functions -->

## The Mail Daemon Functions

* * *

###  check_for_mail()

`status_t check_for_mail(int32* incoming_count = NULL);`

Sends and retrieves mail. More specifically, this function asks the mail daemon to retrieve incoming messages from the POP server and send any queued outgoing messages to the SMTP server. The number of POP messages that were retrieved are stored in the variable pointed to by `incoming_count`. If you specify `NULL` for `incoming_count`, `check_for_mail()` won't return the number of messages retrieved. You should specify `NULL` unless you really want to know how many messages were retrieved, since requesting this information could potentially slow down the retrieval process.

If all is well in the mail world, this function returns `B_OK`; otherwise, it returns a highly useful result code.

Return Code| Description  
---|---  
`B_OK`| Mail was sent and retrieved without incident.  
`B_MAIL_NO_DAEMON`| The mail daemon isn't running.  
`B_MAIL_UNKNOWN_USER`| The POP server doesn't recognize the user name.  
`B_MAIL_WRONG_PASSWORD`| The POP server doesn't recognize the password.  
`B_MAIL_UNKNOWN_HOST`| The POP or SMTP server can't be found.  
`B_MAIL_ACCESS_ERROR`| The connection to the POP or SMTP server failed.  
  
* * *

### count_pop_accounts()

`int32 count_pop_accounts();`

Returns the number of POP accounts that have been configured.

Note

![Note](./images/admonitions/Info_32.png)

The mail daemon currently supports only one POP account, so this function will always return 1. You shouldn't assume there will only be one POP account, though, as this will probably change in the future.

* * *

### decode_base64()

`ssize_t decode_base64(char* out,  
char* in,  
off_t length,  
bool replace_cr = false);`

Decodes the base-64 data pointed to by `in`, which is `length` bytes long, and writes the decoded output into the buffer pointed to by `out`. If `replace_cr` is `true`, carriage return characters in the output are converted into newlines, otherwise the data is returned in its original, unaltered, form.

You would typically specify `replace_cr` as `true` if you're decoding an ASCII text document, and as `false` if decoding a binary file.

This function returns the size of the output data that's been stored in the `out` buffer.

Warning

![Warning](./images/admonitions/Stop_32.png)

You must be certain, in advance, that the output buffer is large enough to hold the decoded data, or this function will do bad things.

* * *

### encode_base64()

`ssize_t encode_base64(char* out,  
char* in,  
off_t length);`

Encodes the data pointed to by `in`, which is `length` bytes long, and writes the base-64 encoded output into the buffer pointed to by `out`.

This function returns the size of the output data that's been stored in the `out` buffer.

Warning

![Warning](./images/admonitions/Stop_32.png)

You must be certain, in advance, that the output buffer is large enough to hold the encoded data, or this function will do bad things.

* * *

### forward_mail()

`status_t forward_mail(entry_ref* message_ref,  
const char* recipients,  
bool now = true);`

Forwards the mail message specified by `message_ref` to the list of users given by `recipients`. The list of user names specified in `recipients` must be separated by commas and/or whitespace, and must be null-terminated.

If the `now` parameter is `true`, the messages will be sent immediately; if `false`, the message will be queued up to be sent the next time [`check_for_mail()` ](TheMailDaemon_Functions.md#check_for_mail "check_for_mail\(\)") is called, or the next time the mail daemon performs an automatic mail check.

Return Code| Description  
---|---  
`B_OK`| The message was forwarded without error.  
`B_MAIL_NO_RECIPIENT`| No valid recipients were specified.  
Other Errors| Errors returned by [`send_queued_mail()`](TheMailDaemon_Functions.md#send_queued_mail "send_queued_mail\(\)"), if `now` is `true`.  
  
* * *

###  get_mail_notification(), set_mail_notification(), mail_notification

`status_t get_mail_notification(mail_notification* notification_settings);``status_t set_mail_notification(mail_notification* notification_settings,  
bool save = true);`

`get_mail_notification()` fills the specified mail_notification structure with information describing how the user is currently being notified of received e-mail. There are two possible notification signals: the mail alert panel and the system beep. The mail_notification structure looks like this:
    
    
    typedef struct
    {
       bool alert;
       bool beep;
    } mail_notification;

`get_mail_notification()` always returns `B_OK`. If the current settings can't be checked (for example, if the user has never configured mail), `alert` will be returned as the default value of `false`, and `beep` will be `true`.

`set_mail_notification()` accepts a pointer to a mail_notification structure and configures the system to report incoming mail using the methods specified therein. If the `save` argument is `true`, the change is set as the new default and will be remembered when the computer is shut down. If `false`, the change is temporary.

Return Code| Description  
---|---  
`B_OK`| The notification was successfully set or retrieved.  
`B_NO_REPLY`| The mail daemon didn't respond to the request.  
  
* * *

###  get_pop_account(), set_pop_account(), mail_pop_account

`status_t get_pop_account(mail_pop_account* account_info,  
int32 index = 0);``status_t set_pop_account(mail_pop_account* account_info,  
int32 index = 0);`

Get and set the specified POP account's information. The mail_pop_account structure is defined as follows:
    
    
    typedef struct
    {
       char pop_name[B_MAX_USER_NAME_LENGTH];
       char pop_password[B_MAX_USER_NAME_LENGTH];
       char pop_host[B_MAX_HOST_NAME_LENGTH];
       char real_name[128];
       char reply_to[128];
       int32 days;
       int32 interval;
       int32 begin_time;
       int32 end_time;
    } mail_pop_account;

The `pop_name`, `pop_password`, and `pop_host` fields in the mail_pop_account structure represent the username, password, and POP server host of the e-mail user. The `real_name` is the user's real name, and `reply_to` is the e-mail address to which replies should be sent.

The `days` field can contain any of the following flags to specify which days of the week the mail daemon should automatically check mail for the described account:

Constant| Description  
---|---  
`B_CHECK_NEVER`| Don't automatically check the account's mail.  
`B_CHECK_WEEKDAYS`| Check the mail only on weekdays.  
`B_CHECK_DAILY`| Check the mail every day.  
`B_CHECK_CONTINUOUSLY`| Check continuously every day.  
  
The `interval` specifies how many seconds apart each e-mail retrieval should be, and the `begin_time` and `end_time` specify the time of day (in seconds) that automatic retrieval should begin and end. If `begin_time` and `end_time` are the same, the daemon checks mail round-the-clock.

Note

![Note](./images/admonitions/Info_32.png)

Eventually these functions will support multiple POP accounts; at this time, the Mail Kit only supports one POP account, so you must use an `index` of 0. Any other `index` will result in a `B_BAD_INDEX` error.

`get_pop_account()` fills the specified mail_pop_account structure with the information on the POP account, and `set_pop_account()` takes the information in the buffer and saves it as the new default.

Return Code| Description  
---|---  
`B_OK`| The notification was successfully set or retrieved.  
`B_BAD_INDEX`| An `index` other than 0 was specified.  
`B_NO_REPLY`| The mail daemon didn't reply to the request.  
  
* * *

###  get_smtp_host(), set_smtp_host()

`status_t get_smtp_host(char* smtp_host);``status_t set_smtp_host(char* smtp_host,  
bool save = true);`

`get_smtp_host()` returns in the buffer pointed to by `smtp_host` the name of the SMTP host as currently configured. The buffer should be at lest `B_MAX_HOST_NAME_LENGTH` bytes long.

`set_smtp_host()` sets the SMTP host through which mail will be sent in the future to the specified host. If `save` is `true`, the new setting becomes the default and will persist through a reboot of the computer; otherwise, the change is only temporary.

Return Code| Description  
---|---  
`B_OK`| The notification was successfully set or retrieved.  
`B_NO_REPLY`| The mail daemon didn't respond to the request.  
  
* * *

### send_queued_mail()

`status_t send_queued_mail();`

Tells the mail daemon to send all pending outgoing mail.

Return Code| Description  
---|---  
`B_OK`| Mail transfer intitiated successfully.  
Other Errors| From [`BMessenger::SendMessage()`](BMessenger.md#BMessenger_SendMessage "SendMessage\(\)")

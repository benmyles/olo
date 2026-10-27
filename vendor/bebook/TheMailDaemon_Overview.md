<!-- System Overview - The Mail Kit - The Mail Daemon -->

## The Mail Daemon  
  
Every computer running the BeOS has a mail daemon; this is a local process that's responsible for sending e-mail to and receiving e-mail from a mail server. The mail server that the daemon talks to is a networking application that's either part of your Internet Service Provider's services, or that's running on a local "mail repository" machine.

The functions described in this section tell you how to manage the mail daemon's connection with the mail server—how to tell the daemon which mail server to communicate with, how to tell the mail daemon to send and retrieve e-mail, how to automate mail retrieval, and so forth.

Many of the functions described here are user-accessible through the E-mail preference application. These functions should generally not be used; the settings they control belong to the user, and your application should usually avoid changing the user's settings. The only legitimate reason to use these configuration setting functions is if you want to build your own E-mail preference application.

The other functions, such as [`forward_mail()`](TheMailDaemon_Functions.md#forward_mail "forward_mail\(\)"), [`check_for_mail()`](TheMailDaemon_Functions.md#check_for_mail "check_for_mail\(\)"), [`encode_base64()`](TheMailDaemon_Functions.md#encode_base64 "encode_base64\(\)"), and [`decode_base64()`](TheMailDaemon_Functions.md#decode_base64 "decode_base64\(\)"), might be legitimately used by your e-mail program.

The architecture of an e-mail message isn't discussed here; for that information, see [`BMailMessage`](BMailMessage.md "BMailMessage").

* * *

### The Mail Daemon and the Mail Server

The mail daemon can talk to two different kinds of mail server:

  1. The Post Office Protocol (POP) server manages individual mail accounts. When the BeOS mail daemon wants to retrieve mail that's been sent to a user, it must tell the mail server which POP accouant it's retrieving mail for.

  2. The Simple Mail Transfer Protocol (SMTP) server manages mail that's being sent out onto the network. Messages sent through an SMTP server will eventually find their way to a POP server to be received by the destination user.

The POP and the SMTP servers are identified by their hosts' names (in other words, the names of the machines on which the servers are running). The mail daemon can only talk to one POP and one SMTP server at a time, but can talk to the two of them simultaneously. Usually—but not always—the POP and SMTP servers reside on the same machine, and so are identified by the same name.

To set the identities of the POP host, you fill in the fields of a mail_pop_account structure and pass the structure to the [`set_pop_account()`](TheMailDaemon_Functions.md#set_pop_account) function. As the name of the structure implies, mail_pop_account encodes more than just the names of the server's host. It also identifies a specific user's POP mail account; the complete definition of the structure is this:
    
    
    typedef struct {
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
  
The `interval` specifies how many seconds apart each e-mail retrieval should be, and the `begin_time` and `end_time` specify the time of day (in seconds) that automatic retrieval should begin and end.

The SMTP server can be selected by calling [`set_smtp_host()`](TheMailDaemon_Functions.md#set_smtp_host), passing in a pointer to the SMTP host's name.

* * *

### Sending and Retrieving Mail

Messages that are retrieved (from the mail server) by the mail daemon are stored as individual files on the user's hard disk, from whence they are plucked and displayed by a mail-reading application (a "mail reader"; Be supplies a simple mail reader called BeMail). Similarly, messages that the user composes (in a mail composition application) and sends are stored as individual files until the mail daemon comes along and passes them on to the mail server.

Sending and retrieving mail is the mail daemon's most important function. Both actions (server-to-database and database-to-server transmission) are performed through the [`check_for_mail()`](TheMailDaemon_Functions.md#check_for_mail "check_for_mail\(\)") function.

The [`BMailMessage`](BMailMessage.md "BMailMessage") class provides a convenient means for creating and sending new mail messages; visit the section on that class for further information and a simple example.

Mail that has been retrieved by the mail daemon can be identified and queried using the mail attributes defined by the Mail Kit. By using the [`BQuery`](BQuery.md "BQuery") class, you can scan all newly-received mail messages and parse the message file to present each message to the user. For a more in-depth discussion of the mail attributes and how to use them to your benefit, read "[Querying Mail Messages](TheMailKit_Overview_Introduction.md#TheMailKit_QueryingMailMessages "Querying Mail Messages")."

* * *

### Other Mail Daemon Features

The other mail structures and functions define the other features that are provided by the mail daemon. These features are:

Mail notification.
    

The mail_notification structure (passed through the [`set_mail_notification()`](TheMailDaemon_Functions.md#set_mail_notification) function) lets you tell the daemon how you would like it to tap you on the shoulder when it has new mail for you to read. Would you like it to display an alert panel? Squawk at you? Both? This can be configured by the user in the E-mail preference application.

Mail forwarding.
    

The [`forward_mail()`](TheMailDaemon_Functions.md#forward_mail "forward_mail\(\)") function lets you ask the Mail Kit to forward a message to one or more other accounts.

Base-64 encoding and decoding.
    

The [`encode_base64()`](TheMailDaemon_Functions.md#encode_base64 "encode_base64\(\)") and [`decode_base64()`](TheMailDaemon_Functions.md#decode_base64 "decode_base64\(\)") functions let you easily handle ASCII-encoded file attachments.

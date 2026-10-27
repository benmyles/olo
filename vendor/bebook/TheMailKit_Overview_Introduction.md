<!-- System Overview - The Mail Kit - Introduction -->

## Introduction

The Mail Kit provides Internet e-mail messaging services. These services include:

  * Configuration of the user's mail accounts.

  * Sending messages using the Simple Mail Transfer Protocol (SMTP).

  * Receiving messages via the Post Office Protocol (POP).

  * Automatic, timed sending and receiving of messages.

  * Encoding and decoding base-64 encoded data.

An assortment of global C functions are provided by the Mail Kit to configure the mail daemon and process base-64 data. See "[The Mail Daemon](TheMailDaemon_Overview.md "The Mail Daemon")" for information on how to use these functions.

Outgoing mail messages are constructed and sent using the [`BMailMessage`](BMailMessage.md "BMailMessage") class.

* * *

### Mail Message Files

Every mail message is stored in an individual file with attached attributes that describe the message in detail; you can query the file system to obtain information about the sender, subject, and receiver of the message, among other things. See "[Querying Mail Messages](TheMailKit_Overview_Introduction.md#TheMailKit_QueryingMailMessages "Querying Mail Messages")" below for more detailed information and an example program.

Every message the user writes is saved in a file until it's sent by the mail daemon (and may or may not be deleted after being sent). Likewise, messages that the mail daemon has retrieved are also stored in files on a local disk.

The process of sending a mail message works something like this:

  * The user writes a new mail message and chooses the mail writing program's "Send" option.

  * The mail writing program creates a [`BMailMessage`](BMailMessage.md "BMailMessage") object and configures it based on the user's inputs by setting the "To," "Subject," and other header fields appropriately, and by storing the message content into the [`BMailMessage`](BMailMessage.md "BMailMessage").

  * The program then calls the [`BMailMessage`](BMailMessage.md "BMailMessage") object's [`Send()`](BMailMessage.md#BMailMessage_Send "Send\(\)") function to tell the mail daemon to send the message.

  * The mail daemon creates a disk file that contains the message. The message content is stored in the file itself, and attributes are created to contain the "To," "Subject," and other relevant header fields. See "[Querying Mail Messages](TheMailKit_Overview_Introduction.md#TheMailKit_QueryingMailMessages "Querying Mail Messages")" for more information. The message's status attribute is set to "New".

  * The next time the mail daemon's [`check_for_mail()`](TheMailDaemon_Functions.md#check_for_mail "check_for_mail\(\)") function is called (either automatically or explicitly), the daemon sends the message via SMTP, then changes the message's status attribute to "Sent". This is done for all mail messages whose status is "Pending".

After sending outgoing messages, the mail daemon will also check to see if any incoming mail is waiting to be retrieved. If there is, it proceeds something like this:

  * The mail daemon fetches the first message from the mail server via POP.

  * The daemon creates a new mail message file, and the message is written into the file. The file contains the Internet headers, message content, and all enclosures (if any).

  * The mail daemon scans the message and adds attributes to the message file for each of the header fields, as well as a couple of extra attributes. These are described in detail in "[Querying Mail Messages](TheMailKit_Overview_Introduction.md#TheMailKit_QueryingMailMessages "Querying Mail Messages")" below.

  * The daemon continues reading mail messages from the mail server until there aren't any left.

* * *

### Querying Mail Messages

The Mail Kit takes full advantage of the BeOS attribute and query system. Each message received is parsed by the mail daemon and important information about it is converted into a defined collection of attributes attached to the file. This makes it extremely easy to create applications that search for messages meeting specific parameters. In an example to follow shortly, we'll create a program that lists all unread messages.

Once the mail daemon has received a message and saved it to disk, any application can query the file system to locate messages that meet certain parameters, then read the attributes and message content to present information about that message to the user.

The following attributes are provided by the Mail Kit:

Constant| Attribute Name| Description  
---|---|---  
`B_MAIL_ATTR_NAME`| MAIL:name| Name of the mail file.  
`B_MAIL_ATTR_STATUS`| MAIL:status|  Is a string that identifies the status of the message. Possible values are: | String| Description  
---|---  
"Error"| An error occurred trying to send the message.  
"New"| The message has not been read yet.  
"Pending"| The message has not been sent yet.  
"Read"| The message has been read.  
"Sent"| The message has been sent.  
  
`B_MAIL_ATTR_PRIOIRITY`| MAIL:priority| "Priority" field value.  
`B_MAIL_ATTR_TO`| MAIL:to| The primary recipient's e-mail address; contained within the "To" header of the message.  
`B_MAIL_ATTR_CC`| MAIL:cc| The addresses to whom the message is carbon copied; contained within the "Cc" header of the message.  
`B_MAIL_ATTR_FROM`| MAIL:from| The sender's e-mail address; contained within the "From" header of the message.  
`B_MAIL_ATTR_SUBJECT`| MAIL:subject| String containing the subject of the message; contained within the "Subject" header of the message.  
`B_MAIL_ATTR_REPLY`| MAIL:reply| The sender's reply-to address; contained within the "Reply-to" header of the message.  
`B_MAIL_ATTR_WHEN`| MAIL:when| BeOS time field (`B_TIME_TYPE`) when the message was sent; contained within the "When" header of the message.  
`B_MAIL_ATTR_FLAGS`| MAIL:flags| 32-bit integer (int32) flags which can be any combination of the following values: | Flag| Description  
---|---  
`B_MAIL_PENDING`| Message is waiting to be sent.  
`B_MAIL_SENT`| Message has been sent.  
`B_MAIL_SAVE`| Mail will be saved after being sent.  
  
`B_MAIL_ATTR_RECIPIENTS`| MAIL:recipients| List of all recipients of the message (primary, cc, and bcc), but is only valid for outgoing messages (this attribute doesn't exist on incoming messages).  
`B_MAIL_ATTR_MIME`| MAIL:mime| A string that defines the version number of the MIME specification used to transmit any enclosures attached to the file. This attribute is only present if the file has one or more enclosures.  
`B_MAIL_ATTR_HEADER`| MAIL:header_length| An int32 value containing the length of the message header.  
`B_MAIL_ATTR_CONTENT`| MAIL:content_length| An int32 value containing the length of the message content.  
  
The following attributes are indexed:

  * `B_MAIL_ATTR_NAME`
  * `B_MAIL_ATTR_STATUS`
  * `B_MAIL_ATTR_PRIORITY`
  * `B_MAIL_ATTR_TO`
  * `B_MAIL_ATTR_CC`
  * `B_MAIL_ATTR_FROM`
  * `B_MAIL_ATTR_SUBJECT`
  * `B_MAIL_ATTR_REPLY`
  * `B_MAIL_ATTR_WHEN`
  * `B_MAIL_ATTR_FLAGS`

The headers, the contents of the messag,e and the enclosures (in base-64 encoded form) can all be found in the file itself and can be read using a [`BFile`](BFile.md "BFile") object. See "[The Storage Kit](TheStorageKit.md "The Storage Kit")" in the for further information on reading files.

#### Queries and Mail Messages

Now that you know what attributes are available on mail message files, and which attributes are indexed, you can consider all the clever things you can use them for. Let's look at an example program that, from a Terminal window, lets you see a list of the unread mail you have.
    
    
    void main(void)
    {
       BQuery query;
       BNode node;
       BVolume vol;
       BVolumeRoster vroster;
       entry_ref ref;
       char buf[256];
       int32 message_count = 0;
    
       vroster.GetBootVolume(&vol);
       query.SetVolume(&vol);

The program begins by establishing needed variables, then using a [`BVolumeRoster`](BVolumeRoster.md "BVolumeRoster") to set the query's search volume to the boot disk. This is covered in more detail in the Storage Kit chapter.
    
    
       if (query.SetPredicate("MAIL:status = New") != B_OK) {
          printf("Error: can't set query predicate.n");
          return;
       }

Then the query is configured to search for new mail. New messages can be identified by the `B_MAIL_ATTR_STATUS` attribute (called "MAIL:status") having a string value of "New". If an error occurs, the program prints an error end returns.
    
    
       if (query.Fetch() != B_OK) {
          printf("Error: new mail query failed.n");
          return;
       }

The query is told to fetch. Again, if this fails, an error message is displayed and the program returns.
    
    
       while (query.GetNextRef(&ref) == B_OK) {
          message_count++;      // Increment message counter

The loop scanning through the fetched new messages begins by incrementing the counter of new messages received.
    
    
          if (node.SetTo(&ref) != B_OK) {
             printf("Error: error scanning new messages.n");
             return;
          }

Then a [`BNode`](BNode.md "BNode") is set to reference the message file. If this fails, the program displays an error message and quits.
    
    
          buf[0] = '0';      // If error, use empty string
          node.ReadAttr(B_MAIL_ATTR_FROM, B_STRING_TYPE, 0,
                        buf, 255);
          buf[20] = '0';      // Truncate to 20 characters
          printf("%3d From: %-20s", message_count, buf);

The buffer we're using the receive the attribute values is initialized to an empty string, then we call [`BNode`](BNode.md "BNode")'s [`ReadAttr()`](BNode.md#BNode_ReadAttr "ReadAttr\(\), WriteAttr\(\), RemoveAttr\(\)") function to read the `B_MAIL_ATTR_FROM` attribute into the buffer. We then truncate the read string to 20 characters for display purposes (to make it fit into the table we're outputting) and print the message number and sender information.
    
    
          buf[0] = '0';      // If error, use empty string
          node.ReadAttr(B_MAIL_ATTR_SUBJECT, B_STRING_TYPE, 0, buf, 255);
          buf[40] = '0';      // Truncate to 40 characters
          printf(" Sub: %sn", buf);
       }

The buffer is reset to an empty string and the `B_MAIL_ATTR_SUBJECT` attribute is read into it. This string is truncated to 40 characters, then printed.

This loop continues until no more new messages are found; this is detected when [`GetNextRef()`](BQuery.md#BQuery_GetNextRef) returns an error.
    
    
       if (message_count) {
          printf("%d new messages.n", message_count);
       }
       else {
          printf("No new messages.n");
       }
    }

Finally, the number of new messages is printed. If there aren't any messages, we very politely print "No new messages." rather than "0 new messages," for a little added panache.

This simple example demonstrates how you can use the attributes provided by the Mail Kit to create mail message reading applications. The message body and attachments are stored within the file itself, and can be read using the functions described in the [`BFile`](BFile.md "BFile") section in [The Storage Kit](TheStorageKit.md "The Storage Kit") chapter

* * *

### The E-mail File Type

The mail daemon ensures, when it's first launched at system boot time, that the BeOS File Type database includes an entry for e-mail files. E-mail files have the MIME string "text/x-email", which is represented by the constant `B_MAIL_TYPE`.

The E-mail entry in the File Type database includes a list of the attributes on which you can search. You can look up this information using the [`BMimeType`](BMimeType.md "BMimeType") class's [`GetAttrInfo()`](BMimeType.md#BMimeType_GetAttrInfo "GetAttrInfo\(\), SetAttrInfo\(\)") function:
    
    
    BMimeType mime;
    BMessage message;
    
    mime.SetTo(B_MAIL_TYPE);
    mime.GetAttrInfo(&message);

After running this code, the message contains the description of the attributes available on e-mail files. The message has three useful arrays of items:

Message Item| Description  
---|---  
`attr:public_name`| The user-readable name of the attribute.  
`attr:name`| The attribute name used for [`BNode`](BNode.md "BNode") and `fs_attr_*` calls.  
`attr:type`| The type of data the attribute contains.  
  
You can examine the items in each of these arrays in the message to get useful information about the attributes you can reference on the e-mail files. For example:
    
    
    printf("Public name: %sn", FindString("attr:public_name", 1));
    printf("Name: %sn", FindString("attr:name", 1));

This code will print the public name and the attribute name of the second attribute registered in the File Type database entry for e-mail files. In the current implementation of the Mail Kit, this would print:
    
    
    Public name: Subject
    Name: MAIL:subject

The number of (and order of) attributes in the database entry might change in the future—and that's where the File Type database comes in handy. If, a year from now, Be adds more attributes to e-mail files, your File Type database-savvy application won't have to be updated to support them.

At this time, the File Type database has attribute information records for each of the following attributes:

  * `B_MAIL_ATTR_NAME`
  * `B_MAIL_ATTR_STATUS`
  * `B_MAIL_ATTR_PRIORITY`
  * `B_MAIL_ATTR_TO`
  * `B_MAIL_ATTR_FROM`
  * `B_MAIL_ATTR_SUBJECT`
  * `B_MAIL_ATTR_REPLY`
  * `B_MAIL_ATTR_WHEN`

You can obtain information on these attributes, their formats, and their user-readable names by looping through the arrays in the message until the `BMessage:Find…()` function returns `NULL` or `B_BAD_INDEX`.

For more information on the `BMimeType` class and the File Type database, see [`BMimeType`](BMimeType.md "BMimeType") in [The Storage Kit](TheStorageKit.md "The Storage Kit").

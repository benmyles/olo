<!-- System Overview - The Mail Kit - BMailMessage -->

## BMailMessage  
  
The [`BMailMessage`](BMailMessage.md "BMailMessage") class provides an easy way to send e-mail messages. If you want to do it the hard way, look up the SMTP RFC and start plodding your way through the Network Kit documentation. You'll get it working one of these days.

Or you can sail right on through and be sending e-mail from your own applications in a matter of minutes using your friend, the [`BMailMessage`](BMailMessage.md "BMailMessage").

* * *

### Constructing a Mail Message

To send an e-mail, you simply construct a new [`BMailMessage`](BMailMessage.md "BMailMessage") object, add a "To" header field, add the content, and send the message on its way. For example:
    
    
    BMailMessage *mail;
    char *message;
    
    mail = new BMailMessage();
    mail->AddHeaderField(B_MAIL_TO, "bob@uncle.com");
    mail->AddHeaderField(B_MAIL_SUBJECT, "Hi");
    message = "Hi, Uncle Bob!";
    mail->AddContent(message, strlen(message));

This is a pretty basic message. The subject is "Hi," the message is sent to "bob@uncle.com," and the message body is "Hi, Uncle Bob!"

You can add other fields, including carbon-copy (CC) and blind-carbon-copy (BCC) fields, and you can add attachments. For example, if you want to also attach a file called `/boot/home/file.zip`, you can do the following:
    
    
    mail->AddEnclosure("/boot/home/file.zip");

Once your message has been constructed, you can send it by calling [`Send()`](BMailMessage.md#BMailMessage_Send "Send\(\)"):
    
    
    mail->Send();

That's the basic technique behind sending e-mail under the BeOS. The mail daemon also fetches incoming mail from a POP server, but you can't use the [`BMailMessage`](BMailMessage.md "BMailMessage") class to read these messages; you use the BeOS [`BQuery`](BQuery.md "BQuery") and [`BNode`](BNode.md "BNode") classes to locate messages of interest and obtain information about them. See "Querying Mail Messages" for more information.

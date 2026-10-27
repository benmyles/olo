<!-- System Overview - The Storage Kit - Mime And File Types -->

## Mime And File Types

MIME (Multipurpose Internet Mail Extensions) is a way to describe the content or intent of a parcel of data. As the name implies, MIME was invented to make e-mail smarter: Want to send a GIF image through e-mail? If your mail program understands MIME, it can encode the image, tag it with a MIME string (in this case, "image/gif"), as well as some other header information (such as the encoding protocol) before sending the message out across the wires. Any MIME-savvy recipient program will be able to properly display the message based (primarily) on the content-type string.

* * *

### MIME in the BeOS

The Be file system adds a MIME string to every file in the (native) file system, and tries to figure out an appropriate MIME type for foreign files, as well. MIME types are used by the Tracker, for example, to figure out what icon it should use to represent a particular file. Another example: When the user double-clicks a file, the MIME type (for that file) is used to identify the application that will open the file. Your application can perform similar deductions: When the user drag-drops a file, your application can look at the file's MIME type and proceed (or reject) accordingly.

There are two parts to Be's MIME support:

File Types.
    

The first part is the inclusion of a MIME string as part of every file, as mentioned above. This is called the file's file type. The MIME string is added as an attribute—it's not part of the file's data. Every file—whether it's a document, an application, or even a directory has its own file type attribute. For more on getting and setting a file's type, see the [`BNodeInfo`](BNodeInfo.md "BNodeInfo") class.

The File Type Database.
    

The second part is the system's File Type database. The database contains information about the file types that the system recognizes. The user can get to the database through the FileTypes preferences application. Programmatically, you access it through the [`BMimeType`](BMimeType.md "BMimeType") and [`BAppFileInfo`](BAppFileInfo.md "BAppFileInfo") classes.

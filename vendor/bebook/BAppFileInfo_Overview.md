<!-- System Overview - The Storage Kit - BAppFileInfo -->

## BAppFileInfo

[`BAppFileInfo`](BAppFileInfo.md "BAppFileInfo") lets you get and set the signature, supported types, icons, app flags, and version info that's stored in an executable file's attributes and/or resources. The object also knows how to write certain particles of information into the File Type database and, if the executable is the progenitor of a running application, into the app roster ([`be_roster`](BRoster.md#BRoster_be_roster "be_roster")).

Warning

![Warning](./images/admonitions/Stop_32.png)

Most apps won't ever need to create or use a [`BAppFileInfo`](BAppFileInfo.md "BAppFileInfo") object. Setting an executable's info is best left to the file's creator, through the use of resource data that's compiled into the executable. Even the act of looking at a file's info should be rare for a normal application.

Warning

![Warning](./images/admonitions/Stop_32.png)

[`BAppFileInfo`](BAppFileInfo.md "BAppFileInfo") objects should only be used to examine and set the characteristics of applications and add-ons. Using the object on a non-executable file could corrupt the file.

* * *

### Initialization

You initialize a [`BAppFileInfo`](BAppFileInfo.md "BAppFileInfo") object by passing it a [`BFile`](BFile.md "BFile") object that represents an application or add-on. The [`BFile`](BFile.md "BFile") needn't be open for reading, but it must be open for writing if you want to set information. The [`BAppFileInfo`](BAppFileInfo.md "BAppFileInfo") object doesn't take over ownership of the [`BFile`](BFile.md "BFile") that you pass it; in particular, deleting a [`BAppFileInfo`](BAppFileInfo.md "BAppFileInfo") doesn't cause the underlying [`BFile`](BFile.md "BFile") to be deleted.

To initialize a [`BAppFileInfo`](BAppFileInfo.md "BAppFileInfo") to point to the executable of [`be_app`](BApplication.md#BApplication_be_app "be_app"), you do this:
    
    
    /* To get app file info for be_app. */
    [app_info](BRoster.md#app_info "app_info") appInfo;
    BFile file;
    [BAppFileInfo](BAppFileInfo.md "BAppFileInfo") appFileInfo;
    
    be_app->GetAppInfo(&appInfo);
    file.SetTo(&appInfo.ref, B_READ_WRITE);
    appFileInfo.SetTo(&file);

* * *

### Attributes, Resources, and the File Type Database

When you ask a [`BAppFileInfo`](BAppFileInfo.md "BAppFileInfo") object to get some information, it looks in its [`BFile`](BFile.md "BFile")'s attributes; if the information isn't there, it then looks in the file's resources. When you ask it to set some information, the info is written as an attribute and also stored as a resource. You can modify this behavior through [`SetInfoLocation()`](BAppFileInfo.md#BAppFileInfo_SetInfoLocation "SetInfoLocation\(\), IsUsingAttributes\(\), IsUsingResources\(\)"): You can tell the object to only access the file's attributes, or to only access the resources.

The signature, icons, and supported types that you set through the functions provided here ( [`SetSignature()`](BAppFileInfo.md#BAppFileInfo_SetSignature), [`SetIcon()`](BAppFileInfo.md#BAppFileInfo_SetIcon), [`SetIconForType()`](BAppFileInfo.md#BAppFileInfo_SetIconForType), [`SetSupportedTypes()`](BAppFileInfo.md#BAppFileInfo_SetSupportedTypes)) are also recorded in the File Types database, as described in the various functions.

* * *

### Functions Inherited From BNodeInfo

You should take care when using the following functions (inherited from [`BNodeInfo`](BNodeInfo.md "BNodeInfo")):

[`SetPreferredApp()`](BNodeInfo.md#BNodeInfo_SetPreferredApp)
    

Never set an application's preferred app; an application is automatically set to be its own preferred app&mdash;it won't work otherwise. An add-on's preferred app is usually itself, but it doesn't have to be. For example, you could set an add-on's preferred app to be the server or application that loads the add-on.

[`SetType()`](BNodeInfo.md#BNodeInfo_SetType)
    

Never set the type of an application or add-on. The type is automatically set to be `B_APP_MIME_TYPE` (a platform-dependent value). If you change the type, your application or add-on will still run (probably), but other parts of the system (double-clicked documents, for example) may have a hard time finding it.

* * *

### Errors

When you ask a [`BAppFileInfo`](BAppFileInfo.md "BAppFileInfo") to retrieve information by reference, the object doesn't clear the reference argument if it fails. Because of this, you should always check the error code that's returned by the `Get…()` functions.

The common errors that `BAppFileInfo` functions return are these:

Constant| Description  
---|---  
`B_OK`| Success.  
`B_NO_INIT`| The [`BAppFileInfo`](BAppFileInfo.md "BAppFileInfo") is uninitialized, or its [`BFile`](BFile.md "BFile") isn't open for writing.  
`B_ERROR`| The [`BFile`](BFile.md "BFile") was locked when you initialized the [`BAppFileInfo`](BAppFileInfo.md "BAppFileInfo").  
  
The info-reading and -writing functions may also return the error codes declared by [`BNode::ReadAttr()`](BNode.md#BNode_ReadAttr "ReadAttr\(\), WriteAttr\(\), RemoveAttr\(\)") and [`BNode::WriteAttr()`](BNode.md#BNode_WriteAttr) , and by [`BResources::WriteTo()`](BResources.md#BResources_WriteTo "WriteTo\(\)").

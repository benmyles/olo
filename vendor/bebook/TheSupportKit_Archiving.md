<!-- System Overview - The Support Kit - Archiving -->

## Archiving

The BeOS provides a protocol for archiving and unarchiving objects. When you archive an object, you record its state into a [`BMessage`](BMessage.md "BMessage") that can be sent to another application, flattened and saved as a file, cached in memory, and so on. Unarchiving does the opposite: It takes a [`BMessage`](BMessage.md "BMessage") archive and turns it into a functioning object.

The archiving protocol is represented by the [`BArchivable`](BArchivable.md "BArchivable") class. Many of the BeOS classes inherit from [`BArchivable`](BArchivable.md "BArchivable"). If you create your own classes and want them to be archivable, they, too, must inherit (directly or indirectly) from [`BArchivable`](BArchivable.md "BArchivable").

The sections below tell you (1) how to archive and unarchive an object, and (2) what you have to do to create an archivable class of your own.

* * *

### Archiving and Unarchiving

To archive a [`BArchivable`](BArchivable.md "BArchivable") object, you create a [`BMessage`](BMessage.md "BMessage") and pass it to the object's [`Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)") function. You can then send the message to some other application or, as shown below, write out its flattened form to a file so you can resurrect it later:
    
    
    /* Archiving and storing a BButton, sans error checks.*/
    BMessage message;
    BButton *button;
    BFile file;
    
    button->Archive(&message);
    file.SetTo(filename, B_CREATE_FILE | B_WRITE_ONLY);
    message.Flatten(&file);

To unarchive, you find an archived object and pass it to the object's constructor, or, if you don't know the object's class (we'll talk more about that later), you pass it to [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"). The constructor case is simple:
    
    
    BMessage msg;
    BFile f;
    BButton *button;
    
    file.SetTo(filename, B_READ_ONLY);
    msg.Unflatten(&f);
    button = new BButton(&msg);

Invoking the constructor is fine if you know the class of the object that you're unarchiving. But consider the case where you're unarchiving an object from a message that you didn't create. For example, let's say your application displays a view that's imported from a foreign archive (sent from another application, or loaded from an add-on). To unarchive the "unknown" object, you pass the [`BMessage`](BMessage.md "BMessage") archive to [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)"), and then cast the returned object (a [`BArchivable`](BMessage.md "BMessage")* ) to some:
    
    
    BView *foreignView;
    foreignView = dynamic_cast<BView *>(instantiate_object(&msg));

You still don't know the class of the object, and you won't be able to invoke any functions that it defines, but at least

Each archivable class implements [`Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)") to record the object properties (data members, "owned" objects, etc.) that the class defines. These properties are added as fields to the argument [`BMessage`](BMessage.md "BMessage"). Most implementations look something like this:
    
    
    status_t MyClass::Archive(BMessage *archive, bool deep)
    {
       _baseClass_ ::Archive(archive, deep);![1](./images/callouts/1.png)
    
       archive->AddInt32("MyClass::Property1", property1);![2](./images/callouts/2.png)
       archive->AddString("MyClass::Property2", property2);
       ...
    
       if ( deep![3](./images/callouts/3.png)) {
          BMessage childArchive;
          for (int32 i; i < CountChildren(); i++)
             if ( childAt(i)->Archive(&childArchive, deep) == B_OK )
                archive->AddMessage("children", &childArchive);
       }
    
       message->AddString("add_on", "application/x-CodeForThisObject");![4](./images/callouts/4.png)
    }

![1](./images/callouts/1.png) | First, call the inherited version of [`Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)"). This gives all the ancestor classes a shot at recording the properties they define.
    
    
    BArchivable::Archive(archive, deep);  
  
---|---  
![2](./images/callouts/2.png) | Next, record the properties that are defined by this class. How the message fields are named and typed are up to the class itself. It's a good idea to give your fields some sort of prefix to keep them from colliding with the Be-defined fields (whose names are, unfortunately, a bit haphazard).  
![3](./images/callouts/3.png) | [`Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)")'s second argument indicates whether the archive should be deep or shallow. In a deep archive, a class archives any objects that it "owns"—roughly speaking, these are the objects that the class is responsible for deleting. The archivable children must also inherit from [`BArchivable`](BArchivable.md "BArchivable").  
![4](./images/callouts/4.png) | This line records the signature of the library that contains the object's code. If you want your object to be loaded dynamically by some other application, you should add the library signature under the string field `add_on`. You don't have to supply a library if you don't care about dynamic loading.  
  
If a class doesn't have any data to add to the archive, it doesn't need to implement [`Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)").

* * *

### Instantiability

To be unarchivable, a class must implement a constructor that takes a [`BMessage`](BMessage.md "BMessage") archive as an argument, and it must implement the static [`Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)") function.

A typical unarchiving constructor calls the inherited version, and then reconstitutes the properties that were added when the object was archived:
    
    
    MyClass::MyClass(BMessage *archive) : _baseClass_(archive)
    {
       int32    i;
       BMessage msg;
       BArchivable* obj;
    
       archive->FindInt32("MyClass::Property1", &property1);
       archive->FindString("MyClass::Property2", &property2);
       ...
    
       while ( data->FindMessage("children", i++, &msg) == B_OK){
          obj = instantiate_object(&msg);
          childList->AddItem(dynamic_cast<ChildClass *>(obj));
       }
    }

The class must also implement the static [`Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)") function (declared in [`Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)")), which needn't do much more than call the archive-accepting constructor, and return a [`Archive()`](BArchivable.md#BArchivable_Archive "Archive\(\)") pointer. For example:
    
    
    BArchivable* TheClass::Instantiate(BMessage* archive)
    {
       if ( validate_instantiation(archive, "TheClass"))
          return new TheClass(archive);
       return NULL;
    }

The [`validate_instantiation()`](TheSupportKit_Functions.md#validate_instantiation "validate_instantiation\(\)") function, provided by the Support Kit, is a safety check that makes sure the [`BMessage`](BMessage.md "BMessage") object is, in fact, an archive for the named class.

* * *

### Unarchiving

To unarchive a [`BMessage`](BMessage.md "BMessage"), you call the [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)") function. When passed a [`BMessage`](BMessage.md "BMessage") archive, [`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)") looks for the first name in the "class" array, finds the [`Instantiate()`](BArchivable.md#BArchivable_Instantiate "Instantiate\(\)") function for that class, and calls it. Failing that, it picks another name from the "class" array (working up the inheritance hierarchy) and tries again.

[`instantiate_object()`](TheSupportKit_Functions.md#instantiate_object "instantiate_object\(\)") returns a [`BArchivable`](BArchivable.md "BArchivable") instance. You then use `cast_as()` to cast the object to a more interesting class. A typical unarchiving session looks something like this:
    
    
    /* archive is the BMessage that we want to turn into an object.
    * In this case, we want to turn it into a BView.
    */
    BArchivable *unarchived = instantiate_object(archive);
    if ( unarchived ) {
       BView *view = cast_as(unarchived, BView);
       if ( view ) {
          . . .
       }
    }

* * *

### Dynamic Loading

It's not defined how a host will interact with an unarchived instance of a previously unknown class. It's up to the parties to define entry points and protocols, just as it is for any other add-on module.

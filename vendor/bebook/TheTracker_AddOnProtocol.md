<!-- Special Topics - The Tracker - Add-on Protocol -->

## Add-on Protocol

The Tracker provides a convenient shortcut mechanism through the use of add-ons. A user can access a special add-ons menu by right clicking in the Tracker. The Tracker interacts with an add-on through the `process_refs()` function described below.

Tracker add-ons should be placed in `/boot/home/config/add-ons/Tracker`. A shortcut key can be associated with the add-on by appending a dash followed by the shortcut key to the filename of the add-on.

* * *

### process_refs()

Declared in: `add-ons/tracker/TrackerAddOn.h`

`void process_refs(entry_ref dir_ref,  
[`BMessage`](BMessage.md "BMessage")* msg,  
void* reserved);`

The Tracker calls this function when the user invokes the add-on. The current directory is found in `dir_ref`.`msg` is a standard `B_REFS_RECEIVED` `BMessage` with the `refs` array containing the entry_refs of the files selected by the user. The third argument is currently unused.

`process_refs()` runs in a separate thread within the Tracker's team, so if your add-on crashes, the Tracker goes too.

A simple Tracker Add-On follows. It simply takes the contents of the arguments to `process_refs()` and outputs them in a window.
    
    
    #include <Application.h>
    #include <InterfaceKit.h>
    #include <StorageKit.h>
    
    #include <stdio.h>
    #include <string.h>
    
    #include <be/add-ons/tracker/TrackerAddon.h>
    
    void process_refs(entry_ref dir_ref, BMessage *msg, void *)
    {
        BWindow *window = new BWindow(BRect(100,100,300,300),
           "Sample Tracker Add-on", B_TITLED_WINDOW, 0);
        BTextView *view = new BTextView(BRect(0,0,200,200), "view",
           BRect(0,0,200,200), B_FOLLOW_ALL_SIDES, B_WILL_DRAW |
           B_FULL_UPDATE_ON_RESIZE);
    
        BPath path;
        BEntry entry(&dir_ref);
        entry.GetPath(&path);
        view->Insert("Current Directory: ");
        view->Insert(path.Path());
        view->Insert("n");
    
        int refs;
        entry_ref file_ref;
        for (refs=0;
             msg->FindRef("refs", refs, &file_ref) == B_NO_ERROR;
             refs++) {
            if (refs == 0)
                  view->Insert("Selected files:n");
            entry.SetTo(&file_ref);
            entry.GetPath(&path);
            view->Insert(path.Path());
            view->Insert("n");
        }
    
        if (refs == 0)
            view->Insert("No files selected.n");
    
        view->MakeEditable(false);
        window->AddChild(view);
        window->Show();
    }
    
    main()
    {
        new BApplication("application/x-sample-tracker-add-on");
    
        (new BAlert("", "Sample Tracker Add-on", "swell"))->Go();
    
        delete be_app;
    }

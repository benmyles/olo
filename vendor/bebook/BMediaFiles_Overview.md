<!-- System Overview - The Media Kit - BMediaFiles -->

## BMediaFiles  
  
One feature provided by the Media Kit is the ability to assign media files (sounds and graphics files, for example) to named elements, which can then be used to locate user-configured sound and graphics options.

That's the technical way of saying that the Media Kit provides the ability to assign sounds and bitmap graphics to events and system attributes, so you can configure the appearance and behavior of your BeOS computer's user interface.

* * *

### Identifying an Entry

Each entry in the media files registry consists of three elements: a type, an item name, and an [entry_ref](BEntry.md#entry_ref "entry_ref"). The type is the type of media data the entry represents. For example, this could be "sound" or "bitmap." The item name is the actual name of the entry in the registry, such as "Startup" or "desktop image." The [entry_ref](BEntry.md#entry_ref "entry_ref") identifies the file that's been assigned to that particular entry.

An application can instantiate a [`BMediaFiles`](BMediaFiles.md "BMediaFiles") object and then use the [`GetRefFor()`](BMediaFiles.md#BMediaFiles_GetRefFor "GetRefFor\(\), SetRefFor\(\), RemoveRefFor\(\)") function to find out what file is assigned to a particular registry entry. For instance, if your application needs to access the desktop image file, you can get this information as follows:
    
    
    entry_ref ref;
    
    if (GetRefFor("bitmap", "desktop image", &ref) == B_OK) {
       /* have your way with the desktop image file */
    }

The user uses the Sounds preference application to assign sound files to events, such as the system beep and the startup sound. These are named "Beep" and "Startup" respectively. The [`beep()`](TheSupportKit_Functions.md#beep "beep\(\), system_beep\(\), add_system_beep_event\(\)") function will always play whatever sound is assigned to the Beep event.

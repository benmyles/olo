<!-- System Overview - The Application Kit - BPropertyInfo -->

## BPropertyInfo  
  
[`BPropertyInfo`](BPropertyInfo.md "BPropertyInfo") is a simple class that manages scripting. A program describes its scripting interface to a [`BPropertyInfo`](BPropertyInfo.md "BPropertyInfo") object through an array of [property_info](BPropertyInfo.md#property_info "property_info") structures, with each entry describing a piece of the scripting suite.

A [`BPropertyInfo`](BPropertyInfo.md "BPropertyInfo") is instantiated by passing a zero-terminated array of [property_info](BPropertyInfo.md#property_info "property_info") to its constructor. A typical initialization looks like:
    
    
    static property_info prop_list[] = {
        { "duck",
          { B_GET_PROPERTY, B_SET_PROPERTY, 0 },
          { B_DIRECT_SPECIFIER, B_INDEX_SPECIFIER, 0 },
          "get or set duck"
        },
        { "head",
          { B_GET_PROPERTY, 0 },
          { B_DIRECT_SPECIFIER, 0 },
          "get head"
        },
        { "head",
          { B_SET_PROPERTY, 0 },
          { B_DIRECT_SPECIFIER, 0 },
          "set head"
        },
        { "feet",
          {0}, {0}, "can do anything with his orange feet"
        },
        0 // terminate list};
    
    BPropertyInfo prop_info(prop_list);
    

Since [`BPropertyInfo`](BPropertyInfo.md "BPropertyInfo") only stores a pointer to the array, it is important that the life span of the array is at least as long as that of the [`BPropertyInfo`](BPropertyInfo.md "BPropertyInfo") object.

Notice that [`BPropertyInfo`](BPropertyInfo.md "BPropertyInfo") doesn't impose any particular structure upon the array; in particular, not all commands and specifiers for a given property need be placed in a single entry in the array. You are free to organize your scripting suite in whatever manner is most convenient for your particular object.

[`BPropertyInfo`](BPropertyInfo.md "BPropertyInfo") is a descendant of [`BFlattenable`](BFlattenable.md "BFlattenable"), and can therefore be used to store a description of an object's supported scripting suite. This is particularly useful when overriding [`GetSupportedSuites()`](BApplication.md#BApplication_GetSupportedSuites "GetSupportedSuites\(\)"):
    
    
    status_t MyHandler::GetSupportedSuites(BMessage *msg)
    {
        msg->AddString("suites", "suite/vnd.Me-my_handler");
        BPropertyInfo prop_info(prop_list);
        msg->AddFlat("messages", &prop_info);
        return _baseClass_ ::GetSupportedSuites(msg);
    }

Naturally, [`BPropertyInfo`](BPropertyInfo.md "BPropertyInfo") is equally as useful in interpreting the results obtained from querying an object for its supported suites.

[`BPropertyInfo`](BPropertyInfo.md "BPropertyInfo") defines the [`FindMatch()`](BPropertyInfo.md#BPropertyInfo_FindMatch "FindMatch\(\)") method designed to simplify the implementation of [`ResolveSpecifier()`](BHandler.md#BHandler_ResolveSpecifier "ResolveSpecifier\(\)"). It returns the index of the property info matching the description given to it, or -1 if none match. This reduces [`ResolveSpecifier()`](BHandler.md#BHandler_ResolveSpecifier "ResolveSpecifier\(\)") in the simplest cases to:
    
    
    BHandler*
    MyHandler::ResolveSpecifier(BMessage* msg, int32 index,
                                BMessage* spec, int32 form,
                                const char* prop)
    {
        BPropertyInfo prop_info(prop_list);
        if (prop_info.FindMatch(msg, index, spec, form, prop) >= 0)
            return this;
    
        return _baseClass_ ::ResolveSpecifier(msg, index, spec, form,
                                           prop);
    }

Of course, for more complicated objects, [`ResolveSpecifier()`](BHandler.md#BHandler_ResolveSpecifier "ResolveSpecifier\(\)") may need to set the target handler to an object other than itself, so more processing may be required. In those cases, the object can use the index returned by [`FindMatch()`](BPropertyInfo.md#BPropertyInfo_FindMatch "FindMatch\(\)") to help it determine the target of the scripting message.

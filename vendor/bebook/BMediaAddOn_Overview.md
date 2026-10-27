<!-- System Overview - The Media Kit - BMediaAddOn -->

## BMediaAddOn  
  
A [`BMediaAddOn`](BMediaAddOn.md "BMediaAddOn")-derived object describes an executable program that lives in a disk file and is loaded by the Media Server when it's needed. A [`BMediaAddOn`](BMediaAddOn.md "BMediaAddOn") tells the Media Server what kinds of nodes it can create and handles the actual creation of those nodes when called upon by the Media Server to do so.

Note

![Note](./images/admonitions/Info_32.png)

It's important to note that the functions in the [`BMediaAddOn`](BMediaAddOn.md "BMediaAddOn") class will typically only be called by the Media Kit (and from within the add-on itself). These functions aren't called by client applications.

* * *

### Getting to Node You…

A given node can support as many media kinds and formats as it wants (although if you support too many widely disparate media types, your add-on may get difficult to maintain, but that's another issue entirely). For example, a node that supports video might support inputting AVI, QuickTime, and MPEG-2, but might only be able to output AVI. This is information that the Media Kit needs to know. For this reason, the [`BMediaAddOn`](BMediaAddOn.md "BMediaAddOn") needs to provide information about the media flavors it supports.

This is done using the [flavor_info](BMediaAddOn.md#flavor_info "flavor_info") structure:
    
    
    struct flavor_info {
       char *name;
       char *info;
       uint64 kinds;
       uint32 flavor_flags;
       int32 internal_id;
       int32 possible_count;
    
       int32 in_format_count;
       uint32 in_format_flags;
       const media_format *in_formats;
    
       int32 out_format_count;
       uint32 out_format_flags;
       const media_format *out_formats;
    
       uint32 _reserved_[16];
    
    private:
       flavor_info & operator=(const flavor_info &other);
    };

Field| Description  
---|---  
`name` and `info`| These fields provide a human-readable name, and information about the flavor.  
`kinds`| Should indicate all the relevant kinds that the node matches; this is a bit field, and it's possible that more than one flag may be relevant. See [node_kind](TheMediaKit_Constants.md#Enums_node_kind "node_kind").  
`flavor_flags`| Contains flags providing additional information about the flavor.

`B_FLAVOR_IS_GLOBAL`
    The flavor will be forced into the Media Add-on Server, and only one instance of it will exist.
`B_FLAVOR_IS_LOCAL`
    The flavor will be forced into the loading application, and many instances of it may exist.
If neither flag is specified, the Media Kit will decide what to do with the flavor.  
`internal_id`| Is an internal ID number that your add-on can use to identify the flavor; the flavor will be requested by the Media Kit using this ID number.  
`possible_count`| Specifies to the Media Kit the maximum number of instances of your node can be in existence at the same time. For example, if your node provides support for a particular sound card, this value should be equal to the number of cards you support that are currently installed in the computer.  
`in_format_count`| Specifies how many input formats the flavor supports  
`in_formats`| Is a list of all the input formats supported by the flavor.  
`in_format_flags`| Provides informational flags about the flavor's inputs. There aren't any defined values for this field yet; be sure to set it to 0.  
`out_format_count`| Specifies how many output formats the flavor supports.  
`out_formats`| Is a list of all the output formats supported by the flavor.  
`out_format_flags`| Provides informational flags about the flavor's outputs. There aren't any defined values for this field yet; be sure to set it to 0.  
  
If your node is a physical input, such as a sound card, your node's kinds field should include `B_PHYSICAL_INPUT` among the flags set therein. Likewise, if your node is a physical output, or a system mixer, you should include `B_PHYSICAL_OUTPUT` or `B_SYSTEM_MIXER`.

Your node's constructor should also call [`AddNodeKind()`](BMediaNode.md#BMediaNode_AddNodeKind "AddNodeKind\(\)") to add these kind flags; the base classes only add `B_BUFFER_CONSUMER`, `B_BUFFER_PRODUCER`, and so forth; the flags indicating that the node represents a physical input, physical output, or system mixer aren't added automatically. For example, a sound digitizer node's constructor might have the following form:
    
    
    MyBufferProducer::MyBufferProducer(const char *name) :
             BMediaNode(name),
             BBufferProducer() {
    
       AddNodeKind(B_PHYSICAL_INPUT);
    
       /* constructor stuff goes here */
    }

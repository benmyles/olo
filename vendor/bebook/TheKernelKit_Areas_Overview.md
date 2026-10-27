<!-- System Overview - The Kernel Kit - Areas -->

## Areas  
  
An area is a chunk of virtual memory. As such, it has all the expected properties of virtual memory: It has a starting address, a size, addresses it comprises are contiguous, and it maps to (possibly non-contiguous) physical memory. The features that an area provides that you don't get with "standard" memory are these:

Areas can be shared.
    

Different areas can refer to the same physical memory. Put another way, different virtual memory addresses can map to the same physical locations. Furthermore, the different areas needn't belong to the same application. By creating and "cloning" areas, applications can easily share the same data.

Areas can be locked into RAM.
    

You can specify that the area's physical memory be locked into RAM when it's created, locked on a page-by-page basis as pages are swapped in, or that it be swapped in and out as needed.

Areas can be read- and write-protected.

Areas are page-aligned.
    

Areas always start on a page boundary, and are allocated in integer multiples of the size of a page. (A page is 4096 bytes, as represented by the `B_PAGE_SIZE` constant.)

You can specify the starting address of the area's virtual memory.
    

The specification can require that the area start precisely at a certain address, anywhere above a certain address, or anywhere at all.

Because areas are large—one page, minimum—you don't create them arbitrarily. The two most compelling reasons to create an area are the two first points listed above: To share data among different applications, and to lock memory into RAM.

In all particulars (but one) you treat the memory that an area gives you exactly as you would treat any allocated memory: You can read and write it through pointer manipulation, or through standard functions such as `memcpy()` and `strcpy()`. The one difference is between areas and malloc'd memory is…

  * You never `free()` the memory that an area allocates for you. If you want to get rid of an area, use the [`delete_area()`](TheKernelKit_Areas.md#delete_area "delete_area\(\)") function, instead.

* * *

### Area IDs and Area Names

Each area that you create is tagged with an area_id number:

  * An area_id number is a positive integer that's global and unique within the scope of the computer. They're not unique across the network, nor are they persistent across boots.

  * The area_id numbers are generated and assigned automatically by the [`create_area()`](TheKernelKit_Areas.md#create_area "create_area\(\)") and [`clone_area()`](TheKernelKit_Areas.md#clone_area "clone_area\(\)") functions. The other area functions operate on these area_id numbers (they're required as arguments).

  * Although they are global, area_id numbers have little meaning outside of the address space (application) in which they were created.

  * Once assigned, the area_id number doesn't change; the number is invalidated when [`delete_area()`](TheKernelKit_Areas.md#delete_area "delete_area\(\)") is called or when the application (team) that created it dies.

  * Don't worry about recycled area_id numbers. When an area is deleted, its area_id goes with it. (area_id values are recycled, but the turnover is at 2^31.)

Areas can also be (loosely) identified by name:

  * When you create an area (through [`create_area()`](TheKernelKit_Areas.md#create_area "create_area\(\)") or [`clone_area()`](TheKernelKit_Areas.md#clone_area "clone_area\(\)")), you get to name it.

  * Area names are not unique—any number of areas can be assigned the same name.

  * To look up an area by name, use the [`find_area()`](TheKernelKit_Areas.md#find_area "find_area\(\)") function.

* * *

### Sharing an Area Between Applications

For multiple applications to share a common area, one of the applications has to create the area, and the other applications clone the area. You clone an area by calling [`clone_area()`](TheKernelKit_Areas.md#clone_area "clone_area\(\)"). The function takes, as its last argument, the area_id of the source area and returns a new (unique) area_id number. All further references to the cloned area (in the cloning application) must be based on the area_id that's returned by [`clone_area()`](TheKernelKit_Areas.md#clone_area "clone_area\(\)").

So how does a cloner find a source area_id in the first place?

  * The source application can pass the "original" area_id number to the cloners.

  * The cloners can find the area by name, by calling [`find_area()`](TheKernelKit_Areas.md#find_area "find_area\(\)").

Keep in mind that area names are not forced to be unique, so the [`find_area()`](TheKernelKit_Areas.md#find_area "find_area\(\)") method has some amount of uncertainty. But this can be minimized through clever name creation.

#### Cloned Memory

The physical memory that lies beneath an area is never implicitly copied—for example, the area mechanism doesn't perform a "copy-on-write." If two areas refer to the same memory because of cloning, a data modification that's affected through one area will be seen by the other area.

* * *

### Locking an Area

When you're working with moderately large amounts of data, it's often the case that you would prefer that the data remain in RAM, even if the rest of your application needs to be swapped out. An argument to [`create_area()`](TheKernelKit_Areas.md#create_area "create_area\(\)") lets you declare, through the use of one of the following constants, the locking scheme that you wish to apply to your area:

Constant| Description  
---|---  
`B_FULL_LOCK`| The area's memory is locked into RAM when the area is created, and won't be swapped out.  
`B_CONTIGUOUS`| Not only is the area's memory locked into RAM, it's also guaranteed to be contiguous. This is particularly—and perhaps exclusively—useful to designers of certain types of device drivers.  
`B_LAZY_LOCK`| Allows individual pages of memory to be brought into RAM through the natural order of things and then locks them.  
`B_NO_LOCK`| Pages are never locked, they're swapped in and out as needed.  
`B_LOMEM`| This is a special constant that's used for for areas that need to be locked, contiguous, and that fit within the first 16MB of physical memory. The folks that need this constant know who they are.  
  
Keep in mind that locking an area essentially reduces the amount of RAM that can be used by other applications, and so increases the likelihood of swapping. So you shouldn't lock simply because you're greedy. But if the area that you're locking is going to be shared among some number of other applications, or if you're writing a real-time application that processes large chunks of data, then locking can be a justifiable excess.

The locking scheme is set by the [`create_area()`](TheKernelKit_Areas.md#create_area "create_area\(\)") function and is thereafter immutable. You can't re-declare the lock when you clone an area.

* * *

### Area Info

Ultimately, you use an area for the virtual memory that it represents: You create an area because you want some memory to which you can write and from which you can read data. These acts are performed in the usual manner, through references to specific addresses. Setting a pointer to a location within the area, and checking that you haven't exceeded the area's memory bounds as you increment the pointer (while reading or writing) are your own responsibility. To do this properly, you need to know the area's starting address and its extent:

  * An area's starting address is maintained as the `address` field in its area_info structure; you retrieve the area_info for a particular area through the [`get_area_info()`](TheKernelKit_Areas.md#get_area_info "get_area_info\(\), get_next_area_info\(\), area_info") function.

  * The size of the area (in bytes) is given as the `size` field of its area_info structure.

An important point, with regard to area_info, is that the address field is only valid for the application that created or cloned the area (in other words, the application that created the area_id that was passed to [`get_area_info()`](TheKernelKit_Areas.md#get_area_info "get_area_info\(\), get_next_area_info\(\), area_info")). Although the memory that underlies an area is global, the address that you get from an area_info structure refers to a specific address space.

If there's any question about whether a particular area_id is "local" or "foreign," you can compare the area_info.`team` field to your thread's team.

* * *

### Deleting an Area

When your application quits, the areas (the area_id numbers) that it created through create_area() or [`clone_area()`](TheKernelKit_Areas.md#clone_area "clone_area\(\)") are automatically rendered invalid. The memory underlying these areas, however, isn't necessarily freed. An area's memory is freed only when (and as soon as) there are no more areas that refer to it.

You can force the invalidation of an area_id by passing it to the [`delete_area()`](TheKernelKit_Areas.md#delete_area "delete_area\(\)") function. Again, the underlying memory is only freed if yours is the last area to refer to the memory.

Deleting an area, whether explicitly through [`delete_area()`](TheKernelKit_Areas.md#delete_area "delete_area\(\)"), or because your application quit, never affects the status of other areas that were cloned from it.

* * *

### Area Examples

#### Example 1: Creating and Writing into an Area

As a simple example of area creation and usage, here we create a ten page area and fill half of it (with nonsense) by bumping a pointer:
    
    
    area_id my_area;
    char *area_addr, *ptr;
    
    /* Create an area. */
    my_area = create_area("my area", /* name you give to the area */
          (void *)&area_addr, /* returns the starting addr */
          B_ANY_ADDRESS, /* area can start anywhere */
          B_PAGE_SIZE*10, /* size in bytes */
          B_NO_LOCK, /* Lock in RAM? No. */
          B_READ_AREA | B_WRITE_AREA); /* permissions */
    
    /* check for errors */
    if (my_area < 0) {
          printf("Something bad happenedn");
          return;
    }
    
    /* Set ptr to the beginning of the area. */
    ptr = area_addr;
    
    /* Fill half the area (with random-ish data). */
    for (int i; i < B_PAGE_SIZE*5; i++)
       *ptr++ = system_time()%256;

You can also `memcpy()` and `strcpy()` into the area:
    
    
    /* Copy the first half of the area into the second half. */
    memcpy(ptr, area_addr, B_PAGE_SIZE*5);
    
    /* Overwrite the beginning of the area. */
    strcpy(area_addr, "Hey, look where I am.");

When we're all done, we delete the area:
    
    
    delete_area(my_area);

#### Example 2: Reading a File into an Area

Here's a function that finds a file, opens it (implicit in the [`BFile`](BFile.md "BFile") constructor), and copies its contents into RAM:
    
    
    #include <File.h>
    
    area_id file_area;
    
    status_t file_reader(const char *pathname)
    {
       status_t err;
       char *area_addr;
    
       BFile file(pathname, B_READ_ONLY);
       if ((err=file.InitCheck()) != B_OK) {
          printf("%s: Can't find or open.n", pathname);
          return err;
       }
    
       err = file.GetSize(&file_size);
       if (err != B_OK || file_size == 0) {
          printf("%s: Disappeared? Empty?n", pathname);
          return err;
       }
    
       /* Round the size up to the nearest page. */
       file_size = (((file_size-1) % B_PAGE_SIZE)+1)*B_PAGE_SIZE;
    
       /* Make sure the size won't overflow a size_t spec. */
       if (file_size >= ((1<<32)-1) ) {
          printf("%s: What'd you do? Read Montana?n");
          return B_NO_MEMORY;
       }
    
       file_area = create_area("File area", (void *)&area_addr,
          B_ANY_ADDRESS, file_size, B_FULL_LOCK,
          B_READ_AREA | B_WRITE_AREA);
    
       /* Check create_area() errors, as in the last example. */
       ...
    
       /* Read the file; delete the area if there's an error. */
       if ((err=file.Read(area_addr, file_size)) < B_OK) {
          printf("%s: File read error.n");
          delete_area(file_area);
          return err;
       }
    
       /* The file is automatically closed when the stack-based
       * BFile is destroyed.
       */
       return B_OK;
    }

#### Example 3: Accessing a Designated Area

In the previous example, a local variable (`area_addr`) was used to capture the starting address of the newly-created area. If some other function wants to access the area, it must "re-find" the starting address (and the length of the area, for boundary checking). To do this, you call [`get_area_info()`](TheKernelKit_Areas.md#get_area_info "get_area_info\(\), get_next_area_info\(\), area_info").

In the following example, an area is passed in by name; the function, which will write its argument buffer to the area, calls [`get_area_info()`](TheKernelKit_Areas.md#get_area_info "get_area_info\(\), get_next_area_info\(\), area_info") to determine the start and extent of the area, and also to make sure that the area is part of this team. If the area was created by some other team, the function could still write to it, but it would have to clone the area first (cloning is demonstrated in the next example).
    
    
    status_t write_to_area(const char *area_name,
                   const void *buf,
                   size_t len)
    {
       area_id area;
       area_info ai;
       thread_id thread;
       thread_info ti;
       status_t err;
    
       if (!area_name)
          return B_BAD_VALUE;
    
       area = find_area(area_name);
    
       /* Did we find it? */
       if (area < B_OK) {
          printf("Couldn't find area %s.n", area_name);
          return err;
       }
    
       /* Get the info. */
       err = get_area_info(area, &ai);
    
       if (err < B_OK) {
          printf("Couldn't get area info.n");
          return err;
       }
    
       /* Get the team of the calling thread; to do this, we have
       * to look in the thread_info structure.
       */
       err = get_thread_info(find_thread(NULL), &ti);
    
       if (err < B_OK) {
          printf("Couldn't get thread info.n");
          return err;
       }
    
       /* Compare this team to the area's team. */
       if (ai.team != ti.team)
          printf("Foreign area.n");
          return B_NOT_ALLOWED;
       }
    
       /* Make sure we're not going to overflow the area,
       * and make sure this area can be written to.
       */
       if (len > ai.size) {
          printf("Buffer bigger than area.n");
          return B_BAD_VALUE;
       }
       if (!(ai.protection & B_WRITE_AREA)) {
          printf("Can't write to this area.n");
          return B_NOT_ALLOWED;
       }
    
       /* Now we can write. */
       memcpy(ai.address, buf, len);
       return B_OK;
    }

It's important that you only write to areas that were created or cloned within the calling team. The starting address of a "foreign" area is usually meaningless within your own address space.

You don't have to check the area's protectection before writing to it (or reading from it). The memory-accessing fucntions (`memcpy()`, in this example) will segfault if an invalid read or write is requested.

#### Example 4: Cloning and Sharing an Area

In the following example, a server and a client are set up to share a common area. Here's the server:
    
    
    /* Server side */
    class AServer
    {
       status_t make_shared_area(size_t size);
       area_id the_area;
       char *area_addr;
    };
    
    status_t AServer::make_shared_area(size_t size)
    {
       /* The size must be rounded to a page. */
       size = ((size % B_PAGE_SIZE)+1) * B_PAGE_SIZE;
       the_area = create_area("server area", (void *)&area_addr,
                B_ANY_ADDRESS, size, B_NO_LOCK,
                B_READ_AREA|B_WRITE_AREA);
    
       if (the_area < B_OK) {
          printf("Couldn't create server arean");
          return the_area;
    
       return B_OK;
    }

And here's the client:
    
    
    /* Client side */
    class AClient
    {
       status_t make_shared_clone();
       area_id the_area;
       char *area_addr;
    };
    
    status_t AClient::make_shared_clone()
    {
       area_id src_area;
    
       src_area = find_area("server area");
       if (src_area < B_ERROR) {
          printf("Couldn't find server area.n");
          return src_area;
       }
    
       the_area = clone_area("client area",
                   (void *)&area_addr,
                      B_ANY_ADDRESS,
                   B_READ_AREA | B_WRITE_AREA,
                   src_area);
    
       if (the_area < B_OK)
          printf("Couldn't create clone arean");
          return the_area;
       }
    
       return B_OK;
    }

Notice that the area creator (the server in the example) doesn't have to designate the created area as sharable. All areas are candidates for cloning.

After it creates the cloned area, the client's area_id value (`AClient`::`the_area`) will be different from the server's (`AServer`::`the_area`). Even though area_id numbers are global, the client should only refer to the server's area_id number in order to clone it. After the clone, the client talks to the area through its own area_id (the value passed backed by [`clone_area()`](TheKernelKit_Areas.md#clone_area "clone_area\(\)")).

#### Example 5: Cloning Addresses

It's sometimes useful for shared areas (in other words, a "source" and a clone) to begin at the same starting address. For example, if a client's clone area starts at the same address as the server's original area, then the client and server can pass area-accessing pointers back and forth without having to translate the addresses. Here we modify the previous example to do this:
    
    
    status_t AClient::make_shared_clone()
    {
       area_id src_area;
    
       src_area = find_area("server area");
    
       if (src_area < B_ERROR) {
          printf("Couldn't find server area.n");
          return B_BAD_VALUE;
       }
    
       /* This time, we specify the address that we want the
       * clone to start at. The B_CLONE_ADDRESS constant
       * does this for us.
       */
       area_addr = src_info.address;
       the_area = clone_area("client area",
                   (void *)&area_addr,
                      B_CLONE_ADDRESS,
                   B_READ_AREA | B_WRITE_AREA,
                   src_area);
    
       if (the_area < B_OK)
          printf("Couldn't create clone arean");
          return the_area;
       }
    
       return B_OK;
    }

Of course, demanding that an area begin at a specific address can be too restrictive; if any of the memory within [`area_addr`, `area_addr` \+ `src_info`.`size`] is already allocated, the clone will fail.

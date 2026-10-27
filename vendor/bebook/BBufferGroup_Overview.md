<!-- System Overview - The Media Kit - BBufferGroup -->

## BBufferGroup  
  
A [`BBufferGroup`](BBufferGroup.md "BBufferGroup") serves as a repository for [`BBuffer`](BBuffer.md "BBuffer")s, and can be used to pass those buffers to another node for its use when sending data. It must also be used by your [`BBufferProducer`](BBufferProducer.md "BBufferProducer") to acquire [`BBuffer`](BBuffer.md "BBuffer")s to put data that you produce into before you send the buffer on to its destination.

You can create a new [`BBufferGroup`](BBufferGroup.md "BBufferGroup") by simply calling:
    
    
    new BBufferGroup;

If you pass in some arguments describing what sort of buffers you want, and how many of them, the group will even create the [`BBuffer`](BBuffer.md "BBuffer")s for you. This is how you'll usually use [`BBufferGroup`](BBufferGroup.md "BBufferGroup"), from within your [`BBufferProducer`](BBufferProducer.md "BBufferProducer") subclass' constructor so you'll know there are buffers ready and waiting when it's time to start sending data. A possible exception is if your node is a filter that simply processes buffers it receives and passes them along.

A [`BBufferGroup`](BBufferGroup.md "BBufferGroup") instance runs a thread that reclaims [`BBuffer`](BBuffer.md "BBuffer")s whose [`Recycle()`](BBuffer.md#BBuffer_Recycle "Recycle\(\)") function has been called. If the group is temporarily out of free buffers, a request for a buffer may block until one is available, or until the request times out, if a timeout is specified when the request is made.

* * *

### Using BBitmaps as Buffers

If you're doing video processing, you might want your buffers to be [`BBitmap`](BBitmap.md "BBitmap")s. Here's how you can accomplish this:
    
    
    BBufferGroup *my_group = new BBufferGroup;
    BBitmap *my_bitmap = new BBitmap(BRect(0,0,639,479), B_BITMAP_IS_AREA,
                B_RGB32, 640*4);
    
    if (!my_bitmap->IsValid()) {
       return B_ERROR;
    }
    area_info bm_info;
    
    if (get_area_info(my_bitmap->Area(), &bm_info) != B_OK) {
       return B_ERROR;
    }
    
    buffer_clone_info bc_info;
    bc_info.area = bm_info.area;
    bc_info.offset = ((char *) my_bitmap->Bits())-((char *) bm_info.address);
    bc_info.size = my_bitmap->BitsLength();
    
    BBuffer *out_buffer = NULL;
    status_t err = my_group->AddBuffer(bc_info, &out_buffer);
    
    /* out_buffer is now set to use the BBitmap's memory */

This code takes advantage of the `B_BITMAP_IS_AREA` flag when creating the bitmap. This forces the bitmap to be in a memory area of its very own, which you can then use when creating the [`BBuffer`](BBuffer.md "BBuffer").

If you need to do real-time accesses to the [`BBuffer`](BBuffer.md "BBuffer"), you should lock it down. In addition, if you use DMA, you'll need to specify for the buffer to be contiguous (you can do this using the [`BBitmap`](BBitmap.md "BBitmap") constructor, specifying the contiguous flag).

Before deleting any [`BBitmap`](BBitmap.md "BBitmap")s used in this way, be sure the buffer group has been deleted first and (if [`SetOutputBuffersFor()`](BBufferConsumer.md#BBufferConsumer_SetOutputBuffersFor "SetOutputBuffersFor\(\)") has been used) all buffers have been reclaimed.

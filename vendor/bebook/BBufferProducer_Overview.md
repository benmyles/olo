<!-- System Overview - The Media Kit - BBufferProducer -->

## BBufferProducer

A [`BBufferProducer`](BBufferProducer.md "BBufferProducer") is a [`BMediaNode`](BMediaNode.md "BMediaNode") that emits buffers containing media data that other nodes ([`BBufferConsumer`](BBufferConsumer.md "BBufferConsumer")s in particular) will receive and, potentially, process. If your node wants to emit buffers, it must be derived from [`BBufferProducer`](BBufferProducer.md "BBufferProducer") and override the hook functions to implement the [`BBufferProducer`](BBufferProducer.md "BBufferProducer") protocol.

* * *

### Video Clipping

Currently, the only video clipping format supported by the Media Kit is `B_CLIP_SHORT_RUNS`, although there is a function in this class for converting between this format and [`BRegion`](BRegion.md "BRegion")s.

This format begins with a header, consisting of two int16 values:

Field| Description  
---|---  
`offsetX`| X offset for all following coordinates.  
`offsetY`| Y offset for all following coordinates.  
  
These values indicate the offset for the X and Y coordinates indicated throughout the rest of the clipping data.

The remainder of the clipping data consists of entries indicating each line of video data, as follows:

Field| Description  
---|---  
`numShorts`| The number of values in the `coordList`. Always an even number. If negative, repeats the previous entry `numShorts` times.  
`coordList…`| List of coordinates. Even entries are left-edge X coordinates, odd entries are right-edge X coordinates.  
  
The clipping data contains one of these entries for each time the clipping information changes.

For example, if the clipping is a rectangle with the left edge at 100, top edge at 50, right edge at 300, and bottom edge at 200, the clipping data for a 640x480 display might be:
    
    
    header
       offsetX: 0
       offsetY: 50
    
    entry 1
       numShorts: 2
       coordList: 100, 300
    
    entry 2
       numShorts: -150
    
    entry 3
       numShorts: 2
       coordList: 0, 639
    
    entry 4
       numShorts: -280

The header indicates that the clipping data begins at row 50.

The first entry indicates that clipping should span from column 100 to column 300 on the first row of clipping (row 50). The second entry says to repeat this 150 times.

Entry 3 indicates that clipping from that point on should be from column 0 to column 639 (the entire width of the display). Entry 4 causes this to repeat 280 times, to the bottom of the display.

* * *

### Seek Tags

In order to support media formats that don't provide timing information in their outer encapsulation layer, or to provide enhanced seeking performance for media formats that support key frames, the Media Kit supports the concept of seek tags. Producers that know their data doesn't have timing information, or that can provide enhanced seeking using special tags, should put a tag in the `user_data` field of the buffer headers it sends. This tag can contain any data the producer wants.

Consumers that can derive good timing information from these packets after decoding them should then choose appropriate seek points (usually key frames) and cache the performance time and tag values of the first buffer that arrives at that seek point.

Producers that can't seek without help from the decoder can then query the consumer by calling [`FindSeekTag()`](BBufferProducer.md#BBufferProducer_FindSeekTag "FindSeekTag\(\)"). This causes the consumer's [`SeekTagRequested()`](BBufferConsumer.md#BBufferConsumer_SeekTagRequested "SeekTagRequested\(\)") function to be called. This returns the seek tag and time that are closest to the requested time. The producer can then use this information locate the appropriate point in the media data.

The easiest way to use this is to use the file offset as the tag data, but any value that makes sense to the producer can be used, since the consumer just saves a copy of the data and passes it back without looking at it.
    
    
    Time          Seek Tag
    0.0 seconds   0
    0.1 seconds   <none>
    0.2 seconds   2
    0.3 seconds   <none>

In this simple example, we have four buffers, two of which have seek tags recorded (at 0.0 seconds and 0.3 seconds). If the producer is seeking to 0.2 seconds, it would call [`FindSeekTag()`](BBufferProducer.md#BBufferProducer_FindSeekTag "FindSeekTag\(\)"), like this:
    
    
    media_seek_tag tag;
    bigtime_t time;
    FindSeekTag(&destination, 0.2*1000000, &tag, &time);
    
    /* now we can use the tag to seek */

If the tag contains a file offset, we can simply seek to that offset in the file and we're ready to go.

In this example, the returned tag is "2" and the time is 0.2 seconds, because there's a seek tag located precisely at the requested time. However, if we look for a seek tag for 0.1 seconds, we get a returned tag of "0" and a time of 0.0 seconds, because that's the closest matching tag to the requested time.

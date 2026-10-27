<!-- Special Topics - Device Drivers - Message Constants -->

## Message Constants

* * *

###  B_ACQUIRE_OVERLAY_LOCK, B_RELEASE_OVERLAY_LOCK

Purpose:| Deliverable  
---|---  
Source:| A graphics driver.  
Target:| The team owning the overlay.  
  
`B_ACQUIRE_OVERLAY_LOCK` is sent by a graphics driver when an overlay is acquired. `B_RELEASE_OVERLAY_LOCK` is sent when the overlay is released.

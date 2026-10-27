<!-- System Overview - The Support Kit - BAutolock -->

## BAutolock

The [`BAutolock`](BAutolock.md "BAutolock") class provides an easy mechanism for automatically locking and unlocking a [`BLocker`](BLocker.md "BLocker") object or, more typically, a [`BLooper`](BLooper.md "BLooper"). All you need to do is allocate a [`BAutolock`](BAutolock.md "BAutolock") object on the stack and pass it a target [`BLocker`](BLocker.md "BLocker") or [`BLooper`](BLooper.md "BLooper"). You may want to make sure the lock is in place before proceeding:
    
    
    BAutolock autolocker(myWindow);
    if ( autolocker.IsLocked() ) {
       . . .
    }

When the stack frame is destroyed, the [`BAutolock`](BAutolock.md "BAutolock") object unlocks the target object and then is itself destroyed.

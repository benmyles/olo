<!-- Classes And Methods - The Support Kit - BAutolock -->

## BAutolock

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `support/Autolock.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BAutolock_Overview.md "BAutolock")

  * [Constructor and Destructor](BAutolock.md#BAutolock_ConstructorDestructor)
  * [Member Functions](BAutolock.md#BAutolock_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BAutolock()

`inline BAutolock([`BLooper`](BLooper.md "BLooper")* looper);` `inline BAutolock([`BLocker`](BLocker.md "BLocker")* locker);` `inline BAutolock([`BLocker`](BLocker.md "BLocker")& locker);`

Locks the target `looper` or `locker` object.

#### ~BAutolock()

`inline ~BAutolock();`

Unlocks the target [`BLooper`](BLooper.md "BLooper") or [`BLocker`](BLocker.md "BLocker").

* * *

### Member Functions

#### IsLocked()

`inline bool IsLocked();`

Returns `true` if the target [`BLooper`](BLooper.md "BLooper") or [`BLocker`](BLocker.md "BLocker"). is locked, and `false` if not.

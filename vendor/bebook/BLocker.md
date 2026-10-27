<!-- Classes And Methods - The Support Kit - BLocker -->

## BLocker

| Derived From:| –  
---|---  
Mix-in Classes:| –  
Declared In:| `support/Locker.h`  
Library:| `libbe.so`  
Allocation:| –  
[Class Overview](BLocker_Overview.md "BLocker")

  * [Constructor and Destructor](BLocker.md#BLocker_ConstructorDestructor)
  * [Member Functions](BLocker.md#BLocker_MemberFunctions)

  
  
* * *

### Constructor and Destructor

#### BLocker()

`BLocker();` `BLocker(const char* name);` `BLocker(bool benaphore_style);` `BLocker(const char* name,  
bool benaphore_style);`

Sets up the object, creating a semaphore to implement the locking mechanism. The optional `name` is for diagnostics and debugging.

By default, a `BLocker` is implemented as a benaphore—a hybrid integer mutex and semaphore. This allows for faster performance when there is little contention for the lock. If you'd rather use a pure semaphore for locking, then pass `false` as the `benaphore_style` argument.

#### ~BLocker()

`virtual ~BLocker();`

Destroys the lock, deleting the controlling semaphore. If there are any threads blocked waiting to lock the object, they're immediately unblocked.

* * *

### Member Functions

####  Lock() , LockWithTimeout() , Unlock()

`bool Lock();``status_t LockWithTimeout(bigtime_t timeout);``void Unlock();`

`Lock()` tries to lock the `BLocker`. The function returns **a)** when the lock is acquired (return = `true`) or **b)** immediately if the `BLocker`'s semaphore is deleted, most commonly as a result of deleting the object (return = `false`). A thread can nest `Lock()` calls, but each call must be balanced by a concomitant `Unlock()`.

`LockWithTimeout()` lets you declare a time limit, specified in microseconds. If `LockWithTimeout()` can't acquire the lock before the time limit expires, it returns `B_TIMED_OUT`. If the timeout is 0, this function immediately returns `B_OK` (if it locked the `BLocker` ) or `B_ERROR` (if it failed to obtain the lock). If the timeout is `B_INFINITE_TIMEOUT`, it blocks without limit, just as `Lock()` does. Note that if `Lock()` returns 0 (`false`), it has failed to lock the `BLocker`, but if `LockWithTimeout()` returns 0 (`B_OK`), it has succeeded.

`Unlock()` releases one level of nested locks and returns immediately. If there are threads blocked waiting for the lock when the lock is released, the thread that's been waiting the longest acquires the lock.

Any thread can call `Unlock()` on any `BLocker`—the thread needn't be the lock's current holder. Call `IsLocked()` before calling `Unlock()` if you want to make sure you own a lock before you unlock it.

####  LockingThread(), IsLocked(), CountLocks(), CountLockRequests(), Sem()

`thread_id LockingThread() const;``bool IsLocked() const;``int32 CountLocks() const;``int32 CountLockRequests() const;``sem_id Sem() const;`

These functions provide information that may be useful for debugging purposes.

`LockingThread()` returns the thread that currently has the `BLocker` locked, or `B_ERROR` if the `BLocker` isn't locked.

`IsLocked()` returns `true` if the calling thread currently has the `BLocker` locked (if it's the locking thread) and `false` if not (if some other thread is the locking thread or the `BLocker` isn't locked).

`CountLocks()` returns the number of times the locking thread has locked the `BLocker`—the number of `Lock()` (or `LockWithTimeout()`) calls that have not yet been balanced by matching `Unlock()` calls.

`CountLockRequests()` returns the number of threads currently trying to lock the `BLocker`. The count includes the thread that currently holds the lock plus all threads currently waiting to acquire it.

`Sem()` returns the sem_id for the semaphore that the `BLocker` uses to implement the locking mechanism.

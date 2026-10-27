<!-- System Overview - The Support Kit - BLocker -->

## BLocker

The [`BLocker`](BLocker.md "BLocker") class provides a locking mechanism that can be used to protect a section of critical code. The code that you want to protect should be placed between [`BLocker`](BLocker.md "BLocker")'s [`Lock()`](BLocker.md#BLocker_Lock "Lock\(\) , LockWithTimeout\(\) , Unlock\(\)") and [`Unlock()`](BLocker.md#BLocker_Unlock) calls:
    
    
    BLocker *aLock = new BLocker();
    . . .
    aLock->Lock();
    /* Protected code goes here. */
    aLock->Unlock();

This guarantees that only one thread at a time will pass through the lock. After a thread has locked the [`BLocker`](BLocker.md "BLocker") object, subsequent attempts to lock by other threads are blocked until the first thread calls [`Unlock()`](BLocker.md#BLocker_Unlock).

[`BLocker`](BLocker.md "BLocker") keeps track of the locking thread—the thread that's currently between [`Lock()`](BLocker.md#BLocker_Lock "Lock\(\) , LockWithTimeout\(\) , Unlock\(\)") and [`Unlock()`](BLocker.md#BLocker_Unlock) calls. It lets the thread make nested calls to [`Lock()`](BLocker.md#BLocker_Lock "Lock\(\) , LockWithTimeout\(\) , Unlock\(\)") without blocking. Because of this, you can wrap a [`BLocker`](BLocker.md "BLocker")'s lock around a series of functions that might, themselves, lock the same [`BLocker`](BLocker.md "BLocker") object.

For example, let's say you have a class called `BadDog` that's declared thus:
    
    
    class BadDog : public BArchivable
    {
    public:
       void DoThis();
       void DoThat();
       void DoThisAndThat();
    
    private:
       BLocker lock;
    };

Let's implement the member functions as follows:
    
    
    void BadDog::DoThis()
    {
       lock.Lock();
       /* Do this here. */
       lock.Unlock();
    }
    
    void BadDog::DoThat()
    {
       lock.Lock();
       /* Do that here. */
       lock.Unlock();
    }
    
    void BadDog::DoThisAndThat()
    {
       lock.Lock();
       DoThis();
       DoThat();
       lock.Unlock();
    }

Notice that `DoThisAndThat()` wraps the lock around its calls to `DoThis()` and `DoThat()`, both of which contain locks as well. A thread that gets past the [`Lock()`](BLocker.md#BLocker_Lock "Lock\(\) , LockWithTimeout\(\) , Unlock\(\)") call in `DoThisAndThat()` won't block when it calls the nested [`Lock()`](BLocker.md#BLocker_Lock "Lock\(\) , LockWithTimeout\(\) , Unlock\(\)") calls that it runs into in `DoThis()` and `DoThat()`.

Keep in mind that nested [`Lock()`](BLocker.md#BLocker_Lock "Lock\(\) , LockWithTimeout\(\) , Unlock\(\)") calls must be balanced by an equal number of [`Unlock()`](BLocker.md#BLocker_Unlock) calls.

See also: the [`BAutolock`](BAutolock.md "BAutolock") class

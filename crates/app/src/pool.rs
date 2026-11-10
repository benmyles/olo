//! The worker pool: where work that can take longer than a frame goes.
//!
//! Threads start when there is work and none is free (one a core at
//! most) and leave after ten seconds without any, so an idle program
//! keeps none.

use std::{
    collections::VecDeque,
    future::Future,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use olo_sync::{Condvar, Mutex};

use crate::Task;

const LINGER: Duration = Duration::from_secs(10);

struct Pool {
    queue: Mutex<VecDeque<async_task::Runnable>>,
    work: Condvar,
    max: usize,
    running: AtomicUsize,
    idle: AtomicUsize,
    next: AtomicUsize,
}

fn pool() -> &'static Arc<Pool> {
    static POOL: OnceLock<Arc<Pool>> = OnceLock::new();
    POOL.get_or_init(|| {
        let max = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).max(2);
        Arc::new(Pool { queue: Mutex::new(VecDeque::new()), work: Condvar::new(), max, running: AtomicUsize::new(0), idle: AtomicUsize::new(0), next: AtomicUsize::new(0) })
    })
}

impl Pool {
    fn dispatch(self: &Arc<Self>, runnable: async_task::Runnable) {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).push_back(runnable);
        if self.idle.load(Ordering::Acquire) > 0 {
            self.work.notify_one();
            return;
        }
        // Nobody is free: another thread, unless there's one a core
        // already (then the work waits its turn).
        if self.running.try_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < self.max).then_some(n + 1)).is_ok() {
            let pool = self.clone();
            let i = self.next.fetch_add(1, Ordering::Relaxed);
            if std::thread::Builder::new().name(format!("worker-{i}")).spawn(move || pool.work()).is_err() {
                self.running.fetch_sub(1, Ordering::AcqRel);
            }
        } else {
            self.work.notify_one();
        }
    }

    fn work(self: Arc<Self>) {
        loop {
            let runnable = {
                let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
                loop {
                    if let Some(r) = queue.pop_front() {
                        break Some(r);
                    }
                    self.idle.fetch_add(1, Ordering::AcqRel);
                    let (q, timed_out) = self.work.wait_timeout(queue, LINGER).unwrap_or_else(|e| e.into_inner());
                    queue = q;
                    self.idle.fetch_sub(1, Ordering::AcqRel);
                    if timed_out.timed_out() && queue.is_empty() {
                        // (Leaving is decided with the queue held, so
                        // work put now finds either this thread or none
                        // counted.)
                        self.running.fetch_sub(1, Ordering::AcqRel);
                        break None;
                    }
                }
            };
            match runnable {
                Some(r) => {
                    r.run();
                }
                None => return,
            }
        }
    }
}

/// Runs a future on the pool's threads. Await the [`Task`] from a
/// looper's task for its result, or have the future send a message when
/// it's done and `detach` it.
pub fn spawn<T: Send + 'static>(future: impl Future<Output = T> + Send + 'static) -> Task<T> {
    let pool = pool().clone();
    let (runnable, task) = async_task::spawn(future, move |runnable| pool.dispatch(runnable));
    runnable.schedule();
    Task::new(task)
}

/// Runs a blocking call on a pool thread (reading a folder, a decode, a
/// round trip to a service): the way to do anything that might take
/// longer than a frame.
pub fn run<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Task<T> {
    spawn(async move { f() })
}

/// How many pool threads there are now (for tests and the curious).
pub fn threads() -> usize {
    pool().running.load(Ordering::Acquire)
}

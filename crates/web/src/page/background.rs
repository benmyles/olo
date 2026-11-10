//! Bounded, demand-driven CPU work. Workers drain the queue and exit;
//! there are no idle threads, timers or polling. Removing queued work
//! drops its captured inputs immediately.
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

type Work = Box<dyn FnOnce() + Send>;
struct State {
    queued: VecDeque<(usize, Work)>,
    workers: usize,
}
pub(super) struct Queue {
    state: Mutex<State>,
    limit: usize,
}
impl Queue {
    pub(super) fn new(limit: usize) -> Arc<Self> {
        assert!(limit > 0);
        Arc::new(Self { state: Mutex::new(State { queued: VecDeque::new(), workers: 0 }), limit })
    }
    pub(super) fn submit(self: &Arc<Self>, id: usize, work: Work) -> std::io::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.queued.push_back((id, work));
        if state.workers < self.limit {
            state.workers += 1;
            let queue = self.clone();
            if let Err(error) = std::thread::Builder::new().name("olo-style-compile".into()).spawn(move || queue.run()) {
                state.workers -= 1;
                if state.workers == 0 {
                    let removed = state.queued.pop_back();
                    drop(state);
                    drop(removed);
                    return Err(error);
                }
                // An existing worker will drain this job too.
            }
        }
        Ok(())
    }
    pub(super) fn remove(&self, id: usize) {
        let removed = {
            let mut state = self.state.lock().unwrap();
            state.queued.iter().position(|(key, _)| *key == id).and_then(|i| state.queued.remove(i))
        };
        // A captured value's destructor may submit or cancel other work.
        drop(removed);
    }
    fn run(&self) {
        loop {
            let work = {
                let mut state = self.state.lock().unwrap();
                match state.queued.pop_front() {
                    Some((_, work)) => work,
                    None => {
                        // Submit and exit decide under the same lock:
                        // a job cannot be left with no worker to run it.
                        state.workers -= 1;
                        return;
                    }
                }
            };
            // One failing job must not strand the other queued pages.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };
    use std::time::Duration;

    #[test]
    fn bounds_work_removes_queued_inputs_and_keeps_survivors() {
        let queue = Queue::new(2);
        let (started, ready) = mpsc::channel();
        let (done, finished) = mpsc::channel();
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut releases = Vec::new();
        for id in 0..2 {
            let (release, wait) = mpsc::channel();
            releases.push(release);
            let (started, done, active, peak) = (started.clone(), done.clone(), active.clone(), peak.clone());
            queue
                .submit(
                    id,
                    Box::new(move || {
                        let n = active.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(n, Ordering::SeqCst);
                        started.send(()).unwrap();
                        wait.recv_timeout(Duration::from_secs(10)).unwrap();
                        active.fetch_sub(1, Ordering::SeqCst);
                        done.send(id).unwrap();
                    }),
                )
                .unwrap();
        }
        for _ in 0..2 {
            ready.recv_timeout(Duration::from_secs(10)).unwrap();
        }
        let mut inputs = Vec::new();
        for id in 2..102 {
            let input = Arc::new(vec![0u8; 4096]);
            inputs.push(Arc::downgrade(&input));
            let (done, active, peak) = (done.clone(), active.clone(), peak.clone());
            queue
                .submit(
                    id,
                    Box::new(move || {
                        let n = active.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(n, Ordering::SeqCst);
                        assert_eq!(input.len(), 4096);
                        active.fetch_sub(1, Ordering::SeqCst);
                        done.send(id).unwrap();
                    }),
                )
                .unwrap();
        }
        for id in (2..102).step_by(2) {
            queue.remove(id);
        }
        assert_eq!(queue.state.lock().unwrap().queued.len(), 50);
        for (i, input) in inputs.iter().enumerate() {
            assert_eq!(input.upgrade().is_some(), i % 2 == 1, "removed inputs freed before workers resume");
        }
        for release in releases {
            release.send(()).unwrap();
        }
        let mut ids: Vec<_> = (0..52).map(|_| finished.recv_timeout(Duration::from_secs(10)).unwrap()).collect();
        ids.sort_unstable();
        assert_eq!(ids, [vec![0, 1], (3..102).step_by(2).collect()].concat());
        assert_eq!(peak.load(Ordering::SeqCst), 2);
    }
}

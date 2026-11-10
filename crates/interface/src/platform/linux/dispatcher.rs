use crate::{PlatformDispatcher, TaskLabel};
use async_task::Runnable;
use calloop::{
    EventLoop,
    channel::{self, Sender},
    timer::TimeoutAction,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use util::ResultExt;

struct TimerAfter {
    duration: Duration,
    runnable: Runnable,
}

pub(crate) struct LinuxDispatcher {
    main_sender: Sender<Runnable>,
    timer_sender: Sender<TimerAfter>,
    workers: Arc<Workers>,
    _timer_thread: thread::JoinHandle<()>,
    main_thread_id: thread::ThreadId,
}

/// Olo: background threads start when there's work none is free for (up
/// to one a core) and leave after a while without any, so an idle app
/// keeps none. (GPUI started one a core up front and kept them.)
struct Workers {
    sender: flume::Sender<Runnable>,
    receiver: flume::Receiver<Runnable>,
    max: usize,
    running: AtomicUsize,
    idle: AtomicUsize,
    next: AtomicUsize,
}

const WORKER_LINGER: Duration = Duration::from_secs(10);

impl Workers {
    fn dispatch(this: &Arc<Self>, runnable: Runnable) {
        this.sender.send(runnable).unwrap();
        if this.idle.load(Ordering::Acquire) == 0 && this.running.try_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < this.max).then_some(n + 1)).is_ok() {
            let workers = this.clone();
            let i = this.next.fetch_add(1, Ordering::Relaxed);
            let spawned = std::thread::Builder::new().name(format!("Worker-{i}")).spawn(move || workers.work(i));
            if spawned.is_err() {
                this.running.fetch_sub(1, Ordering::AcqRel);
            }
        }
    }

    fn work(&self, i: usize) {
        loop {
            self.idle.fetch_add(1, Ordering::AcqRel);
            let next = self.receiver.recv_timeout(WORKER_LINGER);
            self.idle.fetch_sub(1, Ordering::AcqRel);
            let Ok(runnable) = next else {
                self.running.fetch_sub(1, Ordering::AcqRel);
                // (Work sent as it was leaving: another takes it.)
                if !self.receiver.is_empty() && self.idle.load(Ordering::Acquire) == 0 {
                    self.running.fetch_add(1, Ordering::AcqRel);
                    continue;
                }
                return;
            };
            let start = Instant::now();
            runnable.run();
            log::trace!("background thread {}: ran runnable. took: {:?}", i, start.elapsed());
        }
    }
}

impl LinuxDispatcher {
    pub fn new(main_sender: Sender<Runnable>) -> Self {
        let (sender, receiver) = flume::unbounded::<Runnable>();
        let workers = Arc::new(Workers {
            sender,
            receiver,
            max: std::thread::available_parallelism().map(|i| i.get()).unwrap_or(1),
            running: AtomicUsize::new(0),
            idle: AtomicUsize::new(0),
            next: AtomicUsize::new(0),
        });

        let (timer_sender, timer_channel) = calloop::channel::channel::<TimerAfter>();
        let timer_thread = std::thread::Builder::new()
            .name("Timer".to_owned())
            .spawn(|| {
                let mut event_loop: EventLoop<()> = EventLoop::try_new().expect("Failed to initialize timer loop!");

                let handle = event_loop.handle();
                let timer_handle = event_loop.handle();
                handle
                    .insert_source(timer_channel, move |e, _, _| {
                        if let channel::Event::Msg(timer) = e {
                            // This has to be in an option to satisfy the borrow checker. The callback below should only be scheduled once.
                            let mut runnable = Some(timer.runnable);
                            timer_handle
                                .insert_source(calloop::timer::Timer::from_duration(timer.duration), move |_, _, _| {
                                    if let Some(runnable) = runnable.take() {
                                        runnable.run();
                                    }
                                    TimeoutAction::Drop
                                })
                                .expect("Failed to start timer");
                        }
                    })
                    .expect("Failed to start timer thread");

                event_loop.run(None, &mut (), |_| {}).log_err();
            })
            .unwrap();

        Self { main_sender, timer_sender, workers, _timer_thread: timer_thread, main_thread_id: thread::current().id() }
    }
}

impl PlatformDispatcher for LinuxDispatcher {
    fn is_main_thread(&self) -> bool {
        thread::current().id() == self.main_thread_id
    }

    fn dispatch(&self, runnable: Runnable, _: Option<TaskLabel>) {
        Workers::dispatch(&self.workers, runnable);
    }

    fn dispatch_on_main_thread(&self, runnable: Runnable) {
        self.main_sender.send(runnable).unwrap_or_else(|runnable| {
            // NOTE: Runnable may wrap a Future that is !Send.
            //
            // This is usually safe because we only poll it on the main thread.
            // However if the send fails, we know that:
            // 1. main_receiver has been dropped (which implies the app is shutting down)
            // 2. we are on a background thread.
            // It is not safe to drop something !Send on the wrong thread, and
            // the app will exit soon anyway, so we must forget the runnable.
            std::mem::forget(runnable);
        });
    }

    fn dispatch_after(&self, duration: Duration, runnable: Runnable) {
        self.timer_sender.send(TimerAfter { duration, runnable }).ok();
    }
}

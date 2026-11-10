//! A looper: a thread, its queue and what lives in it.

use std::{
    any::{Any, TypeId},
    cell::Cell,
    collections::{BinaryHeap, HashMap, VecDeque},
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
    },
    thread::{self, ThreadId},
    time::{Duration, Instant},
};

use crate::{Messenger, Task};

/// Something posted to a looper from any thread.
type Envelope = Box<dyn FnOnce(&mut Looper) + Send>;
/// Something a looper put off for itself (it needn't be `Send`).
type Deferred = Box<dyn FnOnce(&mut Looper)>;
/// Something a looper runs more than once (a repeating timer, what it
/// does when its queue is dry).
type Again = Box<dyn FnMut(&mut Looper)>;

/// How long messages may keep coming before the looper does what it does
/// when its queue is dry anyway (a window lays out and shows what
/// changed): a flood can't starve it.
const BUSY_FOR: Duration = Duration::from_millis(4);

/// A handler's turn taking longer than this is worth a line in the log
/// (`OLO_SLOW_MESSAGES=ms` changes it, 0 turns it off): a looper that is
/// busy answers nothing else meanwhile.
fn slow() -> Option<Duration> {
    static SLOW: std::sync::OnceLock<Option<Duration>> = std::sync::OnceLock::new();
    *SLOW.get_or_init(|| match std::env::var("OLO_SLOW_MESSAGES").ok().and_then(|v| v.parse::<u64>().ok()) {
        Some(0) => None,
        Some(ms) => Some(Duration::from_millis(ms)),
        None => Some(Duration::from_millis(50)),
    })
}

thread_local! {
    /// The looper running on this thread, if one is.
    static CURRENT: Cell<Option<ThreadId>> = const { Cell::new(None) };
}

struct Shared {
    name: String,
    tx: Sender<Envelope>,
    thread: ThreadId,
    /// Set when the looper has ended: nothing posted will be taken.
    gone: AtomicBool,
    /// Messages posted and not yet taken, for whoever asks how far
    /// behind a looper is.
    waiting: AtomicU64,
}

/// A looper as other threads know it: somewhere to post to.
#[derive(Clone)]
pub struct LooperHandle {
    shared: Arc<Shared>,
}

impl LooperHandle {
    /// Posts `f` to run on the looper's thread, after what's already
    /// queued. False if the looper has ended (`f` is dropped).
    pub fn post(&self, f: impl FnOnce(&mut Looper) + Send + 'static) -> bool {
        if self.shared.gone.load(Ordering::Acquire) {
            return false;
        }
        self.shared.waiting.fetch_add(1, Ordering::Relaxed);
        if self.shared.tx.send(Box::new(f)).is_err() {
            self.shared.waiting.fetch_sub(1, Ordering::Relaxed);
            return false;
        }
        true
    }

    /// Posts something that must not be dropped on another thread (a
    /// local task's turn): if the looper has ended it's leaked instead.
    pub(crate) fn post_or_leak(&self, f: impl FnOnce(&mut Looper) + Send + 'static) {
        self.shared.waiting.fetch_add(1, Ordering::Relaxed);
        if let Err(back) = self.shared.tx.send(Box::new(f)) {
            self.shared.waiting.fetch_sub(1, Ordering::Relaxed);
            std::mem::forget(back);
        }
    }

    /// Asks the looper to end once it has taken what's queued before
    /// this.
    pub fn quit(&self) {
        self.post(|looper| looper.quit());
    }

    pub fn name(&self) -> &str {
        &self.shared.name
    }

    /// Whether the calling thread is this looper's.
    pub fn is_current(&self) -> bool {
        thread::current().id() == self.shared.thread
    }

    /// Whether the looper has ended.
    pub fn is_gone(&self) -> bool {
        self.shared.gone.load(Ordering::Acquire)
    }

    /// How many messages are waiting to be taken.
    pub fn waiting(&self) -> u64 {
        self.shared.waiting.load(Ordering::Relaxed)
    }
}

impl std::fmt::Debug for LooperHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Looper({})", self.shared.name)
    }
}

/// Something that lives in a looper and takes messages there: BeOS's
/// `BHandler`, its `Message` what `MessageReceived` could be handed.
pub trait Handler: 'static {
    type Message: 'static;

    fn message_received(&mut self, message: Self::Message, looper: &mut Looper);
}

/// A handler's place in its looper.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HandlerId {
    index: u32,
    generation: u32,
}

struct Slot {
    generation: u32,
    /// None while the handler is taking a message (it's out), or gone.
    handler: Option<Box<dyn Any>>,
    live: bool,
    /// Removed while it was out: dropped when it comes back.
    doomed: bool,
}

/// A timer set on a looper.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TimerId(u64);

struct Timer {
    due: Instant,
    id: u64,
    every: Option<Duration>,
}

impl PartialEq for Timer {
    fn eq(&self, other: &Self) -> bool {
        self.due == other.due && self.id == other.id
    }
}
impl Eq for Timer {}
impl PartialOrd for Timer {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Timer {
    // (The heap gives its greatest first: the soonest is the greatest.)
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.due.cmp(&self.due).then(other.id.cmp(&self.id))
    }
}

/// A thread with a queue: BeOS's `BLooper`. What's posted to it runs on
/// its thread, one at a time, in order; with nothing to take it sleeps.
pub struct Looper {
    handle: LooperHandle,
    rx: Receiver<Envelope>,
    deferred: VecDeque<Deferred>,
    slots: Vec<Slot>,
    free: Vec<u32>,
    timers: BinaryHeap<Timer>,
    timer_fns: HashMap<u64, Again>,
    next_timer: u64,
    /// The timer running now, and whether it was cancelled meanwhile (by
    /// itself, most often).
    running_timer: Option<(u64, bool)>,
    /// What the looper's own kit keeps in it (a window's views).
    state: HashMap<TypeId, Box<dyn Any>>,
    /// Run when the queue is dry, before the looper sleeps.
    idle: Vec<Again>,
    last_idle: Instant,
    quit: bool,
}

impl Looper {
    /// A looper for the calling thread (the app's, on the main thread);
    /// [`Self::run`] runs it.
    pub fn new(name: impl Into<String>) -> Looper {
        let (tx, rx) = mpsc::channel();
        let shared = Shared { name: name.into(), tx, thread: thread::current().id(), gone: AtomicBool::new(false), waiting: AtomicU64::new(0) };
        Looper {
            handle: LooperHandle { shared: Arc::new(shared) },
            rx,
            deferred: VecDeque::new(),
            slots: Vec::new(),
            free: Vec::new(),
            timers: BinaryHeap::new(),
            timer_fns: HashMap::new(),
            next_timer: 0,
            running_timer: None,
            state: HashMap::new(),
            idle: Vec::new(),
            last_idle: Instant::now(),
            quit: false,
        }
    }

    /// A looper on a thread of its own, named `name`: `init` runs first
    /// there (it makes what the looper owns, which needn't be `Send`),
    /// then the looper runs until it's told to quit.
    pub fn spawn(name: impl Into<String>, init: impl FnOnce(&mut Looper) + Send + 'static) -> LooperHandle {
        let name = name.into();
        let (handle_tx, handle_rx) = mpsc::sync_channel(1);
        // (The kernel keeps 15 bytes of a thread's name.)
        let thread_name: String = name.chars().take(15).collect();
        let spawned = thread::Builder::new().name(thread_name).spawn(move || {
            let mut looper = Looper::new(name);
            let _ = handle_tx.send(looper.handle());
            init(&mut looper);
            looper.run();
        });
        match spawned {
            Ok(_) => handle_rx.recv().expect("a new looper's thread ended before it began"),
            Err(e) => panic!("no thread for a looper: {e}"),
        }
    }

    pub fn handle(&self) -> LooperHandle {
        self.handle.clone()
    }

    pub fn name(&self) -> &str {
        self.handle.name()
    }

    /// Whether a looper is running on the calling thread.
    pub fn on_looper_thread() -> bool {
        CURRENT.with(|c| c.get()) == Some(thread::current().id())
    }

    /// Ends the loop once the message being taken is done.
    pub fn quit(&mut self) {
        self.quit = true;
    }

    /// Puts `f` off until what the looper is doing now is done: before
    /// anything queued from other threads, after what was put off before
    /// it. How one thing a looper owns tells another without calling into
    /// it while it's busy.
    pub fn defer(&mut self, f: impl FnOnce(&mut Looper) + 'static) {
        self.deferred.push_back(Box::new(f));
    }

    // ---- Handlers ----

    /// Puts `handler` in the looper; its [`Messenger`] is how it's sent
    /// messages.
    pub fn add_handler<H: Handler>(&mut self, handler: H) -> HandlerId {
        let boxed: Box<dyn Any> = Box::new(handler);
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.generation = slot.generation.wrapping_add(1);
            slot.handler = Some(boxed);
            slot.live = true;
            slot.doomed = false;
            HandlerId { index, generation: slot.generation }
        } else {
            self.slots.push(Slot { generation: 0, handler: Some(boxed), live: true, doomed: false });
            HandlerId { index: self.slots.len() as u32 - 1, generation: 0 }
        }
    }

    /// Takes a handler out of the looper (messages still on their way to
    /// it are dropped). Removed from inside its own `message_received`,
    /// it goes when that returns.
    pub fn remove_handler(&mut self, id: HandlerId) {
        let Some(slot) = self.slots.get_mut(id.index as usize) else { return };
        if slot.generation != id.generation || !slot.live {
            return;
        }
        if slot.handler.take().is_some() {
            slot.live = false;
            self.free.push(id.index);
        } else {
            slot.doomed = true;
        }
    }

    /// A messenger to a handler in this looper, for any thread.
    pub fn messenger<H: Handler>(&self, id: HandlerId) -> Messenger<H::Message>
    where
        H::Message: Send,
    {
        Messenger::new(self.handle(), move |looper, message| {
            looper.deliver::<H>(id, message);
        })
    }

    /// Hands `message` to a handler now (the looper's own thread). False
    /// if it's gone, or is the one taking a message already (a handler
    /// can't be sent to from inside itself: `defer` it).
    pub fn deliver<H: Handler>(&mut self, id: HandlerId, message: H::Message) -> bool {
        let Some(slot) = self.slots.get_mut(id.index as usize) else { return false };
        if slot.generation != id.generation || !slot.live {
            return false;
        }
        let Some(mut handler) = slot.handler.take() else { return false };
        let started = Instant::now();
        match handler.downcast_mut::<H>() {
            Some(h) => h.message_received(message, self),
            None => debug_assert!(false, "a message for a handler of another type"),
        }
        self.note_slow(started, std::any::type_name::<H::Message>());
        let slot = &mut self.slots[id.index as usize];
        if slot.doomed {
            slot.doomed = false;
            slot.live = false;
            self.free.push(id.index);
        } else {
            slot.handler = Some(handler);
        }
        true
    }

    /// Reads a handler (not while it's taking a message).
    pub fn handler<H: Handler>(&self, id: HandlerId) -> Option<&H> {
        let slot = self.slots.get(id.index as usize)?;
        if slot.generation != id.generation {
            return None;
        }
        slot.handler.as_ref()?.downcast_ref()
    }

    pub fn handler_mut<H: Handler>(&mut self, id: HandlerId) -> Option<&mut H> {
        let slot = self.slots.get_mut(id.index as usize)?;
        if slot.generation != id.generation {
            return None;
        }
        slot.handler.as_mut()?.downcast_mut()
    }

    fn note_slow(&self, started: Instant, what: &str) {
        if let Some(limit) = slow() {
            let took = started.elapsed();
            if took > limit {
                tracing::warn!(looper = self.name(), took_ms = took.as_secs_f64() * 1000.0, "a slow turn: {what}");
            }
        }
    }

    // ---- What the looper keeps ----

    /// Something kept in the looper, one of each type (a kit's own state:
    /// the windows on this thread).
    pub fn state<T: 'static>(&self) -> Option<&T> {
        self.state.get(&TypeId::of::<T>())?.downcast_ref()
    }

    pub fn state_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.state.get_mut(&TypeId::of::<T>())?.downcast_mut()
    }

    pub fn set_state<T: 'static>(&mut self, value: T) {
        self.state.insert(TypeId::of::<T>(), Box::new(value));
    }

    pub fn take_state<T: 'static>(&mut self) -> Option<T> {
        self.state.remove(&TypeId::of::<T>()).and_then(|b| b.downcast().ok()).map(|b| *b)
    }

    /// Calls `f` whenever the queue is dry, before the looper sleeps
    /// (and at least every few milliseconds while messages keep coming):
    /// where a window lays out and shows what the messages changed, once
    /// for the lot.
    pub fn on_idle(&mut self, f: impl FnMut(&mut Looper) + 'static) {
        self.idle.push(Box::new(f));
    }

    // ---- Timers ----

    /// Runs `f` once, `delay` from now: BeOS's `BMessageRunner` with a
    /// count of one.
    pub fn after(&mut self, delay: Duration, f: impl FnOnce(&mut Looper) + 'static) -> TimerId {
        let mut f = Some(f);
        self.timer(delay, None, move |looper| {
            if let Some(f) = f.take() {
                f(looper)
            }
        })
    }

    /// Runs `f` every `interval` until [`Self::cancel`]led. A turn that
    /// comes late isn't made up for: the next is `interval` after it.
    pub fn every(&mut self, interval: Duration, f: impl FnMut(&mut Looper) + 'static) -> TimerId {
        self.timer(interval, Some(interval), f)
    }

    fn timer(&mut self, delay: Duration, every: Option<Duration>, f: impl FnMut(&mut Looper) + 'static) -> TimerId {
        let id = self.next_timer;
        self.next_timer += 1;
        self.timers.push(Timer { due: Instant::now() + delay, id, every });
        self.timer_fns.insert(id, Box::new(f));
        TimerId(id)
    }

    /// Stops a timer (one that already ran once is gone already).
    pub fn cancel(&mut self, timer: TimerId) {
        // (Its place in the heap is passed over when it comes up.)
        self.timer_fns.remove(&timer.0);
        if let Some((running, cancelled)) = self.running_timer.as_mut()
            && *running == timer.0
        {
            *cancelled = true;
        }
    }

    /// Runs one timer that's due. False if none was.
    fn fire_due(&mut self) -> bool {
        let now = Instant::now();
        while let Some(next) = self.timers.peek() {
            if !self.timer_fns.contains_key(&next.id) {
                self.timers.pop();
                continue;
            }
            if next.due > now {
                return false;
            }
            let Some(timer) = self.timers.pop() else { return false };
            let Some(mut f) = self.timer_fns.remove(&timer.id) else { continue };
            let started = Instant::now();
            self.running_timer = Some((timer.id, false));
            f(self);
            let cancelled = self.running_timer.take().is_some_and(|(_, cancelled)| cancelled);
            self.note_slow(started, "a timer");
            if let Some(every) = timer.every
                && !cancelled
            {
                self.timer_fns.insert(timer.id, f);
                self.timers.push(Timer { due: Instant::now() + every, id: timer.id, every: Some(every) });
            }
            return true;
        }
        false
    }

    fn next_due(&mut self) -> Option<Instant> {
        while let Some(next) = self.timers.peek() {
            if self.timer_fns.contains_key(&next.id) {
                return Some(next.due);
            }
            self.timers.pop();
        }
        None
    }

    // ---- Tasks ----

    /// Runs a future on this looper's thread: it's polled as a message
    /// each time it can go on, between the others. For waiting on
    /// something (an [`crate::Answer`], a [`crate::pool`] task) without
    /// blocking; it needn't be `Send`. Dropping the [`Task`] cancels it;
    /// `detach` lets it finish by itself.
    pub fn spawn_local<T: 'static>(&self, future: impl Future<Output = T> + 'static) -> Task<T> {
        let handle = self.handle();
        let (runnable, task) = async_task::spawn_local(future, move |runnable: async_task::Runnable| {
            handle.post_or_leak(move |_| {
                runnable.run();
            });
        });
        runnable.schedule();
        Task::new(task)
    }

    // ---- The loop ----

    /// Takes messages until told to quit.
    pub fn run(&mut self) {
        CURRENT.with(|c| c.set(Some(thread::current().id())));
        self.last_idle = Instant::now();
        while !self.quit {
            if !self.turn(true) {
                break;
            }
        }
        self.handle.shared.gone.store(true, Ordering::Release);
        CURRENT.with(|c| c.set(None));
    }

    /// Takes everything that's ready and returns without sleeping: for
    /// tests, and for a thread that has a loop of its own to run too.
    pub fn run_until_dry(&mut self) {
        let was = CURRENT.with(|c| c.replace(Some(thread::current().id())));
        while !self.quit && self.turn(false) {}
        CURRENT.with(|c| c.set(was));
    }

    /// One thing done. False when there was nothing to do and it wasn't
    /// to wait (or nothing can come any more).
    fn turn(&mut self, wait: bool) -> bool {
        // What the looper put off for itself goes first: it belongs to
        // the message that was just taken.
        if let Some(f) = self.deferred.pop_front() {
            f(self);
            return true;
        }
        // A flood of messages doesn't keep the window from showing what
        // they changed.
        if !self.idle.is_empty() && self.last_idle.elapsed() > BUSY_FOR {
            self.run_idle();
            if !self.deferred.is_empty() {
                return true;
            }
        }
        if self.fire_due() {
            return true;
        }
        match self.rx.try_recv() {
            Ok(envelope) => {
                self.handle.shared.waiting.fetch_sub(1, Ordering::Relaxed);
                envelope(self);
                return true;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => return false,
        }
        // The queue is dry: show what changed, then sleep.
        self.run_idle();
        if !self.deferred.is_empty() || self.quit {
            return true;
        }
        if !wait {
            return false;
        }
        let envelope = match self.next_due() {
            Some(due) => match self.rx.recv_timeout(due.saturating_duration_since(Instant::now())) {
                Ok(e) => Some(e),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return false,
            },
            None => match self.rx.recv() {
                Ok(e) => Some(e),
                Err(_) => return false,
            },
        };
        // (Awake again: what comes now starts a new busy spell.)
        self.last_idle = Instant::now();
        if let Some(envelope) = envelope {
            self.handle.shared.waiting.fetch_sub(1, Ordering::Relaxed);
            envelope(self);
        }
        true
    }

    fn run_idle(&mut self) {
        self.last_idle = Instant::now();
        if self.idle.is_empty() {
            return;
        }
        let mut idle = std::mem::take(&mut self.idle);
        for f in idle.iter_mut() {
            f(self);
        }
        // (Any added meanwhile go after the ones that were there.)
        idle.append(&mut self.idle);
        self.idle = idle;
        self.last_idle = Instant::now();
    }
}

impl Drop for Looper {
    fn drop(&mut self) {
        self.handle.shared.gone.store(true, Ordering::Release);
    }
}

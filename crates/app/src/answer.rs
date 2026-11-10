//! Asking and answering: a message that wants something back.

use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Waker},
    time::{Duration, Instant},
};

use olo_sync::{Condvar, Mutex};

enum State<T> {
    Waiting(Option<Waker>),
    Answered(T),
    /// Taken, or never coming (the [`Reply`] was dropped unanswered).
    Over,
}

struct Shared<T> {
    state: Mutex<State<T>>,
    answered: Condvar,
}

/// The way to answer: put it in the message that asks (BeOS's reply
/// messenger). Answer once; dropped unanswered, the asker learns there
/// will be none.
pub struct Reply<T> {
    shared: Option<Arc<Shared<T>>>,
}

/// What was asked for, when it comes. A looper awaits it in a task
/// (`Looper::spawn_local`), never blocking; a plain worker thread may
/// [`Answer::wait`] for it.
pub struct Answer<T> {
    shared: Arc<Shared<T>>,
}

/// A question's two ends: send the [`Reply`] off in a message, keep the
/// [`Answer`].
pub fn ask<T>() -> (Reply<T>, Answer<T>) {
    let shared = Arc::new(Shared { state: Mutex::new(State::Waiting(None)), answered: Condvar::new() });
    (Reply { shared: Some(shared.clone()) }, Answer { shared })
}

impl<T> Reply<T> {
    pub fn send(mut self, value: T) {
        if let Some(shared) = self.shared.take() {
            finish(&shared, State::Answered(value));
        }
    }
}

impl<T> Drop for Reply<T> {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.take() {
            finish(&shared, State::Over);
        }
    }
}

fn finish<T>(shared: &Shared<T>, with: State<T>) {
    let waker = {
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        match std::mem::replace(&mut *state, with) {
            State::Waiting(waker) => waker,
            // (Answered already: the first stands.)
            earlier => {
                *state = earlier;
                None
            }
        }
    };
    shared.answered.notify_all();
    if let Some(waker) = waker {
        waker.wake();
    }
}

impl<T> Answer<T> {
    /// Blocks until the answer comes, at most `timeout`. None if there
    /// will be none, or it didn't come in time. Not on a looper's
    /// thread: a looper that waits answers nothing (debug builds panic).
    pub fn wait(self, timeout: Duration) -> Option<T> {
        debug_assert!(!crate::Looper::on_looper_thread(), "a looper must not block on an answer: await it in a task");
        let deadline = Instant::now() + timeout;
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            match std::mem::replace(&mut *state, State::Over) {
                State::Answered(value) => return Some(value),
                State::Over => return None,
                waiting => *state = waiting,
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            state = self.shared.answered.wait_timeout(state, left).unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    /// The answer if it's there already.
    pub fn try_take(&self) -> Option<T> {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        match std::mem::replace(&mut *state, State::Over) {
            State::Answered(value) => Some(value),
            other => {
                *state = other;
                None
            }
        }
    }
}

impl<T> Future for Answer<T> {
    /// None if whoever was asked dropped the question unanswered.
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        match std::mem::replace(&mut *state, State::Over) {
            State::Answered(value) => Poll::Ready(Some(value)),
            State::Over => Poll::Ready(None),
            State::Waiting(_) => {
                *state = State::Waiting(Some(cx.waker().clone()));
                Poll::Pending
            }
        }
    }
}

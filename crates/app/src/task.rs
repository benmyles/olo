//! A task: work going on, and its result when it's done.

use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

/// Work going on somewhere (a looper's own task, or the pool's), and its
/// result for whoever awaits it. Dropped, the work is cancelled at its
/// next step; [`Task::detach`] lets it finish unwatched.
#[must_use = "a task dropped is cancelled: keep it, await it or detach() it"]
pub struct Task<T> {
    inner: Option<async_task::Task<T>>,
}

impl<T> Task<T> {
    pub(crate) fn new(inner: async_task::Task<T>) -> Task<T> {
        Task { inner: Some(inner) }
    }

    /// Lets the work finish by itself, its result thrown away.
    pub fn detach(mut self) {
        if let Some(task) = self.inner.take() {
            task.detach();
        }
    }

    /// Whether it has finished.
    pub fn is_finished(&self) -> bool {
        self.inner.as_ref().is_none_or(|t| t.is_finished())
    }
}

impl<T> Future for Task<T> {
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        match self.inner.as_mut() {
            Some(task) => Pin::new(task).poll(cx),
            None => Poll::Pending,
        }
    }
}

//! What a view has to do at some time, with nothing running while
//! nothing is due (idle costs nothing): one timer for the next thing a
//! view has due ([`Due`]), and a message that clears itself after a
//! while ([`Notice`]: a status line's "Deleted “Bank”.", a banner's
//! problem).

use std::time::{Duration, Instant};

use olo_interface::{Cx, SharedString, View};

/// One timer for the next thing a view has due. Set to when that is
/// ([`Due::at`]), or kept to the soonest of what's asked ([`Due::by`]);
/// a time gone by is no timer.
#[derive(Default)]
pub struct Due(Option<(Instant, olo_app::TimerId)>);

impl Due {
    /// Wake at `at` (None: not at all), calling `then` on the view; the
    /// timer there is kept if it's for the same time.
    pub fn at<V: View>(&mut self, at: Option<Instant>, cx: &mut Cx<V>, then: impl FnOnce(&mut V, &mut Cx<V>) + 'static) {
        if self.when() == at {
            return;
        }
        if let Some((_, timer)) = self.0.take() {
            cx.cancel(timer);
        }
        self.0 = at.map(|t| (t, cx.after_do(t.saturating_duration_since(Instant::now()), then)));
    }

    /// Have the view say what it shows again by `at` (`cx.invalidate`;
    /// its body says what's next), unless it's to wake sooner already.
    pub fn by<V: View>(&mut self, at: Instant, cx: &mut Cx<V>) {
        if self.when().is_some_and(|t| t <= at) {
            return;
        }
        self.at(Some(at), cx, |_, cx| cx.invalidate());
    }

    /// When it wakes next, if it's to.
    pub fn when(&self) -> Option<Instant> {
        self.0.as_ref().map(|(t, _)| *t).filter(|t| *t > Instant::now())
    }
}

/// A message shown for a while, then gone of itself. Its view reads it
/// as it draws ([`Notice::text`]) and wakes when it goes
/// ([`Notice::until`], for its [`Due`]).
#[derive(Debug, Clone, Default)]
pub struct Notice(Option<(SharedString, Instant)>);

impl Notice {
    /// Say `text` for `long`, in place of what it said.
    pub fn say(&mut self, text: impl Into<SharedString>, long: Duration) {
        self.0 = Some((text.into(), Instant::now() + long));
    }

    /// What it says now: None once its time is up.
    pub fn text(&self) -> Option<&SharedString> {
        self.0.as_ref().filter(|(_, until)| Instant::now() < *until).map(|(t, _)| t)
    }

    /// When what it says goes.
    pub fn until(&self) -> Option<Instant> {
        self.text().and(self.0.as_ref().map(|(_, until)| *until))
    }

    /// Whether it says `text` now.
    pub fn says(&self, text: &str) -> bool {
        self.text().is_some_and(|t| t == text)
    }

    pub fn clear(&mut self) {
        self.0 = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_goes_of_itself() {
        let mut n = Notice::default();
        assert_eq!((n.text(), n.until()), (None, None));
        n.say("Saved.", Duration::from_secs(60));
        assert!(n.says("Saved.") && n.until().is_some());
        n.say("Gone already.", Duration::ZERO);
        assert_eq!((n.text(), n.until()), (None, None));
        n.say("Saving…", Duration::from_secs(60));
        n.clear();
        assert!(!n.says("Saving…"));
    }
}

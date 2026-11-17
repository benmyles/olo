//! The phone's calls taken here: when its hands-free side is connected,
//! and what's asked of the sound server's telephony service
//! (`olo_system::phone::calls`).
//!
//! A phone connected hands-free sends every call's sound to the
//! computer, as to a car, wherever the call is answered. So it's
//! connected that way only for a call answered or made *here*: Answer
//! on a ringing call's notification, or Call in Phone, connects it,
//! waits for the telephony service to have the phone (and the call
//! ringing), answers or dials, and when no call is left lets the
//! profile go again. A call answered on the phone itself stays there.
//! A phone that connected hands-free by itself is left as it is.
//!
//! If the phone's hands-free side doesn't come in time, a call being
//! answered is answered on the phone instead (its notification's own
//! action), and one being made isn't.
//!
//! Only what to do is decided here ([`Effect`]); the service does it.

use std::time::{Duration, Instant};

use olo_system::phone::calls::{Call, State};

/// How long the phone's hands-free side is waited for.
pub const HANDS_FREE_WITHIN: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Want {
    Answer,
    Dial(String),
    /// Dialled: its call is to show.
    ItsCall,
}

/// The call to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    /// Its object with the telephony service.
    pub call: String,
    pub number: String,
    /// As the phone names the other end (often nothing).
    pub name: String,
    pub state: State,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Connect the phone's hands-free side, or let it go.
    HandsFree(bool),
    Dial {
        gateway: String,
        number: String,
    },
    Answer(String),
    HangUp(String),
    /// Answer it on the phone instead (its notification's action).
    OnThePhone,
    /// The call as it is now (shown, or shown again: it changed).
    Show(Shown),
    /// No call is left.
    Close,
    /// What was asked couldn't be done.
    Failed(&'static str),
}

#[derive(Debug, Default)]
pub struct Calling {
    /// What was asked for that waits for the phone's hands-free side,
    /// and since when.
    wanted: Option<(Want, Instant)>,
    /// Its hands-free side was connected from here (to let go of when
    /// the calls are over), has been seen connected, and has had a call.
    ours: bool,
    seen: bool,
    had_call: bool,
    shown: Option<Shown>,
}

impl Calling {
    /// The telephony service as it is now for the phone: its gateway if
    /// it's connected hands-free, and its calls.
    pub fn survey(&mut self, gateway: Option<&str>, calls: &[Call], now: Instant) -> Vec<Effect> {
        let mut effects = Vec::new();
        let Some(gateway) = gateway else {
            // (Gone: whatever was ours of it is over.)
            if self.seen {
                (self.ours, self.seen, self.had_call) = (false, false, false);
            }
            self.show(&[], &mut effects);
            return effects;
        };
        self.seen = true;
        match self.wanted.clone() {
            Some((Want::Answer, _)) => {
                if let Some(ringing) = calls.iter().find(|c| c.state.ringing()) {
                    self.wanted = None;
                    effects.push(Effect::Answer(ringing.path.clone()));
                }
            }
            Some((Want::Dial(number), _)) => {
                effects.push(Effect::Dial { gateway: gateway.to_owned(), number });
                // (Its call comes next: not let go of before it has.)
                self.wanted = Some((Want::ItsCall, now));
            }
            Some((Want::ItsCall, _)) | None => {}
        }
        if !calls.is_empty() {
            self.had_call = true;
            // (The call asked for is there.)
            if matches!(&self.wanted, Some((Want::ItsCall, _))) {
                self.wanted = None;
            }
        }
        self.show(calls, &mut effects);
        // The calls are over, on a link made from here for them: let go.
        if self.ours && self.had_call && calls.is_empty() && self.wanted.is_none() {
            (self.ours, self.had_call) = (false, false);
            effects.push(Effect::HandsFree(false));
        }
        effects
    }

    /// The person answers the call ringing.
    pub fn answer(&mut self, gateway: Option<&str>, calls: &[Call], now: Instant) -> Vec<Effect> {
        if let Some(ringing) = calls.iter().find(|c| c.state.ringing()) {
            return vec![Effect::Answer(ringing.path.clone())];
        }
        self.wanted = Some((Want::Answer, now));
        if gateway.is_some() {
            // (Connected already: the call hasn't shown there yet.)
            return Vec::new();
        }
        self.ours = true;
        vec![Effect::HandsFree(true)]
    }

    /// The person calls `number`.
    pub fn dial(&mut self, number: &str, gateway: Option<&str>, now: Instant) -> Vec<Effect> {
        if let Some(gateway) = gateway {
            self.wanted = Some((Want::ItsCall, now));
            return vec![Effect::Dial { gateway: gateway.to_owned(), number: number.to_owned() }];
        }
        self.wanted = Some((Want::Dial(number.to_owned()), now));
        self.ours = true;
        vec![Effect::HandsFree(true)]
    }

    /// The person hangs up (or declines): every call the service has;
    /// with none, what was asked for is no longer.
    pub fn hang_up(&mut self, calls: &[Call]) -> Vec<Effect> {
        let was_waiting = self.wanted.take().is_some();
        let mut effects: Vec<Effect> = calls.iter().map(|c| Effect::HangUp(c.path.clone())).collect();
        if effects.is_empty() && was_waiting && self.ours && !self.had_call {
            self.ours = false;
            effects.push(Effect::HandsFree(false));
        }
        effects
    }

    /// Time passed: the phone's hands-free side (or the call asked for)
    /// never came.
    pub fn tick(&mut self, now: Instant) -> Vec<Effect> {
        let Some((want, since)) = self.wanted.clone() else { return Vec::new() };
        if now.duration_since(since) < HANDS_FREE_WITHIN {
            return Vec::new();
        }
        self.wanted = None;
        let mut effects = vec![match want {
            Want::Answer => Effect::OnThePhone,
            Want::ItsCall => Effect::Failed("The phone didn’t make the call."),
            Want::Dial(_) => Effect::Failed("The phone couldn’t be reached to make the call."),
        }];
        if self.ours && !self.had_call {
            self.ours = false;
            effects.push(Effect::HandsFree(false));
        }
        effects
    }

    pub fn due(&self) -> Option<Instant> {
        self.wanted.as_ref().map(|(_, since)| *since + HANDS_FREE_WITHIN)
    }

    /// The call to show: one ringing first, else one that isn't held.
    fn show(&mut self, calls: &[Call], effects: &mut Vec<Effect>) {
        let call = calls.iter().find(|c| c.state.ringing()).or_else(|| calls.iter().find(|c| c.state != State::Held)).or(calls.first());
        let now = call.map(|c| Shown { call: c.path.clone(), number: c.number.clone(), name: c.name.clone(), state: c.state });
        if now != self.shown {
            effects.push(match &now {
                Some(shown) => Effect::Show(shown.clone()),
                None => Effect::Close,
            });
            self.shown = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GATEWAY: &str = "/org/pipewire/Telephony/ag0";

    fn call(n: u32, state: State) -> Call {
        Call { path: format!("{GATEWAY}/call{n}"), number: "+15550100142".into(), name: String::new(), state }
    }

    fn shown(n: u32, state: State) -> Effect {
        Effect::Show(Shown { call: format!("{GATEWAY}/call{n}"), number: "+15550100142".into(), name: String::new(), state })
    }

    /// Answered here: the hands-free side is connected, the ringing
    /// call answered when the service has it, and the link let go when
    /// the call is over.
    #[test]
    fn a_call_answered_here() {
        let (mut c, now) = (Calling::default(), Instant::now());
        assert_eq!(c.answer(None, &[], now), [Effect::HandsFree(true)]);
        // (The phone connected, its call not yet known.)
        assert_eq!(c.survey(Some(GATEWAY), &[], now), []);
        let ringing = [call(1, State::Incoming)];
        assert_eq!(c.survey(Some(GATEWAY), &ringing, now), [Effect::Answer(ringing[0].path.clone()), shown(1, State::Incoming)]);
        assert_eq!(c.survey(Some(GATEWAY), &[call(1, State::Active)], now), [shown(1, State::Active)]);
        assert_eq!(c.survey(Some(GATEWAY), &[call(1, State::Active)], now), [], "nothing new");
        assert_eq!(c.hang_up(&[call(1, State::Active)]), [Effect::HangUp(format!("{GATEWAY}/call1"))]);
        assert_eq!(c.survey(Some(GATEWAY), &[], now), [Effect::Close, Effect::HandsFree(false)]);
        assert_eq!(c.survey(None, &[], now), []);
        assert_eq!(c.due(), None);
    }

    /// A phone that connected hands-free by itself: its calls are
    /// shown and answered, and it's left connected.
    #[test]
    fn a_phone_connected_by_itself() {
        let (mut c, now) = (Calling::default(), Instant::now());
        let ringing = [call(1, State::Incoming)];
        assert_eq!(c.survey(Some(GATEWAY), &ringing, now), [shown(1, State::Incoming)]);
        assert_eq!(c.answer(Some(GATEWAY), &ringing, now), [Effect::Answer(ringing[0].path.clone())]);
        assert_eq!(c.survey(Some(GATEWAY), &[call(1, State::Active)], now), [shown(1, State::Active)]);
        assert_eq!(c.survey(Some(GATEWAY), &[], now), [Effect::Close], "not let go: it wasn't connected from here");
        // (Declined: hung up while ringing.)
        c.survey(Some(GATEWAY), &ringing, now);
        assert_eq!(c.hang_up(&ringing), [Effect::HangUp(ringing[0].path.clone())]);
    }

    /// A call made here: connected, dialled when the service has the
    /// phone, let go when it's over.
    #[test]
    fn a_call_made_here() {
        let (mut c, now) = (Calling::default(), Instant::now());
        assert_eq!(c.dial("+15550100142", None, now), [Effect::HandsFree(true)]);
        assert_eq!(c.survey(Some(GATEWAY), &[], now), [Effect::Dial { gateway: GATEWAY.into(), number: "+15550100142".into() }]);
        // (Not let go of while its call is still to come.)
        assert_eq!(c.survey(Some(GATEWAY), &[], now), []);
        assert_eq!(c.survey(Some(GATEWAY), &[call(1, State::Dialing)], now), [shown(1, State::Dialing)]);
        assert_eq!(c.survey(Some(GATEWAY), &[call(1, State::Alerting)], now), [shown(1, State::Alerting)]);
        assert_eq!(c.survey(Some(GATEWAY), &[call(1, State::Active)], now), [shown(1, State::Active)]);
        assert_eq!(c.survey(Some(GATEWAY), &[], now), [Effect::Close, Effect::HandsFree(false)]);
        // With the phone connected already: dialled at once.
        let mut c = Calling::default();
        c.survey(Some(GATEWAY), &[], now);
        assert_eq!(c.dial("+15550100190", Some(GATEWAY), now), [Effect::Dial { gateway: GATEWAY.into(), number: "+15550100190".into() }]);
    }

    /// The phone's hands-free side never comes: a call being answered
    /// is answered on the phone, one being made isn't, and what was
    /// asked of BlueZ is taken back.
    #[test]
    fn the_hands_free_side_never_comes() {
        let (mut c, now) = (Calling::default(), Instant::now());
        c.answer(None, &[], now);
        assert_eq!(c.due(), Some(now + HANDS_FREE_WITHIN));
        assert_eq!(c.tick(now + Duration::from_secs(2)), []);
        assert_eq!(c.tick(now + HANDS_FREE_WITHIN), [Effect::OnThePhone, Effect::HandsFree(false)]);
        assert_eq!(c.tick(now + HANDS_FREE_WITHIN), []);
        c.dial("+15550100142", None, now);
        assert_eq!(c.tick(now + HANDS_FREE_WITHIN), [Effect::Failed("The phone couldn’t be reached to make the call."), Effect::HandsFree(false)]);
        // Connected, dialled, and no call ever came of it.
        c.dial("+15550100142", None, now);
        c.survey(Some(GATEWAY), &[], now);
        assert_eq!(c.tick(now + HANDS_FREE_WITHIN), [Effect::Failed("The phone didn’t make the call."), Effect::HandsFree(false)]);
        // Given up on by the person first.
        c.answer(None, &[], now);
        assert_eq!(c.hang_up(&[]), [Effect::HandsFree(false)]);
        assert_eq!(c.due(), None);
    }
}

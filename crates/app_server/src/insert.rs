//! Text put into the focused field as an input method would
//! (`Request::InsertText`): the emoji picker's choice, or an agent's text
//! that no keyboard layout types. A field that speaks text-input-v3 takes
//! it whole (`commit_string`), app_server acting as the input method just
//! until it has; one that doesn't (a terminal) gets it as keys, through a
//! keymap made for the text (`inject_text_keysyms`).
//!
//! The asking program usually has the keyboard as it asks (the picker's
//! panel, on its way out), so the text waits, a second at most, for the
//! keyboard to go back to someone else. The reply comes once the text is
//! in, so whatever the asker does next (an agent pressing Return) comes
//! after it.

use std::{rc::Rc, time::Duration};

use olo_protocol::Reply;

use smithay::{
    input::keyboard::xkb,
    reexports::{
        calloop::{
            RegistrationToken,
            timer::{TimeoutAction, Timer},
        },
        wayland_server::Resource,
    },
    wayland::{input_method::InputMethodSeat, seat::WaylandFocus, text_input::TextInputSeat},
};
use tracing::info;

use crate::{
    focus::KeyboardFocusTarget,
    ipc::Conn,
    state::{AnvilState, Backend, client_pid},
};

/// How long the asking program's own surface may keep the keyboard.
const HANDBACK: Duration = Duration::from_secs(1);
/// How long a focused window has to turn its field on once app_server
/// offers to be its input method, before the text goes in as keys.
const TURN_ON: Duration = Duration::from_millis(150);

#[derive(Default)]
pub struct Inserting {
    text: Option<String>,
    /// Who asked: told once the text is in. Its own surfaces don't take
    /// the text.
    conn: Option<Rc<Conn>>,
    from: Option<i32>,
    /// Offered to be the focused window's input method, waiting for its
    /// field to turn on (`text_input_on`).
    offered: bool,
    timer: Option<RegistrationToken>,
}

impl std::fmt::Debug for Inserting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inserting").field("text", &self.text).field("from", &self.from).field("offered", &self.offered).finish()
    }
}

impl<B: Backend> AnvilState<B> {
    /// Put `text` into the focused field, for `conn`: the reply once it's
    /// in (None: it comes later).
    pub fn insert_text(&mut self, text: String, conn: &Rc<Conn>) -> Option<Reply> {
        if self.lock.locked() {
            return Some(Reply::Error { message: "The screen is locked.".into() });
        }
        if text.is_empty() {
            return Some(Reply::Ok);
        }
        self.insert_done(Err("Other text came to put in first.".into()));
        self.inserting.text = Some(text);
        self.inserting.from = conn.peer_pid();
        self.inserting.conn = Some(conn.clone());
        self.insert_wait(HANDBACK);
        self.insert_try();
        None
    }

    /// Whether text waits for the keyboard to go somewhere.
    pub fn insert_waiting(&self) -> bool {
        self.inserting.text.is_some() && !self.inserting.offered
    }

    /// The keyboard went somewhere: the text waiting goes there, unless
    /// it's still the asker's.
    pub fn insert_try(&mut self) {
        if self.inserting.text.is_none() || self.inserting.offered {
            return;
        }
        let Some(focus) = self.seat.get_keyboard().and_then(|k| k.current_focus()) else {
            return;
        };
        // Never into the lock screen.
        if matches!(focus, KeyboardFocusTarget::Surface(_)) {
            return;
        }
        let theirs = focus
            .wl_surface()
            .and_then(|s| s.client())
            .and_then(|c| client_pid(&c, &self.display_handle));
        if theirs.is_some() && theirs == self.inserting.from {
            return;
        }
        let input = self.seat.text_input().clone();
        if self.commit_text(&input) {
            return;
        }
        if self.seat.input_method().has_instance() {
            // A real input method, and the field didn't turn on for it:
            // not a field that takes text that way.
            self.insert_keys();
            return;
        }
        self.inserting.offered = true;
        input.set_compositor_input_method(true);
        self.insert_wait(TURN_ON);
    }

    /// A field turned on (smithay's `TextInputActivation::activated`).
    pub fn text_input_on(&mut self) {
        if !self.inserting.offered {
            return;
        }
        let input = self.seat.text_input().clone();
        self.commit_text(&input);
    }

    /// Commit the text into the field that's on, if one is.
    fn commit_text(&mut self, input: &smithay::wayland::text_input::TextInputHandle) -> bool {
        let Some(text) = self.inserting.text.clone() else { return false };
        let mut sent = false;
        input.with_active_text_input(|field, _| {
            field.commit_string(Some(text.clone()));
            sent = true;
        });
        if !sent {
            return false;
        }
        input.done(false);
        info!(chars = text.chars().count(), "text put in a field");
        self.insert_done(Ok(()));
        true
    }

    /// The window has no field that turns on: type the text, a key for
    /// each character on a keymap of its own.
    fn insert_keys(&mut self) {
        let Some(text) = self.inserting.text.clone() else { return };
        let Some(keyboard) = self.seat.get_keyboard() else { return };
        let keysyms: Vec<_> = text
            .chars()
            .map(|c| match c {
                '\n' => xkb::Keysym::Return,
                '\t' => xkb::Keysym::Tab,
                c => xkb::utf32_to_keysym(c as u32),
            })
            .filter(|k| k.raw() != 0)
            .collect();
        // Off as the input method first: the field mustn't turn on midway.
        self.insert_stop_offering();
        info!(chars = keysyms.len(), "text typed as keys");
        keyboard.inject_text_keysyms(self, &keysyms);
        self.insert_done(Ok(()));
    }

    /// Wake after `after`: if a field hasn't taken the text by then, it
    /// goes in as keys (or, still at the asker, nowhere).
    fn insert_wait(&mut self, after: Duration) {
        if let Some(t) = self.inserting.timer.take() {
            self.handle.remove(t);
        }
        self.inserting.timer = self
            .handle
            .insert_source(Timer::from_duration(after), |_, _, state| {
                state.inserting.timer = None;
                if state.inserting.offered {
                    state.insert_keys();
                } else if state.inserting.text.is_some() {
                    info!("text to put in a field dropped: the keyboard stayed with whoever asked");
                    state.insert_done(Err("Nothing else took the keyboard.".into()));
                }
                TimeoutAction::Drop
            })
            .ok();
    }

    /// Nothing waiting any more: the asker told how it went (if it still
    /// waits), and app_server stops being the input method.
    fn insert_done(&mut self, result: Result<(), String>) {
        if let Some(t) = self.inserting.timer.take() {
            self.handle.remove(t);
        }
        self.insert_stop_offering();
        let was = std::mem::take(&mut self.inserting);
        if let (Some(conn), Some(_)) = (was.conn, was.text) {
            conn.send(&match result {
                Ok(()) => Reply::Ok,
                Err(message) => Reply::Error { message },
            });
        }
    }

    fn insert_stop_offering(&mut self) {
        if std::mem::take(&mut self.inserting.offered) {
            self.seat.text_input().set_compositor_input_method(false);
        }
    }
}

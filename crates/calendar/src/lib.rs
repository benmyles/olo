//! Calendar's engine (apps/calendar is the app; Ben, 2026-11-17: "build a
//! beautiful olo native calendar app … any number of google calendars …
//! as well as any number of fastmail calendars … aggregate all the
//! connected calendar sources into one unified calendar view, and also
//! support read and write actions").
//!
//! - [`ical`]: iCalendar as text: content lines, their folding and
//!   escapes, the tree of components; read whole and written back whole,
//!   so what Olo doesn't know of an event survives its changing one.
//! - [`rrule`]: how an event repeats: the rule read, written, and worked
//!   out into its starts (the RFC's own examples are its tests).
//! - [`event`]: an event as a calendar shows and changes it, read from a
//!   `VEVENT` and written back into it, and its occurrences between two
//!   moments: time zones, all-day events, dates taken out, one
//!   occurrence changed by itself.
//! - [`find`]: events found by their words from outside Calendar (the
//!   command palette): each once, at its next occurrence or its last.
//! - [`dav`]: CalDAV, the one protocol for every account (Fastmail's
//!   with an app password, Google's with its sign-in's token): an
//!   account's calendars, what is in one, what changed since a token,
//!   and an event put, changed or removed, never over a change made
//!   somewhere else meanwhile. [`standin`]: a stand-in server, for tests
//!   and for trying the app with no account.
//! - [`account`]: the accounts set up (Google's, Fastmail's, any other
//!   CalDAV server's) and what signs each in, kept in the login keyring.
//! - [`store`]: calendars kept on disk as they were when last in step
//!   with their servers, brought in step by asking only for what
//!   changed; what a person changes goes to the server first, then here.
//!
//! Times are `jiff`'s: the system's time zone database, the clock's
//! changes handled where a wall time meets them.

pub mod account;
pub mod dav;
pub mod event;
pub mod find;
pub mod ical;
pub mod rrule;
pub mod standin;
pub mod store;

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
//!
//! Times are `jiff`'s: the system's time zone database, the clock's
//! changes handled where a wall time meets them.

pub mod event;
pub mod ical;
pub mod rrule;

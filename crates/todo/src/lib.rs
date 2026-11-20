//! To-Do's engine (apps/todo is the app; Ben, 2026-11-18: "a beautiful
//! olo native todo app that supports reminders / due dates / simple
//! kanban style board / attaching links, images, etc. / beautiful styled
//! deskbar widgets").
//!
//! - [`item`]: a to-do as To-Do shows and changes it, read from an
//!   iCalendar `VTODO` and written back into it whole (what another
//!   program put there stays): its title, notes, due, reminder, section,
//!   place, attachments, and whether it's done.
//! - [`store`]: the lists on disk, a folder each, a file a to-do (a
//!   vdir: other programs read and sync them as they are), read whole
//!   once and then only what changed; changes written whole or not at
//!   all, and what they replaced handed back for undo.
//! - [`quick`]: a to-do typed on one line ("Call Ann tomorrow at 3pm"):
//!   its title and its due.
//! - [`view`]: what each of the window's lists shows, in order (Today,
//!   Upcoming, All, Done, and a list's own by its sections), and where a
//!   to-do moved goes.
//! - [`demo`]: made-up lists, for pictures and the layout lint.
//!
//! Times are `jiff`'s, as Calendar's engine has them (`olo_calendar`,
//! whose iCalendar this reads and writes).

pub mod demo;
pub mod item;
pub mod quick;
pub mod store;
pub mod view;

pub use item::{Attachment, Remind, Status, ToDo};
pub use store::{Book, Item, List};

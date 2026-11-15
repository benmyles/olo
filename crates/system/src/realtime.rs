//! Realtime threads, guarded: what the person is waiting on runs before
//! anything else wanting the processor, under any scheduler and any load
//! (Ben, 2026-11-15: "nothing should ever be able to impact the
//! responsiveness of olo"; "any audio i/o and other stuff like that
//! should be realtime too").
//!
//! A nice isn't enough: the CPU is shared between cgroups before nice
//! counts (each app is a scope of its own), and a scheduler loaded over
//! the kernel's (scx_lavd) gave nice little weight. Under other programs'
//! builds and a GPU job, app_server's loop at nice −10 waited on a run
//! queue 100–300 ms of every second and the display stalled for up to
//! 2.5 s; made realtime, it waited 0 ms in a minute of the same load. The
//! realtime class is above every normal and sched_ext one.
//!
//! SCHED_RR, set by the program itself where the account lets it (Olo
//! Doctor's Allow Realtime), each thread registered with the guard in
//! `olo_app::realtime` first: never put back for running long, its
//! program ended only when it's stuck (30 s without once sleeping), with
//! a report. Never through rtkit, whose rule ends a program at 200 ms.
//! Threads a realtime one starts are normal (SCHED_RESET_ON_FORK).
//! [`demote_self`] puts one back for good (a window that lost the
//! keyboard).

pub use olo_xdg::realtime::*;

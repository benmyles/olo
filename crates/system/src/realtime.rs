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
//! Through rtkit (`priority::make_realtime`), SCHED_RR, each thread
//! registered with the guard in `olo_app::realtime` first (a soft limit of
//! 50 ms of realtime CPU without sleeping, where SIGXCPU puts the threads
//! back to their nice, under rtkit's hard 200 ms that would end the
//! process). Here a keeper thread, asleep on a pipe the guard's handler
//! writes to, makes them realtime again [`AGAIN_AFTER`] later (and logs
//! it). Nothing runs meanwhile: the keeper is blocked in a read.
//!
//! Threads a realtime one starts are normal (rtkit sets
//! SCHED_RESET_ON_FORK). [`demote_self`] puts one back for good (a window
//! that lost the keyboard).

pub use olo_xdg::realtime::*;

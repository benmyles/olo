//! A tiled edge drag shown together (`tiling::TileDrag`). Each step of
//! the drag is a round: the dragged window and every neighbour following
//! it are asked their new sizes at once, and what each draws for them is
//! held back (a commit blocker, `dmabuf_check::Gate`) until every one of
//! them has drawn it, or `LIMIT` has passed, so they land in the same
//! frame: no window over another, no gap between them. The next round
//! starts once one is shown, from where the drag has got to by then, so
//! the tiles go as fast as the slowest of them draws (at the display's
//! rate, when they keep up), together.

use std::time::{Duration, Instant};

use smithay::{
    desktop::WindowSurface,
    reexports::{
        calloop::timer::{TimeoutAction, Timer},
        wayland_server::{Resource, protocol::wl_surface::WlSurface},
    },
    utils::{IsAlive, Logical, Point, Serial, Size},
    wayland::{
        compositor::{BufferAssignment, CompositorHandler, SurfaceAttributes, add_blocker, with_states},
        shell::xdg::XdgToplevelSurfaceData,
    },
};

use tracing::{debug, info};

use super::WindowElement;
use crate::{
    dmabuf_check::Gate,
    state::{AnvilState, Backend},
};

/// The longest a round waits for its slowest window: one that hasn't
/// drawn by then lands when it does, and the rest don't wait for it.
const LIMIT: Duration = Duration::from_millis(50);

/// One step of a drag on its way to the screen.
pub struct Round {
    /// Which round (the timer that ends it checks it's still this one).
    number: u64,
    members: Vec<Member>,
    /// Windows it only moves (drawn at their new size already): moved as
    /// it shows, with the rest, not on their own before.
    moves: Vec<(WindowElement, Point<i32, Logical>)>,
    gate: Gate,
    began: Instant,
}

/// A window in a round.
struct Member {
    surface: WlSurface,
    /// The configure that asked it, the size it asked (None: its own),
    /// and what it showed then (`drawn_part`).
    serial: Serial,
    asked: Option<Size<i32, Logical>>,
    was: Option<(i64, i64)>,
    /// What it drew for it has come (and waits), and how many things it
    /// waits for besides (the GPU drawing it, its buffer's check:
    /// `round_fenced`): it's ready to show at none.
    drawn: bool,
    waits: u32,
}

/// What a surface shows of its buffers, as committed (`pending`: as the
/// commit on its way has it, None if it brings no buffer): the part a
/// viewport crops (the kit's apps draw into part of a bigger buffer,
/// `wp_viewport`'s source, set as each frame is drawn), else the whole
/// buffer, in buffer pixels. A frame drawn at a new size has another.
fn drawn_part(surface: &WlSurface, pending: bool) -> Option<(i64, i64)> {
    use smithay::wayland::viewporter::ViewportCachedState;
    let (buffer, src) = with_states(surface, |states| {
        let mut attributes = states.cached_state.get::<SurfaceAttributes>();
        let a = if pending { attributes.pending() } else { attributes.current() };
        let buffer = match &a.buffer {
            Some(BufferAssignment::NewBuffer(b)) => Some(b.clone()),
            _ => None,
        };
        let mut viewport = states.cached_state.get::<ViewportCachedState>();
        let src = if pending { viewport.pending().src } else { viewport.current().src };
        (buffer, src)
    });
    if pending && buffer.is_none() {
        return None;
    }
    if let Some(src) = src {
        return Some(((src.size.w * 64.0).round() as i64, (src.size.h * 64.0).round() as i64));
    }
    let size = match buffer {
        Some(b) => smithay::backend::renderer::buffer_dimensions(&b)?,
        // (What it shows now: the buffer itself is the renderer's by then.)
        None => smithay::backend::renderer::utils::with_renderer_surface_state(surface, |s| s.buffer_size().map(|l| l.to_buffer(s.buffer_scale(), s.buffer_transform()))).flatten()?,
    };
    Some((size.w as i64 * 64, size.h as i64 * 64))
}

/// The size (logical) of what the commit on its way shows, if it brings
/// a buffer: the viewport's source at the surface's scale (the kit's
/// apps draw that part of a bigger buffer, a buffer pixel to a device
/// pixel), else the whole buffer at its scale.
fn coming_size(surface: &WlSurface) -> Option<(f64, f64)> {
    use smithay::wayland::{fractional_scale::with_fractional_scale, viewporter::ViewportCachedState};
    let (buffer, src, buffer_scale, scale) = with_states(surface, |states| {
        let mut attributes = states.cached_state.get::<SurfaceAttributes>();
        let a = attributes.pending();
        let buffer = match &a.buffer {
            Some(BufferAssignment::NewBuffer(b)) => Some(b.clone()),
            _ => None,
        };
        let buffer_scale = a.buffer_scale.max(1);
        let src = states.cached_state.get::<ViewportCachedState>().pending().src;
        let scale = with_fractional_scale(states, |f| f.preferred_scale()).unwrap_or(1.0).max(0.1);
        (buffer, src, buffer_scale, scale)
    });
    let buffer = buffer?;
    if let Some(src) = src {
        return Some((src.size.w / scale, src.size.h / scale));
    }
    let d = smithay::backend::renderer::buffer_dimensions(&buffer)?;
    Some((d.w as f64 / buffer_scale as f64, d.h as f64 / buffer_scale as f64))
}

impl std::fmt::Debug for Round {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let drawn = self.members.iter().filter(|m| m.drawn).count();
        write!(f, "Round {} ({drawn} of {} drawn, {:?} on)", self.number, self.members.len(), self.began.elapsed())
    }
}

impl<BackendData: Backend + 'static> AnvilState<BackendData> {
    /// A round in progress (asked, not all shown yet).
    pub fn in_round(&self) -> bool {
        self.round.is_some()
    }

    /// `w` is in the round on its way, or the last one to come: its size
    /// goes with the round, not on its own.
    pub fn round_has(&self, w: &WindowElement) -> bool {
        let Some(surface) = w.wl_surface() else { return false };
        self.round.as_ref().is_some_and(|r| r.members.iter().any(|m| m.surface == *surface)) || self.round_then.iter().any(|o| o == w)
    }

    /// A drag's last round (where it was let go): now, or once the one on
    /// its way has shown.
    pub fn round_last(&mut self, windows: Vec<WindowElement>) {
        if self.in_round() {
            self.round_then = windows;
        } else {
            self.round_send(&windows);
        }
    }

    /// `w` goes to `loc` (its element's location) with the drag's last
    /// round, as it's being made (`tile_drag_end`): it moves as the round
    /// shows, with the windows changing size, not a frame or more before.
    pub fn round_move(&mut self, w: &WindowElement, loc: Point<i32, Logical>) {
        self.round_moves.retain(|(m, _)| m != w);
        self.round_moves.push((w.clone(), loc));
    }

    /// Send `windows` the sizes waiting for them (their pending state), as
    /// one round, with the moves waiting (`round_move`). Windows with
    /// nothing new aren't in it; with none, there is no round (the moves
    /// are made now).
    pub fn round_send(&mut self, windows: &[WindowElement]) {
        let mut members = Vec::new();
        for w in windows {
            let WindowSurface::Wayland(t) = w.0.underlying_surface();
            if !t.is_initial_configure_sent() {
                continue;
            }
            let was = drawn_part(t.wl_surface(), false);
            // (Asked no new size, a state only, it has nothing to draw for
            // the round: sent, but not waited for.)
            let asked = t.with_pending_state(|s| s.size);
            let had = with_states(t.wl_surface(), |states| states.data_map.get::<XdgToplevelSurfaceData>().and_then(|d| d.lock().ok()?.last_acked.as_ref().map(|c| c.state.size)));
            if let Some(serial) = t.send_pending_configure()
                && had != Some(asked)
            {
                members.push(Member { surface: t.wl_surface().clone(), serial, asked, was, drawn: false, waits: 0 });
            }
        }
        let moves = std::mem::take(&mut self.round_moves);
        if members.is_empty() {
            self.round_moved(moves);
            return;
        }
        self.rounds += 1;
        let number = self.rounds;
        debug!(target: "olo::rounds", number, members = ?members.iter().map(|m| (self.window_for_surface(&m.surface).map(|w| w.id()), m.surface.id(), m.asked.map(|a| (a.w, a.h)))).collect::<Vec<_>>(), moves = moves.len(), "round sent");
        self.round = Some(Round { number, members, moves, gate: Gate::default(), began: Instant::now() });
        let _ = self.handle.insert_source(Timer::from_duration(LIMIT), move |_, _, state| {
            if state.round.as_ref().is_some_and(|r| r.number == number) {
                state.round_shown();
            }
            TimeoutAction::Drop
        });
    }

    /// A surface is about to commit (`new_surface`'s hook): if it's a
    /// window of the round, drawing what the round asked of it, the
    /// commit waits for the others' (the round's number then). The last
    /// one's shows them all.
    pub fn round_commit(&mut self, surface: &WlSurface) -> Option<u64> {
        let round = self.round.as_mut()?;
        let member = round.members.iter_mut().find(|m| m.surface == *surface && !m.drawn)?;
        // (The role's: Smithay's own hook, after this one, hands it to the
        // commit.)
        let acked = with_states(surface, |states| states.data_map.get::<XdgToplevelSurfaceData>().and_then(|d| d.lock().ok()?.last_acked.as_ref().map(|c| c.serial)));
        if !acked.is_some_and(|a| a.is_no_older_than(&member.serial)) {
            return None;
        }
        // Drawn at the size asked: not answered with a frame of a size
        // before (the kit's apps may answer with one drawn already, the
        // new one to come a frame later; their window geometry is the new
        // size at once), which would leave it a step behind the others.
        // That one shows as it comes. (Asked to choose its own size: any
        // frame of another than it showed. An app that won't be the size
        // asked holds the round its `LIMIT`.)
        let drawn = match member.asked {
            Some(a) => coming_size(surface).is_some_and(|(w, h)| (w - a.w as f64).abs() <= 1.0 && (h - a.h as f64).abs() <= 1.0),
            None => drawn_part(surface, true).is_some_and(|now| Some(now) != member.was),
        };
        if !drawn {
            debug!(target: "olo::rounds", number = round.number, surface = ?surface.id(), asked = ?member.asked, coming = ?coming_size(surface), "a round's window drew another size");
            return None;
        }
        member.drawn = true;
        if let Some(client) = surface.client() {
            round.gate.hold(client);
            add_blocker(surface, round.gate.clone());
        }
        // (Let go once this commit is done waiting: not from inside it, and
        // once its fence, if it has one, says the GPU has drawn it.)
        let number = round.number;
        if round.members.iter().all(|m| m.drawn) {
            self.handle.insert_idle(|state| state.round_try_show());
        }
        Some(number)
    }

    /// The commit `round_commit` took waits for something else too: its
    /// GPU work (a fence), or its buffer's check (`dmabuf_check::Gate`),
    /// each a `round_ready` when it's done. Shown with the others' before
    /// that, its frame would come after them, a step or more behind.
    pub fn round_fenced(&mut self, surface: &WlSurface) {
        if let Some(m) = self.round.as_mut().and_then(|r| r.members.iter_mut().find(|m| m.surface == *surface)) {
            m.waits += 1;
        }
    }

    /// One of what round `number`'s commit of `surface` waits for is done.
    pub fn round_ready(&mut self, surface: &WlSurface, number: u64) {
        let Some(round) = self.round.as_mut().filter(|r| r.number == number) else { return };
        if let Some(m) = round.members.iter_mut().find(|m| m.surface == *surface) {
            m.waits = m.waits.saturating_sub(1);
        }
        self.round_try_show();
    }

    /// The round shown if every window has drawn it and is ready.
    fn round_try_show(&mut self) {
        if self.round.as_ref().is_some_and(|r| r.members.iter().all(|m| m.drawn && m.waits == 0)) {
            self.round_shown();
        }
    }

    /// The round is over (all drawn, or out of time): what waited shows
    /// together, and the drag goes on from where it has got to.
    pub fn round_shown(&mut self) {
        let Some(round) = self.round.take() else { return };
        let dh = self.display_handle.clone();
        for client in round.gate.open() {
            self.client_compositor_state(&client).blocker_cleared(self, &dh);
        }
        debug!(target: "olo::rounds", number = round.number, took = ?round.began.elapsed(), "round shown");
        self.round_moved(round.moves);
        // (In `timings`' phases: how long rounds take, and how many ran
        // out of time; and which windows didn't keep up, in the log.)
        let all = round.members.iter().all(|m| m.drawn);
        self.timings.add(if all { "drag round" } else { "drag round (out of time)" }, round.began.elapsed());
        if !all {
            let late: Vec<u64> = round.members.iter().filter(|m| !m.drawn).filter_map(|m| Some(self.window_for_surface(&m.surface)?.id())).collect();
            info!(round = round.number, ?late, "a drag round ran out of time");
        }
        self.backend_data.wake(None);
        let then = std::mem::take(&mut self.round_then);
        if then.is_empty() {
            self.tile_drag_next();
        } else {
            self.round_send(&then);
        }
    }

    fn round_moved(&mut self, moves: Vec<(WindowElement, Point<i32, Logical>)>) {
        for (w, loc) in moves {
            if w.alive() && self.space.element_location(&w).is_some_and(|at| at != loc) {
                self.space.relocate_element(&w, loc);
            }
        }
    }
}

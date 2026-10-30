use std::cell::RefCell;


#[cfg(feature = "udev")]
use smithay::wayland::drm_syncobj::DrmSyncobjCachedState;

use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    desktop::{
        LayerSurface, PopupKind, PopupManager, Space, WindowSurfaceType, layer_map_for_output,
        space::SpaceElement,
    },
    input::pointer::{CursorImageStatus, CursorImageSurfaceData},
    output::Output,
    reexports::{
        calloop::Interest,
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            Client, Resource,
            protocol::{wl_buffer::WlBuffer, wl_output, wl_surface::WlSurface},
        },
    },
    utils::{IsAlive, Logical, Point, Rectangle, Size},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            BufferAssignment, CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes,
            TraversalAction, add_blocker, add_pre_commit_hook, get_parent, is_sync_subsurface, with_states,
            with_surface_tree_upward,
        },
        dmabuf::get_dmabuf,
        shell::{
            wlr_layer::{
                Layer, LayerSurface as WlrLayerSurface, LayerSurfaceData, WlrLayerShellHandler,
                WlrLayerShellState,
            },
            xdg::{SurfaceCachedState, XdgToplevelSurfaceData},
        },
    },
};

use crate::{
    ClientState,
    state::{AnvilState, Backend},
};

mod autofit;
pub mod autotile;
mod element;
mod grabs;
pub mod deco;
pub mod groups;
pub mod modes;
pub mod tabdrag;
pub mod tiling;
mod wm;
mod xdg;

pub(crate) use self::deco::fill;
pub use self::element::*;
pub use self::grabs::*;

/// The menus of the Background and Bottom layers (the desktop's), each
/// with where it is on `output`, front first. Like any menu they go over
/// the windows and the Top layer (Deskbar), under only the Overlay layer,
/// not under the windows with their surface, where smithay would put
/// them: render.rs draws them there, and `surface_under` finds them there.
pub fn lower_layer_menus(output: &Output) -> Vec<(WlSurface, Point<i32, Logical>)> {
    let map = layer_map_for_output(output);
    map.layers()
        .rev()
        .filter(|l| matches!(l.layer(), Layer::Background | Layer::Bottom))
        .filter_map(|l| map.layer_geometry(l).map(|g| (l, g.loc)))
        .flat_map(|(l, at)| {
            PopupManager::popups_for_surface(l.wl_surface())
                .map(move |(p, offset)| (p.wl_surface().clone(), at + offset - p.geometry().loc))
        })
        .collect()
}

fn fullscreen_output_geometry(
    wl_surface: &WlSurface,
    wl_output: Option<&wl_output::WlOutput>,
    space: &mut Space<WindowElement>,
) -> Option<Rectangle<i32, Logical>> {
    // First test if a specific output has been requested
    // if the requested output is not found ignore the request
    wl_output
        .and_then(Output::from_resource)
        .or_else(|| {
            let w = space
                .elements()
                .find(|window| window.wl_surface().map(|s| &*s == wl_surface).unwrap_or(false));
            w.and_then(|w| space.outputs_for_element(w).first().cloned())
        })
        .as_ref()
        .and_then(|o| space.output_geometry(o))
}

#[derive(Default)]
pub struct FullscreenSurface(RefCell<Option<WindowElement>>);

impl FullscreenSurface {
    pub fn set(&self, window: WindowElement) {
        *self.0.borrow_mut() = Some(window);
    }

    pub fn get(&self) -> Option<WindowElement> {
        let mut window = self.0.borrow_mut();
        if window.as_ref().map(|w| !w.alive()).unwrap_or(false) {
            *window = None;
        }
        window.clone()
    }

    pub fn clear(&self) -> Option<WindowElement> {
        self.0.borrow_mut().take()
    }
}

impl<BackendData: Backend> BufferHandler for AnvilState<BackendData> {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl<BackendData: Backend> CompositorHandler for AnvilState<BackendData> {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }
    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        if let Some(state) = client.get_data::<ClientState>() {
            return &state.compositor_state;
        }
        panic!("Unknown client data type")
    }

    fn new_surface(&mut self, surface: &WlSurface) {
        add_pre_commit_hook::<Self, _>(surface, move |state, _dh, surface| {
            #[cfg(feature = "udev")]
            let mut acquire_point = None;
            let maybe_dmabuf = with_states(surface, |surface_data| {
                #[cfg(feature = "udev")]
                acquire_point.clone_from(
                    &surface_data
                        .cached_state
                        .get::<DrmSyncobjCachedState>()
                        .pending()
                        .acquire_point,
                );
                surface_data
                    .cached_state
                    .get::<SurfaceAttributes>()
                    .pending()
                    .buffer
                    .as_ref()
                    .and_then(|assignment| match assignment {
                        BufferAssignment::NewBuffer(buffer) => get_dmabuf(buffer).cloned().ok(),
                        _ => None,
                    })
            });
            if let Some(dmabuf) = maybe_dmabuf {
                // Not shown before it is checked (`dmabuf_check`).
                if let Some(gate) = state.backend_data.dmabuf_gate(&dmabuf)
                    && let Some(client) = surface.client()
                {
                    gate.hold(client);
                    add_blocker(surface, gate);
                }
                #[cfg(feature = "udev")]
                if let Some(acquire_point) = acquire_point
                    && let Ok((blocker, source)) = acquire_point.generate_blocker() {
                        let client = surface.client().unwrap();
                        let res = state.handle.insert_source(source, move |_, _, data| {
                            let dh = data.display_handle.clone();
                            data.client_compositor_state(&client).blocker_cleared(data, &dh);
                            Ok(())
                        });
                        if res.is_ok() {
                            add_blocker(surface, blocker);
                            return;
                        }
                    }
                if let Ok((blocker, source)) = dmabuf.generate_blocker(Interest::READ)
                    && let Some(client) = surface.client() {
                        let res = state.handle.insert_source(source, move |_, _, data| {
                            let dh = data.display_handle.clone();
                            data.client_compositor_state(&client).blocker_cleared(data, &dh);
                            Ok(())
                        });
                        if res.is_ok() {
                            add_blocker(surface, blocker);
                        }
                    }
            }
        });
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        self.backend_data.early_import(surface);

        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self.window_for_surface(&root) {
                window.0.on_commit();

                if &root == surface {
                    self.place_on_content(surface);
                    let buffer_offset = with_states(surface, |states| {
                        states
                            .cached_state
                            .get::<SurfaceAttributes>()
                            .current()
                            .buffer_delta
                            .take()
                    });

                    if let Some(buffer_offset) = buffer_offset {
                        let current_loc = self.space.element_location(&window).unwrap();
                        self.space.relocate_element(&window, current_loc + buffer_offset);
                    }
                    let held = self.held_size(&window);
                    keep_on_screen(&mut self.space, surface, &window, held);
                    self.tile_follower_drew(&window);
                    self.keep_in_tile_area(&window);
                }
            }
        }
        self.popups.commit(surface);

        if matches!(&self.cursor_status, CursorImageStatus::Surface(cursor_surface) if cursor_surface == surface)
        {
            with_states(surface, |states| {
                let cursor_image_attributes = states.data_map.get::<CursorImageSurfaceData>();

                if let Some(mut cursor_image_attributes) =
                    cursor_image_attributes.map(|attrs| attrs.lock().unwrap())
                {
                    let buffer_delta = states
                        .cached_state
                        .get::<SurfaceAttributes>()
                        .current()
                        .buffer_delta
                        .take();
                    if let Some(buffer_delta) = buffer_delta {
                        tracing::trace!(hotspot = ?cursor_image_attributes.hotspot, ?buffer_delta, "decrementing cursor hotspot");
                        cursor_image_attributes.hotspot -= buffer_delta;
                    }
                }
            });
        }

        if matches!(&self.dnd_icon, Some(icon) if &icon.surface == surface) {
            let dnd_icon = self.dnd_icon.as_mut().unwrap();
            with_states(&dnd_icon.surface, |states| {
                let buffer_delta = states
                    .cached_state
                    .get::<SurfaceAttributes>()
                    .current()
                    .buffer_delta
                    .take()
                    .unwrap_or_default();
                tracing::trace!(offset = ?dnd_icon.offset, ?buffer_delta, "moving dnd offset");
                dnd_icon.offset += buffer_delta;
            });
        }

        ensure_initial_configure(surface, &self.space, &mut self.popups)
    }
}

impl<BackendData: Backend> WlrLayerShellHandler for AnvilState<BackendData> {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        wl_output: Option<wl_output::WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        let asked = wl_output.as_ref().and_then(Output::from_resource);
        // The palette opens on the display in use (the focused window's,
        // else the pointer's), as shortcuts act there, not on the main one.
        let asked = asked.or_else(|| OPEN_WHERE_IN_USE.contains(&namespace.as_str()).then(|| self.active_output()).flatten());
        let auto = asked.is_none();
        let Some(output) = asked.or_else(|| self.main_output()) else {
            surface.send_close();
            return;
        };
        let layer = LayerSurface::new(surface, namespace);
        if auto {
            // Follows the main display when it changes.
            layer.user_data().insert_if_missing(|| AutoOutput);
        }
        let mut map = layer_map_for_output(&output);
        map.map_layer(&layer).unwrap();
    }

    /// A layer surface's popup (Deskbar's menus) only learns its parent
    /// now, after xdg-shell's `new_popup`: keep it on screen here.
    fn new_popup(&mut self, _parent: WlrLayerSurface, popup: smithay::wayland::shell::xdg::PopupSurface) {
        self.unconstrain_popup(&popup);
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        // Every display's layers, those of one mirroring another too (out
        // of the space): a bar closed because its display began mirroring
        // would stay in its map, empty, and count as there when the
        // display comes back (Deskbar's, moving the tiling area over).
        if let Some((mut map, layer)) = self.space.outputs().chain(self.mirrors.iter()).find_map(|o| {
            let map = layer_map_for_output(o);
            let layer = map
                .layers()
                .find(|&layer| layer.layer_surface() == &surface)
                .cloned();
            layer.map(|layer| (map, layer))
        }) {
            map.unmap_layer(&layer);
        }
        // It had the keyboard (the palette, Quick Look's overlay…): give
        // it back to the window in front, or nothing would have it and
        // the next palette would find no window to act on.
        let keyboard = self.seat.get_keyboard().unwrap();
        let had_focus = match keyboard.current_focus() {
            Some(crate::focus::KeyboardFocusTarget::LayerSurface(l)) => l.layer_surface() == &surface,
            Some(f) => !f.alive(),
            None => true,
        };
        if had_focus && !self.lock.locked() {
            self.focus_topmost();
        }
    }
}

impl<BackendData: Backend> AnvilState<BackendData> {
    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<WindowElement> {
        self.space
            .elements()
            .find(|window| window.wl_surface().map(|s| &*s == surface).unwrap_or(false))
            .cloned()
    }
}

#[derive(Default)]
pub struct SurfaceData {
    pub geometry: Option<Rectangle<i32, Logical>>,
    pub resize_state: ResizeState,
    /// The size the window was last kept on screen at.
    pub fitted: Option<Size<i32, Logical>>,
}

/// When a window first takes a size, or the app resizes it, slide it so
/// all of it (tab included) is on the screen it is mostly on, where it
/// can be: a window opened at the cascade position that is taller than
/// the room below it moves up instead of hanging off the bottom. Windows
/// the user is resizing, ones put back where they were, and a size the
/// window was asked to take (zoomed, tiled, unzoomed: Olo placed it) are
/// left alone.
///
/// An app doesn't make its window bigger than the screen, or change the
/// size Olo holds it to (`held`, width and height: zoomed, tiled): it is
/// asked back to the size it was given, or to fit the screen, unless its
/// minimum size leaves no choice.
fn keep_on_screen(space: &mut Space<WindowElement>, surface: &WlSurface, window: &WindowElement, held: (bool, bool)) {
    let Some(geo) = space.element_geometry(window) else { return };
    if geo.size.w <= 0 || geo.size.h <= 0 {
        return;
    }
    let changed = with_states(surface, |states| {
        let data = states.data_map.get_or_insert(|| RefCell::new(SurfaceData::default()));
        let mut data = data.borrow_mut();
        if data.fitted == Some(geo.size) || !matches!(data.resize_state, ResizeState::NotResizing) {
            return false;
        }
        data.fitted = Some(geo.size);
        true
    });
    if !changed {
        return;
    }
    // (Worked out before taking the surface's lock again.)
    let content = window.inner_size(geo.size);
    let asked = window.0.toplevel().and_then(|t| t.with_committed_state(|s| s.and_then(|s| s.size)));
    if asked == Some(content) {
        return;
    }
    let Some(toplevel) = window.0.toplevel() else { return };
    // (Fullscreen: the display's size, whatever it draws.)
    if toplevel.with_pending_state(|s| s.states.contains(xdg_toplevel::State::Fullscreen)) {
        return;
    }
    let least = with_states(surface, |states| states.cached_state.get::<SurfaceCachedState>().current().min_size);
    let (hold_w, hold_h) = held;
    if (hold_w || hold_h)
        && let Some(given) = toplevel.with_pending_state(|s| s.size)
    {
        let back = (hold_w && content.w != given.w && given.w >= least.w) || (hold_h && content.h != given.h && given.h >= least.h);
        if back {
            if !grabs::awaiting_ack(toplevel) {
                let size = Size::from((if hold_w { given.w } else { content.w }, if hold_h { given.h } else { content.h }));
                toplevel.with_pending_state(|s| s.size = Some(size));
                toplevel.send_configure();
            }
            return;
        }
    }
    let overlap = |o: &Output| space.output_geometry(o).and_then(|g| g.intersection(geo)).map_or(0, |r| r.size.w as i64 * r.size.h as i64);
    let middle = (geo.loc + geo.size.downscale(2).to_point()).to_f64();
    let Some(zone) = space
        .output_under(middle)
        .next()
        .or_else(|| space.outputs().filter(|o| overlap(o) > 0).max_by_key(|o| overlap(o)))
        .or_else(|| space.outputs().next())
        .and_then(|o| {
            let g = space.output_geometry(o)?;
            let z = layer_map_for_output(o).non_exclusive_zone();
            Some(Rectangle::new(g.loc + z.loc, z.size))
        })
    else {
        return;
    };
    if window.meta().put_back.replace(false) && zone.contains(geo.loc) {
        return;
    }
    const MARGIN: i32 = 8;
    // Too big for the screen: as big as fits (or as small as it can be).
    let room = |span: i32, frame: i32, least: i32, now: i32| (span - 2 * MARGIN - frame).max(least).min(now);
    let fitting = Size::from((
        if hold_w { content.w } else { room(zone.size.w, geo.size.w - content.w, least.w, content.w) },
        if hold_h { content.h } else { room(zone.size.h, geo.size.h - content.h, least.h, content.h) },
    ));
    if fitting != content {
        toplevel.with_pending_state(|s| s.size = Some(fitting));
        toplevel.send_configure();
    }
    let fit = |pos: i32, len: i32, lo: i32, span: i32| {
        let hi = lo + span;
        if pos + len + MARGIN > hi { (hi - len - MARGIN).max(lo + MARGIN).min(pos) } else { pos }
    };
    let x = fit(geo.loc.x, geo.size.w, zone.loc.x, zone.size.w);
    let y = fit(geo.loc.y, geo.size.h, zone.loc.y, zone.size.h);
    if (x, y) != (geo.loc.x, geo.loc.y) {
        let loc = space.element_location(window).unwrap_or_default();
        space.relocate_element(window, loc + Point::from((x - geo.loc.x, y - geo.loc.y)));
    }
}

fn ensure_initial_configure(surface: &WlSurface, space: &Space<WindowElement>, popups: &mut PopupManager) {
    with_surface_tree_upward(
        surface,
        (),
        |_, _, _| TraversalAction::DoChildren(()),
        |_, states, _| {
            states
                .data_map
                .insert_if_missing(|| RefCell::new(SurfaceData::default()));
        },
        |_, _, _| true,
    );

    if let Some(window) = space
        .elements()
        .find(|window| window.wl_surface().map(|s| &*s == surface).unwrap_or(false))
        .cloned()
    {
        // send the initial configure if relevant
        if let Some(toplevel) = window.0.toplevel() {
            let initial_configure_sent = with_states(surface, |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .initial_configure_sent
            });
            if !initial_configure_sent {
                toplevel.send_configure();
            }
            let resizing = with_states(surface, |states| {
                states
                    .data_map
                    .get::<RefCell<SurfaceData>>()
                    .is_some_and(|d| matches!(d.borrow().resize_state, ResizeState::Resizing(_)))
            });
            if resizing {
                grabs::resize_caught_up(toplevel);
            }
        }

        with_states(surface, |states| {
            let mut data = states
                .data_map
                .get::<RefCell<SurfaceData>>()
                .unwrap()
                .borrow_mut();

            // Finish resizing.
            if let ResizeState::WaitingForCommit(_) = data.resize_state {
                data.resize_state = ResizeState::NotResizing;
            }
        });

        return;
    }

    if let Some(popup) = popups.find_popup(surface) {
        let popup = match popup {
            PopupKind::Xdg(ref popup) => popup,
            // Doesn't require configure
            PopupKind::InputMethod(ref _input_popup) => {
                return;
            }
        };

        if !popup.is_initial_configure_sent() {
            // NOTE: This should never fail as the initial configure is always
            // allowed.
            popup.send_configure().expect("initial configure failed");
        }

        return;
    };

    if let Some(output) = space.outputs().find(|o| {
        let map = layer_map_for_output(o);
        map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
            .is_some()
    }) {
        let initial_configure_sent = with_states(surface, |states| {
            states
                .data_map
                .get::<LayerSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .initial_configure_sent
        });

        let mut map = layer_map_for_output(output);

        // arrange the layers before sending the initial configure
        // to respect any size the client may have sent
        map.arrange();
        // send the initial configure if relevant
        if !initial_configure_sent {
            let layer = map
                .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                .unwrap();

            layer.layer_surface().send_configure();
        }
    };
}

fn place_new_window(
    space: &mut Space<WindowElement>,
    pointer_location: Point<f64, Logical>,
    window: &WindowElement,
    activate: bool,
) {
    // place the window at a random location on same output as pointer
    // or if there is not output in a [0;800]x[0;800] square

    let output = space
        .output_under(pointer_location)
        .next()
        .or_else(|| space.outputs().next())
        .cloned();
    let output_geometry = output
        .and_then(|o| {
            let geo = space.output_geometry(&o)?;
            let map = layer_map_for_output(&o);
            let zone = map.non_exclusive_zone();
            Some(Rectangle::new(geo.loc + zone.loc, zone.size))
        })
        .unwrap_or_else(|| Rectangle::from_size((800, 800).into()));

    // set the initial toplevel bounds
    #[allow(irrefutable_let_patterns)]
    if let Some(toplevel) = window.0.toplevel() {
        toplevel.with_pending_state(|state| {
            state.bounds = Some(output_geometry.size);
        });
    }

    // BeOS-style cascade: the first diagonal slot no other window sits on.
    const STEP: i32 = 24;
    const ORIGIN: Point<i32, Logical> = Point::new(48, 40);
    let taken: Vec<Point<i32, Logical>> = space
        .elements()
        .filter(|w| *w != window)
        .filter_map(|w| space.element_location(w))
        .collect();
    let slots = ((output_geometry.size.h / 3) / STEP).max(1);
    let n = (0..slots)
        .find(|i| {
            let p = output_geometry.loc + ORIGIN + Point::from((i * STEP, i * STEP));
            !taken.contains(&p)
        })
        .unwrap_or(0);
    let Point { x, y, .. } = output_geometry.loc + ORIGIN + Point::from((n * STEP, n * STEP));

    space.map_element(window.clone(), (x, y), activate);
}

/// Where outputs go, in logical coordinates: each at its saved position
/// (Preferences ▸ Display) unless that would overlap one already placed;
/// the rest in a row to the right. The whole layout is then shifted to
/// start at (0, 0), which keeps it stable however it was dragged.
pub fn arrange(outputs: &[(String, Size<i32, Logical>)], saved: impl Fn(&str) -> Option<(i32, i32)>) -> Vec<Point<i32, Logical>> {
    let mut placed: Vec<Option<Rectangle<i32, Logical>>> = vec![None; outputs.len()];
    for (i, (name, size)) in outputs.iter().enumerate() {
        let Some(pos) = saved(name) else { continue };
        let r = Rectangle::new(pos.into(), *size);
        if !placed.iter().flatten().any(|p| p.overlaps(r)) {
            placed[i] = Some(r);
        }
    }
    for (i, (_, size)) in outputs.iter().enumerate() {
        if placed[i].is_none() {
            let rects: Vec<_> = placed.iter().flatten().copied().collect();
            let x = rects.iter().map(|r| r.loc.x + r.size.w).max().unwrap_or(0);
            let y = rects.iter().map(|r| r.loc.y).min().unwrap_or(0);
            placed[i] = Some(Rectangle::new((x, y).into(), *size));
        }
    }
    let mut rects: Vec<_> = placed.into_iter().flatten().collect();
    close_gaps(&mut rects);
    let min_x = rects.iter().map(|r| r.loc.x).min().unwrap_or(0);
    let min_y = rects.iter().map(|r| r.loc.y).min().unwrap_or(0);
    rects.iter().map(|r| r.loc - Point::from((min_x, min_y))).collect()
}

/// Whether two screens share a stretch of edge (the pointer can cross).
fn touching(a: &Rectangle<i32, Logical>, b: &Rectangle<i32, Logical>) -> bool {
    let (ar, ab, br, bb) = (a.loc.x + a.size.w, a.loc.y + a.size.h, b.loc.x + b.size.w, b.loc.y + b.size.h);
    let y_shared = a.loc.y.max(b.loc.y) < ab.min(bb);
    let x_shared = a.loc.x.max(b.loc.x) < ar.min(br);
    a.overlaps(*b) || (ar == b.loc.x || br == a.loc.x) && y_shared || (ab == b.loc.y || bb == a.loc.y) && x_shared
}

/// The screens reachable from `from` across shared edges.
fn reachable(rects: &[Rectangle<i32, Logical>], from: usize) -> Vec<usize> {
    let mut seen = vec![from];
    let mut i = 0;
    while i < seen.len() {
        let r = rects[seen[i]];
        for (j, other) in rects.iter().enumerate() {
            if !seen.contains(&j) && touching(&r, other) {
                seen.push(j);
            }
        }
        i += 1;
    }
    seen
}

/// Screens that don't touch the rest move over until they do: a gap (left
/// by a screen that was between them and is unplugged, or saved by an
/// older version) would leave the pointer unable to reach them. Nearest
/// first, keeping each one's side and, where it can, its offset.
fn close_gaps(rects: &mut [Rectangle<i32, Logical>]) {
    for _ in 0..rects.len() {
        let group = reachable(rects, 0);
        if group.len() == rects.len() {
            return;
        }
        let gap = |a: &Rectangle<i32, Logical>, b: &Rectangle<i32, Logical>| {
            let dx = (a.loc.x - (b.loc.x + b.size.w)).max(b.loc.x - (a.loc.x + a.size.w)).max(0);
            let dy = (a.loc.y - (b.loc.y + b.size.h)).max(b.loc.y - (a.loc.y + a.size.h)).max(0);
            dx + dy
        };
        let Some((i, j)) = (0..rects.len())
            .filter(|i| !group.contains(i))
            .flat_map(|i| group.iter().map(move |&j| (i, j)))
            .min_by_key(|&(i, j)| gap(&rects[i], &rects[j]))
        else {
            return;
        };
        let (a, b) = (rects[i], rects[j]);
        let y_shared = a.loc.y.max(b.loc.y) < (a.loc.y + a.size.h).min(b.loc.y + b.size.h);
        let x_shared = a.loc.x.max(b.loc.x) < (a.loc.x + a.size.w).min(b.loc.x + b.size.w);
        let beside = if a.loc.x >= b.loc.x + b.size.w { b.loc.x + b.size.w - a.loc.x } else { b.loc.x - a.loc.x - a.size.w };
        let below = if a.loc.y >= b.loc.y + b.size.h { b.loc.y + b.size.h - a.loc.y } else { b.loc.y - a.loc.y - a.size.h };
        let delta: Point<i32, Logical> = if y_shared {
            (beside, 0).into()
        } else if x_shared {
            (0, below).into()
        } else {
            // Diagonal: beside it, tops lined up.
            (beside, b.loc.y - a.loc.y).into()
        };
        // Its own neighbours come along; if that would overlap, it goes
        // alone to the right of the rightmost screen.
        let moving: Vec<usize> = reachable(rects, i);
        let fits = moving.iter().all(|&m| {
            let moved = Rectangle::new(rects[m].loc + delta, rects[m].size);
            group.iter().all(|&g| !moved.overlaps(rects[g]))
        });
        if fits {
            for m in moving {
                rects[m].loc += delta;
            }
        } else {
            let right = group.iter().map(|&g| rects[g]).max_by_key(|r| r.loc.x + r.size.w).unwrap_or(b);
            rects[i].loc = (right.loc.x + right.size.w, right.loc.y).into();
        }
    }
}

/// Layer surfaces (by namespace) that, naming no output, open on the
/// display in use rather than the main one: called up from the keyboard
/// to work there.
const OPEN_WHERE_IN_USE: &[&str] = &["palette"];

/// Marks a layer surface placed on the main display because it didn't
/// name an output.
#[derive(Debug)]
pub struct AutoOutput;

/// Where a screen was when the windows were last arranged on the screens
/// (`fixup_positions`).
#[derive(Default)]
struct Arranged(std::cell::Cell<Option<Rectangle<i32, Logical>>>);

pub fn fixup_positions(
    space: &mut Space<WindowElement>,
    display: &olo_protocol::DisplaySettings,
    pointer_location: Point<f64, Logical>,
) {
    // fixup outputs
    let outputs: Vec<Output> = space.outputs().cloned().collect();
    let sizes: Vec<(String, Size<i32, Logical>)> = outputs
        .iter()
        .map(|o| (o.name(), space.output_geometry(o).map(|geo| geo.size).unwrap_or_else(|| Size::from((0, 0)))))
        .collect();
    let keys: std::collections::HashMap<String, String> = outputs.iter().map(|o| (o.name(), crate::desktop::display_key(o))).collect();
    let positions = arrange(&sizes, |name| display.config(keys.get(name).map_or(name, |k| k.as_str()), name).and_then(|c| c.position));
    // Each window's screen (under the middle of its frame, as the screens
    // were when last arranged: a new scale or mode has already changed a
    // screen's size, not yet its place, and a window on its far side would
    // be under none, or another) and where on it, so screens moving in the
    // arrangement take their windows along.
    let was = |o: &Output| o.user_data().get::<Arranged>().and_then(|a| a.0.get()).or_else(|| space.output_geometry(o));
    let on: Vec<(WindowElement, Output, Point<i32, Logical>, Rectangle<i32, Logical>)> = space
        .elements()
        .filter_map(|w| {
            let loc = space.element_location(w)?;
            let f = space.element_geometry(w)?;
            let middle = (f.loc + f.size.downscale(2).to_point()).to_f64();
            let o = outputs.iter().find(|o| was(o).is_some_and(|g| g.to_f64().contains(middle))).cloned().or_else(|| space.output_under(middle).next().cloned())?;
            Some((w.clone(), o.clone(), loc - was(&o)?.loc, f))
        })
        .collect();
    for (output, at) in outputs.iter().zip(positions) {
        space.map_output(output, at);
        layer_map_for_output(output).arrange();
    }
    for o in &outputs {
        o.user_data().insert_if_missing(Arranged::default);
        if let Some(a) = o.user_data().get::<Arranged>() {
            a.0.set(space.output_geometry(o));
        }
    }
    for (w, o, offset, f) in on {
        if let Some(g) = space.output_geometry(&o) {
            let mut to = g.loc + offset;
            // Its screen got smaller under it: in from the side it went
            // past, so it stays on its own screen (not the one beside).
            let at = to + (f.loc - space.element_location(&w).unwrap_or(to));
            let middle = (at + f.size.downscale(2).to_point()).to_f64();
            if !g.to_f64().contains(middle) {
                let fit = |pos: i32, len: i32, lo: i32, span: i32| if len <= span { pos.clamp(lo, lo + span - len) } else { lo };
                let x = fit(at.x, f.size.w, g.loc.x, g.size.w);
                let y = fit(at.y, f.size.h, g.loc.y, g.size.h);
                to += Point::from((x - at.x, y - at.y));
            }
            if let Some(from) = space.element_location(&w) {
                tiling::shift_remembered(&w, to - from);
            }
            space.relocate_element(&w, to);
        }
    }

    // fixup windows
    let mut orphaned_windows = Vec::new();
    let outputs = space
        .outputs()
        .flat_map(|o| {
            let geo = space.output_geometry(o)?;
            let map = layer_map_for_output(o);
            let zone = map.non_exclusive_zone();
            Some(Rectangle::new(geo.loc + zone.loc, zone.size))
        })
        .collect::<Vec<_>>();
    for window in space.elements() {
        let window_location = match space.element_location(window) {
            Some(loc) => loc,
            None => continue,
        };
        let geo_loc = window.bbox().loc + window_location;

        if !outputs.iter().any(|o_geo| o_geo.contains(geo_loc)) {
            orphaned_windows.push(window.clone());
        }
    }
    // Windows left on no display go to the main one (state.rehome_windows
    // then fits them there).
    let main = space
        .outputs()
        .find(|o| display.is_main(&crate::desktop::display_key(o), &o.name()))
        .and_then(|o| space.output_geometry(o))
        .map(|g| (g.loc + g.size.downscale(2).to_point()).to_f64());
    for window in orphaned_windows.into_iter() {
        place_new_window(space, main.unwrap_or(pointer_location), &window, false);
    }
}

#[cfg(test)]
mod arrange_tests {
    use super::*;

    fn out(name: &str, w: i32, h: i32) -> (String, Size<i32, Logical>) {
        (name.into(), (w, h).into())
    }

    #[test]
    fn gaps_close() {
        // Ben's three: Dell, Studio Display, built-in, left to right, the
        // Studio's place lost: the built-in moves back beside the Dell.
        let outs = [out("DP-2", 1920, 1080), out("DP-1", 2560, 1440), out("eDP-1", 1728, 1080)];
        let saved = |n: &str| match n {
            "DP-2" => Some((0, 1196)),
            "eDP-1" => Some((4480, 1196)),
            _ => None,
        };
        let p = arrange(&outs, saved);
        assert_eq!(p, vec![(0, 0).into(), (3648, 0).into(), (1920, 0).into()]);
        // All three saved: as saved (shifted to start at 0, 0).
        let p = arrange(&outs, |n| match n {
            "DP-1" => Some((1920, 1016)),
            n => saved(n),
        });
        assert_eq!(p, vec![(0, 180).into(), (1920, 0).into(), (4480, 180).into()]);
        // One above another with a gap: it comes down to touch.
        let outs = [out("A", 1728, 1117), out("B", 1920, 1080)];
        let p = arrange(&outs, |n| match n {
            "A" => Some((0, 0)),
            "B" => Some((0, -2000)),
            _ => None,
        });
        assert_eq!(p, vec![(0, 1080).into(), (0, 0).into()]);
        // Only a corner in common: beside it instead.
        let p = arrange(&outs, |n| match n {
            "A" => Some((0, 0)),
            "B" => Some((1728, 1117)),
            _ => None,
        });
        assert_eq!(p, vec![(0, 0).into(), (1728, 0).into()]);
    }

    #[test]
    fn saved_positions_and_rows() {
        let outs = [out("A", 1728, 1117), out("B", 1920, 1080)];
        // Nothing saved: a row.
        let p = arrange(&outs, |_| None);
        assert_eq!(p, vec![(0, 0).into(), (1728, 0).into()]);
        // B above A, centred: shifted so the layout starts at (0, 0).
        let p = arrange(&outs, |n| match n {
            "A" => Some((0, 0)),
            "B" => Some((-96, -1080)),
            _ => None,
        });
        assert_eq!(p, vec![(96, 1080).into(), (0, 0).into()]);
        // Overlapping saved positions: the second falls back to the row.
        let p = arrange(&outs, |_| Some((0, 0)));
        assert_eq!(p, vec![(0, 0).into(), (1728, 0).into()]);
    }
}

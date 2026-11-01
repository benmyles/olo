//! One program's connection: passed through to app_server, with enough of
//! its state kept to build it again on a new app_server.
//!
//! Every message is decoded (the proxy knows each global's interface; it
//! hides the ones it doesn't). Requests are passed on in order, and as
//! they go each object's lasting state is noted: how it was made, the
//! setters that still hold (title, app id, window geometry, …), a
//! surface's committed buffer, scale and regions, a buffer's recipe (its
//! pool or dma-buf planes, descriptors kept), subsurface stacking.
//!
//! When app_server goes away the program is told what a restart ends
//! (pointer and keyboard leave, frame callbacks fire, popups are dismissed,
//! selections cancelled, buffers released) and its requests wait. On the
//! new app_server the objects are made again with the same ids, in the
//! order they were first made; then each window's role is committed and
//! its configure acknowledged, and its contents committed; then the
//! waiting requests go on and the program never knew.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;

use wayland_backend::protocol::{AllowNull, ArgumentType, Interface};

use crate::interfaces::{self as ifs, event, request};
use crate::wire::{Arg, Conn, Msg, SERVER_START, decode};

/// In a buffer's recipe, the temporary pool or params object.
const TEMP: u32 = u32::MAX;
/// The hello on a connection to app_server, and its answer (with pids).
pub const HELLO: &[u8; 4] = b"TBPX";
pub const HELLO_LEN: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Waiting for app_server's answer to our hello.
    Hello,
    /// Replaying: learning the new app_server's globals.
    Globals,
    /// Replaying: windows' roles committed, waiting for their configures.
    Configure,
    Live,
    /// app_server has gone; waiting for the next one.
    Down,
}

#[derive(Default)]
struct Surface {
    pending: Contents,
    current: Contents,
    /// This surface and its subsurfaces' surfaces, bottom to top.
    stack: Vec<u32>,
    /// The wl_subsurface making this a subsurface.
    sub: Option<u32>,
    xdg: Option<u32>,
    layer: Option<u32>,
}

type Rects = Vec<(bool, [i32; 4])>;

#[derive(Default, Clone)]
struct Contents {
    /// `Some(0)`: no buffer.
    buffer: Option<u32>,
    scale: Option<i32>,
    transform: Option<i32>,
    /// `Some(None)`: the whole surface.
    input: Option<Option<Rects>>,
    opaque: Option<Option<Rects>>,
}

impl Contents {
    fn apply(&mut self, p: Contents) {
        self.buffer = p.buffer.or(self.buffer);
        self.scale = p.scale.or(self.scale);
        self.transform = p.transform.or(self.transform);
        self.input = p.input.or(self.input.take());
        self.opaque = p.opaque.or(self.opaque.take());
    }
}

#[derive(Default)]
enum Extra {
    #[default]
    None,
    Surface(Box<Surface>),
    /// How to make the buffer again (`TEMP` standing for a temporary
    /// object), and whether app_server still holds it.
    Buffer { recipe: Vec<Msg>, busy: bool, via: Option<Parent> },
    Pool { size: i32 },
    Params { adds: Vec<Msg>, used: bool },
    Region(Rects),
    XdgSurface { surface: u32, role: Option<u32> },
    Popup { xdg: u32 },
    Toplevel { xdg: u32 },
    Layer { surface: u32 },
    Subsurface { surface: u32, parent: u32 },
    Token { done: bool },
}

/// What an object was made from, as it was then: the id may since have
/// been destroyed, or reused for something else.
#[derive(Clone, Copy)]
struct Parent {
    id: u32,
    seq: u64,
    iface: &'static Interface,
    version: u32,
    /// The global it was bound from, if it was.
    global: Option<u32>,
}

struct Obj {
    iface: &'static Interface,
    version: u32,
    /// When it was made, to make it again in the same order.
    seq: u64,
    /// The request that made it, descriptors duplicated.
    create: Option<Msg>,
    parent: Option<Parent>,
    /// Bound from this global (the program's name for it).
    global: Option<u32>,
    /// Made by a request still waiting to be passed on.
    unsent: bool,
    /// Can't be made again: requests to it are dropped until the program
    /// destroys it.
    zombie: bool,
    /// Destroyed; the id is free again after wl_display.delete_id.
    destroyed: bool,
    /// Setters that still hold, the latest of each kind.
    slots: Vec<(&'static str, Msg)>,
    x: Extra,
}

impl Obj {
    fn new(iface: &'static Interface, version: u32, seq: u64) -> Obj {
        Obj { iface, version, seq, create: None, parent: None, global: None, unsent: true, zombie: false, destroyed: false, slots: Vec::new(), x: Extra::None }
    }

    fn live(&self) -> bool {
        !self.zombie && !self.destroyed && !self.unsent
    }
}

struct Global {
    /// The name the program knows it by.
    name: u32,
    iface: &'static Interface,
    version: u32,
    /// Its name on the app_server we're connected to.
    up: Option<u32>,
}

/// Objects of our own on app_server, made while replaying.
enum Temp {
    Filler,
    Barrier(Phase),
    Other,
}

pub struct Client {
    pub down: Conn,
    pub up: Option<Conn>,
    pub pid: i32,
    pub uid: u32,
    pub phase: Phase,
    /// The app_server we're connected to, and the one before.
    pub server: i32,
    old_server: i32,
    pub down_since: Option<Instant>,
    pub last_try: Option<Instant>,
    /// app_server sent a protocol error: the connection ends for good.
    errored: bool,
    objs: HashMap<u32, Obj>,
    seq: u64,
    /// The highest id the program has used.
    max_id: u32,
    /// Requests waiting to be passed on.
    queue: VecDeque<Msg>,
    globals: Vec<Global>,
    /// Globals the new app_server announced while replaying.
    fresh: Vec<(u32, &'static Interface, u32)>,
    /// The program's ids for app_server's objects, and back.
    s_up: HashMap<u32, u32>,
    s_down: HashMap<u32, u32>,
    /// app_server objects the program destroyed, for decoding messages
    /// already on their way.
    graveyard: HashMap<u32, (&'static Interface, u32)>,
    temps: HashMap<u32, (&'static Interface, Temp)>,
    next_temp: u32,
    /// Configures we acknowledged ourselves, and those the current
    /// app_server sent: (xdg or layer surface, serial).
    acked: HashSet<(u32, u32)>,
    sent_configures: HashSet<(u32, u32)>,
    /// Device → the surface it entered (pointer, keyboard, text input,
    /// drag and drop).
    focus: HashMap<u32, u32>,
    started: Instant,
}

type Res<T = ()> = Result<T, String>;

impl Client {
    pub fn new(down: Conn, pid: i32, uid: u32) -> Client {
        let mut objs = HashMap::new();
        let mut display = Obj::new(ifs::display(), 1, 0);
        display.unsent = false;
        objs.insert(1, display);
        Client {
            down,
            up: None,
            pid,
            uid,
            phase: Phase::Down,
            server: 0,
            old_server: 0,
            down_since: Some(Instant::now()),
            last_try: None,
            errored: false,
            objs,
            seq: 1,
            max_id: 1,
            queue: VecDeque::new(),
            globals: Vec::new(),
            fresh: Vec::new(),
            s_up: HashMap::new(),
            s_down: HashMap::new(),
            graveyard: HashMap::new(),
            temps: HashMap::new(),
            next_temp: 0,
            acked: HashSet::new(),
            sent_configures: HashSet::new(),
            focus: HashMap::new(),
            started: Instant::now(),
        }
    }

    /// Connected (again): say who we're speaking for.
    pub fn connected(&mut self, mut up: Conn) {
        let mut hello = Vec::with_capacity(HELLO_LEN);
        hello.extend_from_slice(HELLO);
        hello.extend_from_slice(&1u32.to_ne_bytes());
        hello.extend_from_slice(&self.pid.to_ne_bytes());
        hello.extend_from_slice(&self.uid.to_ne_bytes());
        up.send_raw(&hello);
        self.up = Some(up);
        self.phase = Phase::Hello;
    }

    pub fn waiting(&self) -> usize {
        self.queue.len()
    }

    // ---- from the program -------------------------------------------------

    pub fn read_client(&mut self) -> Res {
        while let Some((id, op, len)) = self.down.peek()? {
            let body = self.down.inbuf[8..len].to_vec();
            self.down.inbuf.drain(..len);
            let Some(obj) = self.objs.get(&id) else {
                return Err(format!("request to unknown object {id}"));
            };
            let (iface, version) = (obj.iface, obj.version);
            let desc = iface.requests.get(op as usize).ok_or_else(|| format!("{}: no request {op}", iface.name))?;
            let msg = decode(id, op, &body, desc.signature, &mut self.down.infds)?;
            if let Some(new) = msg.new_id() {
                let (child, version) = if iface.name == "wl_registry" {
                    let child = ifs::global(msg.str(1)).ok_or_else(|| format!("bind to {}", String::from_utf8_lossy(msg.str(1))))?;
                    (child, msg.uint(2))
                } else {
                    (desc.child_interface.ok_or("untyped new id")?, version)
                };
                if new >= SERVER_START || new == 0 {
                    return Err(format!("bad new id {new}"));
                }
                self.max_id = self.max_id.max(new);
                self.seq += 1;
                self.objs.insert(new, Obj::new(child, version, self.seq));
            }
            self.queue.push_back(msg);
            if self.queue.len() > 200_000 {
                return Err("too many requests waiting".into());
            }
        }
        self.pump();
        Ok(())
    }

    /// Pass on waiting requests, if we're live.
    pub fn pump(&mut self) {
        while self.phase == Phase::Live
            && let Some(msg) = self.queue.pop_front()
        {
            self.request(msg);
        }
    }

    fn is_zombie(&self, id: u32) -> bool {
        if id >= SERVER_START && !self.s_up.contains_key(&id) {
            return true;
        }
        self.objs.get(&id).is_none_or(|o| o.zombie)
    }

    fn request(&mut self, mut msg: Msg) {
        let Some(obj) = self.objs.get(&msg.id) else { return };
        let iface = obj.iface;
        let desc = &iface.requests[msg.op as usize];
        let mut doomed = obj.zombie;
        for (i, t) in desc.signature.iter().enumerate() {
            if let ArgumentType::Object(null) = t {
                let o = msg.uint(i);
                if o != 0 && self.is_zombie(o) {
                    if *null == AllowNull::Yes {
                        msg.args[i] = Arg::Object(0);
                    } else {
                        doomed = true;
                    }
                }
            }
        }
        if !doomed && msg.id >= SERVER_START && !self.s_up.contains_key(&msg.id) {
            doomed = true;
        }
        let new = msg.new_id();
        if let Some(new) = new
            && let Some(o) = self.objs.get_mut(&new)
        {
            o.unsent = false;
        }
        if !doomed && !self.record(&mut msg, desc.name) {
            doomed = true;
        }
        if doomed {
            if let Some(new) = new {
                self.zombify(new);
            }
            if desc.is_destructor && self.objs.get(&msg.id).is_some_and(|o| o.zombie) {
                self.forget(msg.id, true);
            }
            return;
        }
        if let Some(new) = new {
            let parent = self.objs.get(&msg.id).map(|p| Parent { id: msg.id, seq: p.seq, iface: p.iface, version: p.version, global: p.global });
            if let Some(o) = self.objs.get_mut(&new) {
                o.create = msg.dup().ok();
                o.parent = parent;
            }
        }
        if desc.is_destructor {
            self.destroyed(msg.id);
        }
        self.send_up(msg);
    }

    /// Note what a request changes; false to drop it.
    fn record(&mut self, msg: &mut Msg, name: &'static str) -> bool {
        let iface = self.objs[&msg.id].iface.name;
        let new = msg.new_id();
        // What a new object is.
        if let Some(new) = new {
            let x = match self.objs[&new].iface.name {
                "wl_surface" => Extra::Surface(Box::new(Surface { stack: vec![new], ..Default::default() })),
                "wl_region" => Extra::Region(Vec::new()),
                "wl_shm_pool" => Extra::Pool { size: msg.int(2) },
                "zwp_linux_buffer_params_v1" => Extra::Params { adds: Vec::new(), used: false },
                "xdg_surface" => {
                    let surface = msg.uint(1);
                    if let Some(s) = self.surface(surface) {
                        s.xdg = Some(new);
                    }
                    Extra::XdgSurface { surface, role: None }
                }
                "xdg_toplevel" => {
                    if let Some(Extra::XdgSurface { role, .. }) = self.objs.get_mut(&msg.id).map(|o| &mut o.x) {
                        *role = Some(new);
                    }
                    Extra::Toplevel { xdg: msg.id }
                }
                "xdg_popup" => {
                    if let Some(Extra::XdgSurface { role, .. }) = self.objs.get_mut(&msg.id).map(|o| &mut o.x) {
                        *role = Some(new);
                    }
                    Extra::Popup { xdg: msg.id }
                }
                "zwlr_layer_surface_v1" => {
                    let surface = msg.uint(1);
                    if let Some(s) = self.surface(surface) {
                        s.layer = Some(new);
                    }
                    Extra::Layer { surface }
                }
                "wl_subsurface" => {
                    let (surface, parent) = (msg.uint(1), msg.uint(2));
                    if let Some(s) = self.surface(surface) {
                        s.sub = Some(new);
                    }
                    if let Some(p) = self.surface(parent) {
                        p.stack.retain(|&s| s != surface);
                        p.stack.push(surface);
                    }
                    Extra::Subsurface { surface, parent }
                }
                "xdg_activation_token_v1" => Extra::Token { done: false },
                "wl_buffer" => self.buffer_recipe(msg),
                _ => Extra::None,
            };
            self.objs.get_mut(&new).unwrap().x = x;
        }
        match (iface, name) {
            ("wl_fixes", "destroy_registry") => self.destroyed(msg.uint(0)),
            ("wl_registry", "bind") => {
                if let Some(o) = new.and_then(|n| self.objs.get_mut(&n)) {
                    o.global = Some(msg.uint(0));
                }
                let Some(g) = self.globals.iter().find(|g| g.name == msg.uint(0)) else { return false };
                let Some(up) = g.up else { return false };
                msg.args[0] = Arg::Uint(up);
            }
            ("wl_surface", _) => {
                let id = msg.id;
                let rects = |c: &Client, r: u32| -> Option<Rects> {
                    match c.objs.get(&r).map(|o| &o.x) {
                        Some(Extra::Region(r)) => Some(r.clone()),
                        _ => None,
                    }
                };
                let input = (name == "set_input_region").then(|| rects(self, msg.uint(0)));
                let opaque = (name == "set_opaque_region").then(|| rects(self, msg.uint(0)));
                let mut busy = None;
                if let Some(s) = self.surface(id) {
                    match name {
                        "attach" => s.pending.buffer = Some(msg.uint(0)),
                        "set_buffer_scale" => s.pending.scale = Some(msg.int(0)),
                        "set_buffer_transform" => s.pending.transform = Some(msg.int(0)),
                        "set_input_region" => s.pending.input = input,
                        "set_opaque_region" => s.pending.opaque = opaque,
                        "commit" => {
                            busy = s.pending.buffer.filter(|&b| b != 0);
                            let p = std::mem::take(&mut s.pending);
                            s.current.apply(p);
                        }
                        _ => {}
                    }
                }
                if let Some(Extra::Buffer { busy, .. }) = busy.and_then(|b| self.objs.get_mut(&b)).map(|o| &mut o.x) {
                    *busy = true;
                }
            }
            ("wl_region", "add" | "subtract") => {
                let r = [msg.int(0), msg.int(1), msg.int(2), msg.int(3)];
                if let Some(Extra::Region(rects)) = self.objs.get_mut(&msg.id).map(|o| &mut o.x) {
                    rects.push((name == "add", r));
                }
            }
            ("wl_shm_pool", "resize") => {
                if let Some(Extra::Pool { size }) = self.objs.get_mut(&msg.id).map(|o| &mut o.x) {
                    *size = msg.int(0);
                }
            }
            ("zwp_linux_buffer_params_v1", "add") => {
                if let (Ok(m), Some(Extra::Params { adds, .. })) = (msg.dup(), self.objs.get_mut(&msg.id).map(|o| &mut o.x)) {
                    adds.push(m);
                }
            }
            ("zwp_linux_buffer_params_v1", "create" | "create_immed") => {
                if let Some(Extra::Params { used, .. }) = self.objs.get_mut(&msg.id).map(|o| &mut o.x) {
                    *used = true;
                }
            }
            ("wl_subsurface", "place_above" | "place_below") => {
                if let Some(Extra::Subsurface { surface, parent }) = self.objs.get(&msg.id).map(|o| &o.x) {
                    let (surface, parent, sibling) = (*surface, *parent, msg.uint(0));
                    if let Some(p) = self.surface(parent)
                        && surface != sibling
                    {
                        p.stack.retain(|&s| s != surface);
                        let at = p.stack.iter().position(|&s| s == sibling).unwrap_or(p.stack.len() - 1);
                        p.stack.insert(if name == "place_above" { at + 1 } else { at }, surface);
                    }
                }
            }
            ("xdg_surface" | "zwlr_layer_surface_v1", "ack_configure") => {
                let key = (msg.id, msg.uint(0));
                if self.acked.remove(&key) {
                    return false;
                }
                if !self.sent_configures.contains(&key) {
                    // Sent by an app_server that has gone.
                    return false;
                }
                self.sent_configures.retain(|&(o, s)| o != key.0 || s > key.1);
            }
            _ => {}
        }
        if let Some(slot) = slot(iface, name)
            && let Ok(m) = msg.dup()
        {
            let o = self.objs.get_mut(&msg.id).unwrap();
            o.slots.retain(|(s, _)| *s != slot);
            o.slots.push((slot, m));
        }
        true
    }

    /// How to make a new buffer again.
    fn buffer_recipe(&self, msg: &Msg) -> Extra {
        let parent = &self.objs[&msg.id];
        let mut recipe = Vec::new();
        match parent.iface.name {
            "wl_shm_pool" => {
                let (Some(create), Extra::Pool { size }) = (&parent.create, &parent.x) else { return Extra::None };
                let Some(Arg::Fd(fd)) = create.args.get(1) else { return Extra::None };
                let Ok(fd) = fd.try_clone() else { return Extra::None };
                let Ok(buffer) = msg.dup() else { return Extra::None };
                let shm = &self.objs.get(&create.id);
                let Some(shm) = shm else { return Extra::None };
                recipe.push(Msg::new(create.id, request(shm.iface, "create_pool"), vec![Arg::NewId(TEMP), Arg::Fd(fd), Arg::Int(*size)]));
                recipe.push(Msg { id: TEMP, ..buffer });
                recipe.push(Msg::new(TEMP, request(parent.iface, "destroy"), vec![]));
            }
            "zwp_linux_buffer_params_v1" => {
                let (Some(create), Extra::Params { adds, .. }) = (&parent.create, &parent.x) else { return Extra::None };
                let Some(dmabuf) = self.objs.get(&create.id) else { return Extra::None };
                recipe.push(Msg::new(create.id, request(dmabuf.iface, "create_params"), vec![Arg::NewId(TEMP)]));
                for add in adds {
                    let Ok(add) = add.dup() else { return Extra::None };
                    recipe.push(Msg { id: TEMP, ..add });
                }
                let Ok(buffer) = msg.dup() else { return Extra::None };
                recipe.push(Msg { id: TEMP, ..buffer });
                recipe.push(Msg::new(TEMP, request(parent.iface, "destroy"), vec![]));
            }
            _ => {
                let Ok(m) = msg.dup() else { return Extra::None };
                recipe.push(m);
            }
        }
        Extra::Buffer { recipe, busy: false, via: parent.parent }
    }

    fn surface(&mut self, id: u32) -> Option<&mut Surface> {
        match self.objs.get_mut(&id).map(|o| &mut o.x) {
            Some(Extra::Surface(s)) => Some(s),
            _ => None,
        }
    }

    /// The program destroyed `id` (passed on).
    fn destroyed(&mut self, id: u32) {
        self.unlink(id);
        if id >= SERVER_START {
            if let Some(up) = self.s_up.remove(&id) {
                self.s_down.remove(&up);
                let o = &self.objs[&id];
                self.graveyard.insert(up, (o.iface, o.version));
            }
            self.objs.remove(&id);
        } else if let Some(o) = self.objs.get_mut(&id) {
            o.destroyed = true;
            o.slots.clear();
            o.create = None;
        }
    }

    /// Undo an object's links to others as it goes.
    fn unlink(&mut self, id: u32) {
        let x = match self.objs.get_mut(&id) {
            Some(o) => std::mem::take(&mut o.x),
            None => return,
        };
        match x {
            Extra::XdgSurface { surface, .. } => {
                if let Some(s) = self.surface(surface) {
                    s.xdg = None;
                }
            }
            Extra::Toplevel { xdg } | Extra::Popup { xdg } => {
                if let Some(Extra::XdgSurface { role, .. }) = self.objs.get_mut(&xdg).map(|o| &mut o.x) {
                    *role = None;
                }
            }
            Extra::Layer { surface } => {
                if let Some(s) = self.surface(surface) {
                    s.layer = None;
                }
            }
            Extra::Subsurface { surface, parent } => {
                if let Some(s) = self.surface(surface) {
                    s.sub = None;
                }
                if let Some(p) = self.surface(parent) {
                    p.stack.retain(|&s| s != surface);
                }
            }
            Extra::Surface(s) => {
                // Its subsurfaces lose their parent.
                for child in s.stack.iter().filter(|&&c| c != id) {
                    if let Some(c) = self.surface(*child) {
                        c.sub = None;
                    }
                }
            }
            _ => {}
        }
    }

    /// The program destroyed a zombie, which app_server never had: answer
    /// for it.
    fn forget(&mut self, id: u32, by_client: bool) {
        self.unlink(id);
        self.objs.remove(&id);
        if id < SERVER_START && by_client {
            self.send_down(Msg::new(1, event(ifs::display(), "delete_id"), vec![Arg::Uint(id)]));
        }
    }

    /// `id` can't be made again; tell the program what that means for it.
    fn zombify(&mut self, id: u32) {
        let Some(o) = self.objs.get_mut(&id) else { return };
        if o.zombie {
            return;
        }
        o.zombie = true;
        let iface = o.iface;
        let popup = match o.x {
            Extra::Popup { xdg } => Some(xdg),
            _ => None,
        };
        let token_pending = matches!(o.x, Extra::Token { done: false });
        let ev = |name: &str, args: Vec<Arg>| Msg::new(id, event(iface, name), args);
        let msg = match iface.name {
            "xdg_popup" => Some(ev("popup_done", vec![])),
            "wl_data_source" | "zwp_primary_selection_source_v1" | "ext_data_control_source_v1" | "zwlr_data_control_source_v1" => {
                Some(ev("cancelled", vec![]))
            }
            "zwp_locked_pointer_v1" => Some(ev("unlocked", vec![])),
            // A lease is the old app_server's; tablets, tools and pads come
            // again (announced anew) from the next.
            "wp_drm_lease_v1" => Some(ev("finished", vec![])),
            "zwp_tablet_v2" | "zwp_tablet_tool_v2" | "zwp_tablet_pad_v2" => Some(ev("removed", vec![])),
            "zwp_confined_pointer_v1" => Some(ev("unconfined", vec![])),
            "zxdg_imported_v2" => Some(ev("destroyed", vec![])),
            "xdg_activation_token_v1" if token_pending => Some(ev("done", vec![Arg::Str(Some(Vec::new()))])),
            _ => None,
        };
        if let Some(xdg) = popup {
            self.zombify(xdg);
        }
        if let Some(m) = msg {
            self.send_down(m);
        }
    }

    fn send_up(&mut self, mut msg: Msg) {
        if self.phase != Phase::Live && debug() {
            let o = self.objs.get(&msg.id);
            let (iface, name) = match self.temps.get(&msg.id) {
                Some((i, _)) => (i.name, i.requests.get(msg.op as usize).map_or("?", |d| d.name)),
                None => o.map_or(("?", "?"), |o| (o.iface.name, o.iface.requests.get(msg.op as usize).map_or("?", |d| d.name))),
            };
            eprintln!("olo-wlproxy: replay {iface}@{}.{name} {:?}", msg.id, msg.args);
        }
        // The program's ids for app_server's objects become app_server's.
        if msg.id >= SERVER_START {
            msg.id = self.s_up.get(&msg.id).copied().unwrap_or(msg.id);
        }
        for a in &mut msg.args {
            if let Arg::Object(o) = a
                && *o >= SERVER_START
            {
                *o = self.s_up.get(o).copied().unwrap_or(0);
            }
        }
        if let Some(up) = &mut self.up {
            up.send(msg);
        }
    }

    fn send_down(&mut self, msg: Msg) {
        self.down.send(msg);
    }

    // ---- from app_server --------------------------------------------------

    pub fn read_server(&mut self) -> Res {
        if self.phase == Phase::Hello {
            let Some(reply) = self.up.as_mut().and_then(|u| u.take_raw(HELLO_LEN)) else { return Ok(()) };
            if &reply[..4] != HELLO {
                return Err("app_server didn't answer our hello".into());
            }
            let pid = i32::from_ne_bytes(reply[8..12].try_into().unwrap());
            if self.old_server != 0 && pid == self.old_server {
                // The old app_server, still on its way out: try again.
                self.up = None;
                self.phase = Phase::Down;
                return Ok(());
            }
            self.server = pid;
            if self.old_server == 0 {
                self.phase = Phase::Live;
                self.down_since = None;
                self.pump();
            } else {
                eprintln!("olo-wlproxy: replaying pid {}'s {} objects on app_server {pid}", self.pid, self.objs.len());
                self.replay_start();
            }
        }
        loop {
            let Some(up) = self.up.as_mut() else { return Ok(()) };
            let Some((id, op, len)) = up.peek()? else { return Ok(()) };
            let body = up.inbuf[8..len].to_vec();
            up.inbuf.drain(..len);
            // Ours?
            if let Some((iface, _)) = self.temps.get(&id) {
                let desc = iface.events.get(op as usize).ok_or("bad event")?;
                decode(id, op, &body, desc.signature, &mut self.up.as_mut().unwrap().infds)?;
                if desc.name == "done"
                    && let Some((_, Temp::Barrier(phase))) = self.temps.get(&id)
                {
                    let phase = *phase;
                    self.barrier(phase);
                }
                continue;
            }
            let (cid, iface, version) = if id >= SERVER_START {
                match self.s_down.get(&id) {
                    Some(c) => {
                        let o = &self.objs[c];
                        (*c, o.iface, o.version)
                    }
                    None => match self.graveyard.get(&id) {
                        Some((i, v)) => (0, *i, *v),
                        None => return Err(format!("event from unknown object {id}")),
                    },
                }
            } else {
                match self.objs.get(&id) {
                    Some(o) => (id, o.iface, o.version),
                    None => return Err(format!("event from unknown object {id}")),
                }
            };
            let desc = iface.events.get(op as usize).ok_or_else(|| format!("{}: no event {op}", iface.name))?;
            let mut msg = decode(id, op, &body, desc.signature, &mut self.up.as_mut().unwrap().infds)?;
            if cid == 0 {
                continue;
            }
            msg.id = cid;
            if !self.event(&mut msg, iface, version, desc.name, desc.child_interface) {
                continue;
            }
            if desc.is_destructor {
                if cid >= SERVER_START {
                    self.destroyed(cid);
                } else if let Some(o) = self.objs.get_mut(&cid) {
                    o.destroyed = true;
                }
            }
            self.send_down(msg);
        }
    }

    /// Note what an event changes and make it the program's; false to drop
    /// it.
    fn event(&mut self, msg: &mut Msg, iface: &'static Interface, version: u32, name: &str, child: Option<&'static Interface>) -> bool {
        // Objects app_server makes get ids of the program's.
        for i in 0..msg.args.len() {
            match msg.args[i] {
                Arg::NewId(up) => {
                    let Some(child) = child else { return false };
                    let c = self.server_id(up);
                    self.seq += 1;
                    let mut o = Obj::new(child, version, self.seq);
                    o.unsent = false;
                    if child.name == "wl_buffer" {
                        o.x = Extra::Buffer { recipe: Vec::new(), busy: false, via: None };
                    }
                    self.objs.insert(c, o);
                    self.graveyard.remove(&up);
                    msg.args[i] = Arg::NewId(c);
                }
                Arg::Object(o) if o >= SERVER_START => {
                    msg.args[i] = Arg::Object(self.s_down.get(&o).copied().unwrap_or(0));
                }
                _ => {}
            }
        }
        match (iface.name, name) {
            ("wl_display", "error") => {
                self.errored = true;
                let iface = self.objs.get(&msg.uint(0)).map_or("?", |o| o.iface.name);
                eprintln!("olo-wlproxy: pid {}: app_server's error on {iface}@{}: {} {}", self.pid, msg.uint(0), msg.uint(1), String::from_utf8_lossy(msg.str(2)));
            }
            ("wl_display", "delete_id") => {
                let id = msg.uint(0);
                if self.temps.remove(&id).is_some() {
                    return false;
                }
                self.objs.remove(&id);
            }
            ("wl_registry", "global") => {
                let (up, version) = (msg.uint(0), msg.uint(2));
                let Some(known) = ifs::global(msg.str(1)) else { return false };
                let version = version.min(known.version);
                if matches!(self.phase, Phase::Globals) {
                    if !self.fresh.iter().any(|f| f.0 == up) {
                        self.fresh.push((up, known, version));
                    }
                    return false;
                }
                let name = match self.globals.iter().find(|g| g.up == Some(up)) {
                    Some(g) => g.name,
                    None => {
                        let name = self.global_name(up);
                        self.globals.push(Global { name, iface: known, version, up: Some(up) });
                        name
                    }
                };
                msg.args[0] = Arg::Uint(name);
                msg.args[2] = Arg::Uint(version);
            }
            ("wl_registry", "global_remove") => {
                if matches!(self.phase, Phase::Globals) {
                    self.fresh.retain(|f| f.0 != msg.uint(0));
                    return false;
                }
                let Some(g) = self.globals.iter_mut().find(|g| g.up == Some(msg.uint(0))) else { return false };
                g.up = None;
                msg.args[0] = Arg::Uint(g.name);
            }
            ("wl_buffer", "release") => {
                if let Some(Extra::Buffer { busy, .. }) = self.objs.get_mut(&msg.id).map(|o| &mut o.x) {
                    *busy = false;
                }
            }
            ("wl_pointer" | "wl_keyboard", "enter") => {
                self.focus.insert(msg.id, msg.uint(1));
            }
            ("zwp_text_input_v3", "enter") => {
                self.focus.insert(msg.id, msg.uint(0));
            }
            ("wl_data_device", "enter") => {
                self.focus.insert(msg.id, msg.uint(1));
            }
            ("wl_pointer" | "wl_keyboard" | "zwp_text_input_v3" | "wl_data_device", "leave") | ("wl_data_device", "drop") => {
                self.focus.remove(&msg.id);
            }
            ("xdg_surface", "configure") | ("zwlr_layer_surface_v1", "configure") => {
                let key = (msg.id, msg.uint(0));
                if self.phase == Phase::Configure {
                    // Acknowledged now, so the contents can follow; the
                    // program's own acknowledgement is dropped.
                    let ack = Msg::new(msg.id, request(iface, "ack_configure"), vec![Arg::Uint(key.1)]);
                    self.send_up(ack);
                    self.acked.insert(key);
                } else {
                    self.sent_configures.insert(key);
                }
            }
            ("xdg_activation_token_v1", "done") => {
                if let Some(Extra::Token { done }) = self.objs.get_mut(&msg.id).map(|o| &mut o.x) {
                    *done = true;
                }
            }
            _ => {}
        }
        true
    }

    /// The program's id for a new object of app_server's: the same, unless
    /// the program still has that id (from an app_server before).
    fn server_id(&mut self, up: u32) -> u32 {
        let free = |c: &Client, id: u32| !c.objs.contains_key(&id) && !c.s_up.contains_key(&id);
        let id = if free(self, up) { up } else { (SERVER_START..).find(|&id| free(self, id)).unwrap() };
        self.s_up.insert(id, up);
        self.s_down.insert(up, id);
        id
    }

    fn global_name(&self, up: u32) -> u32 {
        if self.globals.iter().all(|g| g.name != up) { up } else { self.globals.iter().map(|g| g.name).max().unwrap_or(0) + 1 }
    }

    // ---- app_server went away ---------------------------------------------

    /// The connection to app_server ended. True if the program can wait
    /// for the next app_server; false if it must end too.
    pub fn lost(&mut self) -> bool {
        self.up = None;
        if self.errored {
            return false;
        }
        if self.phase == Phase::Down {
            return true;
        }
        let replaying = self.phase != Phase::Live;
        self.phase = Phase::Down;
        self.down_since = Some(Instant::now());
        self.last_try = Some(Instant::now());
        if self.server != 0 {
            self.old_server = self.server;
        }
        self.temps.clear();
        self.acked.clear();
        self.sent_configures.clear();
        self.graveyard.clear();
        self.s_up.clear();
        self.s_down.clear();
        self.fresh.clear();
        if replaying {
            // Lost mid-replay: the program was already told.
            return true;
        }
        // Input leaves.
        let focus: Vec<(u32, u32)> = self.focus.drain().collect();
        for (dev, surface) in focus {
            let Some(o) = self.objs.get(&dev) else { continue };
            let (iface, version) = (o.iface, o.version);
            match iface.name {
                "wl_pointer" => {
                    self.send_down(Msg::new(dev, event(iface, "leave"), vec![Arg::Uint(0), Arg::Object(surface)]));
                    if version >= 5 {
                        self.send_down(Msg::new(dev, event(iface, "frame"), vec![]));
                    }
                }
                "wl_keyboard" => self.send_down(Msg::new(dev, event(iface, "leave"), vec![Arg::Uint(0), Arg::Object(surface)])),
                "zwp_text_input_v3" => self.send_down(Msg::new(dev, event(iface, "leave"), vec![Arg::Object(surface)])),
                "wl_data_device" => self.send_down(Msg::new(dev, event(iface, "leave"), vec![])),
                _ => {}
            }
        }
        let current: HashSet<u32> = self
            .objs
            .values()
            .filter_map(|o| match &o.x {
                Extra::Surface(s) if !o.zombie && !o.destroyed => s.current.buffer,
                _ => None,
            })
            .collect();
        let time = self.started.elapsed().as_millis() as u32;
        let mut ids: Vec<u32> = self.objs.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            let Some(o) = self.objs.get_mut(&id) else { continue };
            if o.unsent || o.zombie {
                continue;
            }
            let iface = o.iface;
            let display = ifs::display();
            if o.destroyed {
                // app_server would have freed the id.
                self.objs.remove(&id);
                self.send_down(Msg::new(1, event(display, "delete_id"), vec![Arg::Uint(id)]));
                continue;
            }
            match iface.name {
                "wl_callback" => {
                    self.objs.remove(&id);
                    self.send_down(Msg::new(id, event(iface, "done"), vec![Arg::Uint(time)]));
                    self.send_down(Msg::new(1, event(display, "delete_id"), vec![Arg::Uint(id)]));
                }
                "wp_presentation_feedback" => {
                    self.objs.remove(&id);
                    self.send_down(Msg::new(id, event(iface, "discarded"), vec![]));
                    self.send_down(Msg::new(1, event(display, "delete_id"), vec![Arg::Uint(id)]));
                }
                "wl_buffer" => {
                    let Extra::Buffer { busy, .. } = &mut o.x else { continue };
                    if *busy && !current.contains(&id) {
                        *busy = false;
                        self.send_down(Msg::new(id, event(iface, "release"), vec![]));
                    }
                    if id >= SERVER_START {
                        self.zombify(id);
                    }
                }
                "xdg_popup" | "xdg_positioner" | "wl_data_source" | "zwp_primary_selection_source_v1" | "ext_data_control_source_v1"
                | "zwlr_data_control_source_v1" | "zwp_locked_pointer_v1" | "zwp_confined_pointer_v1" | "zxdg_exported_v2" | "zxdg_imported_v2"
                | "xdg_activation_token_v1" | "wp_drm_lease_v1" | "wp_drm_lease_request_v1" => self.zombify(id),
                _ if id >= SERVER_START => self.zombify(id),
                _ => {}
            }
        }
        true
    }

    // ---- replaying on a new app_server ------------------------------------

    fn temp(&mut self, iface: &'static Interface, kind: Temp) -> u32 {
        let id = self.next_temp;
        self.next_temp += 1;
        self.temps.insert(id, (iface, kind));
        id
    }

    fn callback_iface() -> &'static Interface {
        ifs::display().requests[0].child_interface.unwrap()
    }

    fn barrier_now(&mut self, phase: Phase) {
        let id = self.temp(Self::callback_iface(), Temp::Barrier(phase));
        self.send_up(Msg::new(1, request(ifs::display(), "sync"), vec![Arg::NewId(id)]));
    }

    fn replay_start(&mut self) {
        // Every id the program has used is made and freed, so the ids it
        // holds can be made again in any order (libwayland takes a new id
        // only up to one past the highest yet).
        for id in 2..=self.max_id {
            self.temps.insert(id, (Self::callback_iface(), Temp::Filler));
            self.send_up(Msg::new(1, request(ifs::display(), "sync"), vec![Arg::NewId(id)]));
        }
        self.next_temp = self.max_id + 1;
        let registries: Vec<u32> = self.objs.iter().filter(|(_, o)| o.iface.name == "wl_registry" && o.live()).map(|(id, _)| *id).collect();
        for id in registries {
            self.send_up(Msg::new(1, request(ifs::display(), "get_registry"), vec![Arg::NewId(id)]));
        }
        self.phase = Phase::Globals;
        self.fresh.clear();
        self.barrier_now(Phase::Globals);
    }

    fn barrier(&mut self, phase: Phase) {
        match phase {
            Phase::Globals => self.replay_objects(),
            Phase::Configure => self.replay_contents(),
            _ => {}
        }
    }

    fn registries(&self) -> Vec<u32> {
        self.objs.iter().filter(|(_, o)| o.iface.name == "wl_registry" && o.live()).map(|(id, _)| *id).collect()
    }

    /// Match the new app_server's globals to the ones the program knows
    /// (same interface, in order), tell it of any that came or went, and
    /// make every object again.
    fn replay_objects(&mut self) {
        let mut fresh = std::mem::take(&mut self.fresh);
        fresh.sort_by_key(|f| f.0);
        let mut olds: Vec<usize> = (0..self.globals.len()).filter(|&i| self.globals[i].up.is_some()).collect();
        olds.sort_by_key(|&i| self.globals[i].name);
        for g in &mut self.globals {
            g.up = None;
        }
        let registries = self.registries();
        let mut added = Vec::new();
        for (up, iface, version) in fresh {
            match olds.iter().position(|&i| std::ptr::eq(self.globals[i].iface, iface)) {
                Some(at) => {
                    let i = olds.remove(at);
                    self.globals[i].up = Some(up);
                    self.globals[i].version = version;
                }
                None => added.push((up, iface, version)),
            }
        }
        let reg = ifs::display().requests[1].child_interface.unwrap();
        for i in olds {
            let name = self.globals[i].name;
            for &r in &registries {
                self.send_down(Msg::new(r, event(reg, "global_remove"), vec![Arg::Uint(name)]));
            }
        }
        for (up, iface, version) in added {
            let name = self.global_name(up);
            self.globals.push(Global { name, iface, version, up: Some(up) });
            for &r in &registries {
                self.send_down(Msg::new(r, event(reg, "global"), vec![Arg::Uint(name), Arg::Str(Some(iface.name.as_bytes().to_vec())), Arg::Uint(version)]));
            }
        }

        let mut order: Vec<(u64, u32)> = self
            .objs
            .iter()
            .filter(|(id, o)| **id > 1 && **id < SERVER_START && o.live() && o.iface.name != "wl_registry")
            .map(|(id, o)| (o.seq, *id))
            .collect();
        order.sort_unstable();
        for &(_, id) in &order {
            self.replay_create(id);
        }
        for &(_, id) in &order {
            let o = &self.objs[&id];
            if o.zombie {
                continue;
            }
            let mut msgs: Vec<Msg> = o.slots.iter().filter_map(|(_, m)| m.dup().ok()).collect();
            match &o.x {
                Extra::Params { adds, used: false } => msgs.extend(adds.iter().filter_map(|m| m.dup().ok())),
                Extra::Region(rects) => {
                    for &(add, [x, y, w, h]) in rects {
                        let op = request(o.iface, if add { "add" } else { "subtract" });
                        msgs.push(Msg::new(id, op, vec![Arg::Int(x), Arg::Int(y), Arg::Int(w), Arg::Int(h)]));
                    }
                }
                _ => {}
            }
            for m in msgs {
                self.replay_msg(m);
            }
        }
        // Each window's role, committed without contents for its configure.
        let roles: Vec<u32> = order
            .iter()
            .filter(|(_, id)| {
                let o = &self.objs[id];
                match &o.x {
                    Extra::Surface(s) if !o.zombie => {
                        s.layer.is_some_and(|l| !self.is_zombie(l)) || s.xdg.and_then(|x| self.role(x)).is_some_and(|r| self.objs[&r].iface.name == "xdg_toplevel")
                    }
                    _ => false,
                }
            })
            .map(|(_, id)| *id)
            .collect();
        for id in roles {
            let iface = self.objs[&id].iface;
            self.send_up(Msg::new(id, request(iface, "commit"), vec![]));
        }
        self.phase = Phase::Configure;
        self.barrier_now(Phase::Configure);
    }

    /// The live role (toplevel or popup) of an xdg_surface.
    fn role(&self, xdg: u32) -> Option<u32> {
        match self.objs.get(&xdg) {
            Some(o) if !o.zombie => match o.x {
                Extra::XdgSurface { role: Some(r), .. } if !self.is_zombie(r) => Some(r),
                _ => None,
            },
            _ => None,
        }
    }

    /// Make one object again (or give up on it).
    fn replay_create(&mut self, id: u32) {
        let o = &self.objs[&id];
        let (mut recipe, parent): (Vec<Msg>, Option<Parent>) = match (&o.x, &o.create) {
            (Extra::Buffer { recipe, via, .. }, _) if !recipe.is_empty() => (recipe.iter().filter_map(|m| m.dup().ok()).collect(), via.or(o.parent)),
            (_, Some(create)) => (create.dup().into_iter().collect(), o.parent),
            _ => (Vec::new(), None),
        };
        let (Some(parent), false) = (parent, recipe.is_empty()) else {
            self.zombify(id);
            return;
        };
        let mut temps = Vec::new();
        if recipe.iter().any(|m| m.args.iter().any(|a| matches!(a, Arg::NewId(TEMP)))) {
            temps.push(self.next_temp);
            self.next_temp += 1;
        }
        for m in &mut recipe {
            if m.id == TEMP {
                m.id = temps[0];
            }
            for a in &mut m.args {
                if let Arg::NewId(n) | Arg::Object(n) = a
                    && *n == TEMP
                {
                    *n = temps[0];
                }
            }
        }
        // What it was made from: still there, or another of its kind.
        let registry = self.registries().into_iter().next();
        let mut before = Vec::new();
        let mut after = Vec::new();
        let same = self.objs.get(&parent.id).is_some_and(|p| p.live() && p.seq == parent.seq);
        let target = if parent.iface.name == "wl_registry" {
            registry
        } else if same {
            Some(parent.id)
        } else if let (Some(g), Some(r), Some(destroy)) = (parent.global, registry, parent.iface.requests.iter().position(|d| d.is_destructor && d.signature.is_empty())) {
            let t = self.next_temp;
            self.next_temp += 1;
            temps.push(t);
            let reg = self.objs[&r].iface;
            before.push(Msg::new(r, request(reg, "bind"), vec![Arg::Uint(g), Arg::Str(Some(parent.iface.name.as_bytes().to_vec())), Arg::Uint(parent.version), Arg::NewId(t)]));
            after.push(Msg::new(t, destroy as u16, vec![]));
            self.temps.insert(t, (parent.iface, Temp::Other));
            Some(t)
        } else {
            None
        };
        let Some(target) = target else {
            self.zombify(id);
            return;
        };
        if recipe[0].id == parent.id {
            recipe[0].id = target;
        }
        if let Some(&t) = temps.first()
            && recipe[0].new_id() == Some(t)
            && let Some(iface) = parent.iface.requests.get(recipe[0].op as usize).and_then(|d| d.child_interface)
        {
            // The recipe's pool or params, freed by its last step.
            self.temps.insert(t, (iface, Temp::Other));
        }
        // Check every step before sending any.
        let made = self.objs[&id].seq;
        let mut out = Vec::new();
        for m in before.into_iter().chain(recipe).chain(after) {
            match self.prepare(m, &temps, Some(made)) {
                Some(m) => out.push(m),
                None => {
                    for t in &temps {
                        self.temps.remove(t);
                    }
                    self.zombify(id);
                    return;
                }
            }
        }
        for m in out {
            self.send_up(m);
        }
    }

    /// A recorded request made fit for the new app_server: globals renamed,
    /// gone objects left out (`None` if it can't be sent). `made`: when
    /// the object it makes was made; an object it names that was made
    /// after that has the id of one that's gone (a fractional scale or
    /// viewport the program never destroyed names its destroyed surface,
    /// whose id went to something else).
    fn prepare(&self, mut m: Msg, temps: &[u32], made: Option<u64>) -> Option<Msg> {
        let ours = |id: u32| temps.contains(&id);
        let gone = |id: u32| self.is_zombie(id) || made.is_some_and(|made| self.objs.get(&id).is_some_and(|o| o.seq > made));
        if !ours(m.id) && self.is_zombie(m.id) {
            return None;
        }
        let iface = if ours(m.id) { self.temps.get(&m.id).map(|t| t.0) } else { Some(self.objs[&m.id].iface) };
        if let Some(iface) = iface {
            if iface.name == "wl_registry" {
                let g = self.globals.iter().find(|g| g.name == m.uint(0))?;
                if m.uint(2) > g.version {
                    return None;
                }
                m.args[0] = Arg::Uint(g.up?);
            }
            let desc = iface.requests.get(m.op as usize)?;
            for (i, t) in desc.signature.iter().enumerate() {
                if let ArgumentType::Object(null) = t {
                    let o = m.uint(i);
                    if o != 0 && !ours(o) && gone(o) {
                        if *null == AllowNull::Yes {
                            m.args[i] = Arg::Object(0);
                        } else {
                            return None;
                        }
                    }
                }
            }
        }
        Some(m)
    }

    fn replay_msg(&mut self, m: Msg) {
        if let Some(m) = self.prepare(m, &[], None) {
            self.send_up(m);
        }
    }

    /// Commit each surface's contents again, subsurfaces before their
    /// parents, then let the program carry on.
    fn replay_contents(&mut self) {
        let compositor = self.objs.iter().find(|(_, o)| o.iface.name == "wl_compositor" && o.live()).map(|(id, o)| (*id, o.iface));
        let surfaces: Vec<u32> = self.objs.iter().filter(|(_, o)| o.live() && matches!(o.x, Extra::Surface(_))).map(|(id, _)| *id).collect();
        // Stacking first (it takes effect with the parent's commit).
        for &id in &surfaces {
            let Some(Extra::Surface(s)) = self.objs.get(&id).map(|o| &o.x) else { continue };
            let stack: Vec<u32> = s.stack.iter().copied().filter(|&c| c == id || !self.is_zombie(c)).collect();
            let Some(me) = stack.iter().position(|&c| c == id) else { continue };
            let mut steps = Vec::new();
            for i in (0..me).rev() {
                steps.push((stack[i], "place_below", stack[i + 1]));
            }
            for i in me + 1..stack.len() {
                steps.push((stack[i], "place_above", stack[i - 1]));
            }
            for (child, how, sibling) in steps {
                let sub = match self.objs.get(&child).map(|o| &o.x) {
                    Some(Extra::Surface(c)) => c.sub,
                    _ => None,
                };
                if let Some(sub) = sub.filter(|&s| !self.is_zombie(s)) {
                    let iface = self.objs[&sub].iface;
                    self.send_up(Msg::new(sub, request(iface, how), vec![Arg::Object(sibling)]));
                }
            }
        }
        let roots: Vec<u32> = surfaces
            .iter()
            .copied()
            .filter(|&id| match self.objs.get(&id).map(|o| &o.x) {
                Some(Extra::Surface(s)) => s.sub.is_none_or(|sub| self.is_zombie(sub)),
                _ => false,
            })
            .collect();
        let mut done = HashSet::new();
        for root in roots {
            self.commit_tree(root, compositor, &mut done);
        }
        self.phase = Phase::Live;
        self.down_since = None;
        self.pump();
    }

    fn commit_tree(&mut self, id: u32, compositor: Option<(u32, &'static Interface)>, done: &mut HashSet<u32>) {
        if !done.insert(id) {
            return;
        }
        let Some(Extra::Surface(s)) = self.objs.get(&id).map(|o| &o.x) else { return };
        let children: Vec<u32> = s.stack.iter().copied().filter(|&c| c != id).collect();
        for c in children {
            if !self.is_zombie(c) {
                self.commit_tree(c, compositor, done);
            }
        }
        let o = &self.objs[&id];
        let (iface, version) = (o.iface, o.version);
        let Extra::Surface(s) = &o.x else { return };
        let c = s.current.clone();
        // An xdg_surface with no role yet can't have a buffer.
        let roleless = s.xdg.is_some_and(|x| !self.is_zombie(x) && self.role(x).is_none());
        let mut out = Vec::new();
        if let Some(b) = c.buffer.filter(|&b| b != 0 && !roleless && self.objs.get(&b).is_some_and(|o| o.live())) {
            out.push(Msg::new(id, request(iface, "attach"), vec![Arg::Object(b), Arg::Int(0), Arg::Int(0)]));
            let damage = if version >= 4 { "damage_buffer" } else { "damage" };
            out.push(Msg::new(id, request(iface, damage), vec![Arg::Int(0), Arg::Int(0), Arg::Int(i32::MAX), Arg::Int(i32::MAX)]));
        }
        if let Some(v) = c.scale {
            out.push(Msg::new(id, request(iface, "set_buffer_scale"), vec![Arg::Int(v)]));
        }
        if let Some(v) = c.transform {
            out.push(Msg::new(id, request(iface, "set_buffer_transform"), vec![Arg::Int(v)]));
        }
        for m in out {
            self.send_up(m);
        }
        for (which, rects) in [("set_input_region", c.input), ("set_opaque_region", c.opaque)] {
            let Some(rects) = rects else { continue };
            match (rects, compositor) {
                (None, _) => self.send_up(Msg::new(id, request(iface, which), vec![Arg::Object(0)])),
                (Some(rects), Some((comp, comp_iface))) => {
                    let region_iface = comp_iface.requests[request(comp_iface, "create_region") as usize].child_interface.unwrap();
                    let r = self.temp(region_iface, Temp::Other);
                    self.send_up(Msg::new(comp, request(comp_iface, "create_region"), vec![Arg::NewId(r)]));
                    for (add, [x, y, w, h]) in rects {
                        let op = request(region_iface, if add { "add" } else { "subtract" });
                        self.send_up(Msg::new(r, op, vec![Arg::Int(x), Arg::Int(y), Arg::Int(w), Arg::Int(h)]));
                    }
                    self.send_up(Msg::new(id, request(iface, which), vec![Arg::Object(r)]));
                    self.send_up(Msg::new(r, request(region_iface, "destroy"), vec![]));
                }
                _ => {}
            }
        }
        self.send_up(Msg::new(id, request(iface, "commit"), vec![]));
        if let Some(b) = c.buffer.filter(|&b| b != 0)
            && let Some(Extra::Buffer { busy, .. }) = self.objs.get_mut(&b).map(|o| &mut o.x)
        {
            *busy = true;
        }
    }
}

fn debug() -> bool {
    static DEBUG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DEBUG.get_or_init(|| std::env::var_os("OLO_WLPROXY_DEBUG").is_some())
}

/// Setters whose latest call holds until changed: (interface, request)
/// → the state it sets.
fn slot(iface: &str, req: &'static str) -> Option<&'static str> {
    Some(match (iface, req) {
        ("xdg_toplevel", "set_maximized" | "unset_maximized") => "maximized",
        ("xdg_toplevel", "set_fullscreen" | "unset_fullscreen") => "fullscreen",
        ("xdg_toplevel", "set_parent" | "set_title" | "set_app_id" | "set_max_size" | "set_min_size") => req,
        ("xdg_surface", "set_window_geometry") => req,
        ("wp_viewport", "set_source" | "set_destination") => req,
        ("zxdg_toplevel_decoration_v1", "set_mode" | "unset_mode") => "mode",
        ("wl_subsurface", "set_position") => req,
        ("wl_subsurface", "set_sync" | "set_desync") => "sync",
        ("wl_shm_pool", "resize") => req,
        ("zwlr_layer_surface_v1", "set_size" | "set_anchor" | "set_exclusive_zone" | "set_margin" | "set_keyboard_interactivity" | "set_layer" | "set_exclusive_edge") => req,
        ("org_kde_kwin_server_decoration", "request_mode") => req,
        ("xdg_dialog_v1", "set_modal" | "unset_modal") => "modal",
        ("wp_alpha_modifier_surface_v1", "set_multiplier") => req,
        ("wp_content_type_v1", "set_content_type") => req,
        ("zwp_virtual_keyboard_v1", "keymap") => req,
        _ => return None,
    })
}

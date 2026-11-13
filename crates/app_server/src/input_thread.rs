//! The input thread: libinput read, and the hardware cursor moved, on a
//! thread of its own (Preferences ▸ Performance ▸ Scheduler, or
//! `OLO_INPUT_THREAD=1`; off while it's tried, HACKING "The pointer on a
//! thread of its own").
//!
//! The pointer is the one thing that must never stop. app_server's loop
//! does everything else too (clients' requests, every display's frames),
//! and under load it was seen held for 78 and 142 ms in the GPU driver,
//! waiting for a buffer's lock (`drm_exec_lock_contended`, 2026-11-12),
//! with the pointer standing still meanwhile. So libinput lives here, at
//! the highest nice rtkit gives: its events are read as they come, made
//! into owned events ([`Threaded`], smithay's `InputBackend`), and sent to
//! the loop, a batch a read, which handles them as it always did. While
//! the loop is held (a motion it hasn't handled after [`TAKE_OVER`]), this
//! thread moves the cursor plane itself, with the kernel's legacy cursor
//! move (the plane's buffer as it is; the CRTC's locks, none of the
//! GPU's), to where the loop will put the pointer: where it last was and
//! the motion since, on the display it's on. The loop catches up from the
//! same events and draws it there itself. (As mutter's input and KMS
//! threads.)
//!
//! Nothing of libinput crosses threads (its context and devices aren't
//! `Send`): the loop gets snapshots of the devices ([`Dev`]) and sends
//! [`Command`]s (settings, the keyboards' lights, a restart for the touch
//! filter, the session paused or resumed). Devices are opened by the
//! session, which is the loop's: this thread asks (`contacts::Request`)
//! and waits for the answer; the loop never waits on this thread.

use std::{
    collections::{HashMap, VecDeque},
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        drm::DrmDeviceFd,
        input::{
            self as backend, Axis, AxisRelativeDirection, AxisSource, ButtonState, InputBackend, InputEvent, InputTime, KeyState, Keycode, PointerMotionEvent as _, ProximityState, Switch,
            SwitchState, TabletToolDescriptor, TabletToolTipState, TouchSlot, UnusedEvent,
        },
        libinput::LibinputInputBackend,
    },
    reexports::{
        calloop::{self, EventLoop, RegistrationToken, channel},
        drm::control::{Device as _, crtc},
        input::{self as libinput, Libinput},
    },
    utils::{Logical, Physical, Point, Rectangle, Size, Transform},
};
use tracing::{debug, info, warn};

/// How long a motion may wait for the loop before this thread moves the
/// cursor itself: past a frame at 120 Hz and the odd slow one, short of
/// what's felt (a pointer late by 20 ms is).
pub const TAKE_OVER: Duration = Duration::from_millis(12);

/// How often this thread moves the cursor at most: the plane changes on
/// screen at a vblank, so a mouse's 1000 reports a second needn't be 1000
/// moves (a frame at 240 Hz). The newest place goes when it's time.
pub const MOVE_EVERY: Duration = Duration::from_millis(4);

/// Whether the input thread is wanted: Preferences ▸ Performance's
/// switch (`Settings::input_thread`), unless `OLO_INPUT_THREAD` says (1
/// or 0).
pub fn wanted(settings: Option<&olo_protocol::Settings>) -> bool {
    match std::env::var("OLO_INPUT_THREAD").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => settings.is_some_and(|s| s.input_thread),
    }
}

// ---------------------------------------------------------------------
// Devices and events, owned.

/// What the loop knows of an input device: read once, when it's added.
#[derive(Debug)]
pub struct DevInfo {
    /// Unique for the life of app_server (a restart's devices are new).
    pub id: u64,
    pub sysname: String,
    pub name: String,
    pub syspath: Option<PathBuf>,
    pub vendor: u32,
    pub product: u32,
    caps: u8,
    /// Its size in mm (a touchpad's, a tablet's).
    pub size: Option<(f64, f64)>,
    /// How many fingers it taps with (a touchpad: more than none).
    pub tap_fingers: u32,
    /// libinput scrolls it with a button held (a pointing stick).
    pub scroll_on_button: bool,
    /// It has an A key (a keyboard to type on).
    pub has_key_a: bool,
}

impl DevInfo {
    /// The snapshot of a libinput device.
    pub fn of(device: &libinput::Device, id: u64) -> DevInfo {
        use smithay::backend::input::{Device as _, DeviceCapability as C};
        let caps =
            [C::Keyboard, C::Pointer, C::Touch, C::TabletTool, C::TabletPad, C::Gesture, C::Switch].into_iter().filter(|c| backend::Device::has_capability(device, *c)).fold(0, |a, c| a | cap_bit(c));
        DevInfo {
            id,
            sysname: device.sysname().to_owned(),
            name: device.name().into_owned(),
            syspath: device.syspath(),
            vendor: device.id_vendor(),
            product: device.id_product(),
            caps,
            size: device.size(),
            tap_fingers: device.config_tap_finger_count(),
            scroll_on_button: device.config_scroll_default_method() == Some(libinput::ScrollMethod::OnButtonDown),
            has_key_a: device.keyboard_has_key(30) == Ok(true),
        }
    }

    pub fn has(&self, capability: backend::DeviceCapability) -> bool {
        self.caps & cap_bit(capability) != 0
    }
}

fn cap_bit(c: backend::DeviceCapability) -> u8 {
    use backend::DeviceCapability as C;
    match c {
        C::Keyboard => 1,
        C::Pointer => 2,
        C::Touch => 4,
        C::TabletTool => 8,
        C::TabletPad => 16,
        C::Gesture => 32,
        C::Switch => 64,
    }
}

/// An input device, as the loop has it.
#[derive(Debug, Clone)]
pub struct Dev(pub Arc<DevInfo>);

impl std::ops::Deref for Dev {
    type Target = DevInfo;
    fn deref(&self) -> &DevInfo {
        &self.0
    }
}

impl PartialEq for Dev {
    fn eq(&self, other: &Dev) -> bool {
        self.0.id == other.0.id
    }
}
impl Eq for Dev {}
impl Hash for Dev {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.id.hash(state)
    }
}

impl backend::Device for Dev {
    fn id(&self) -> String {
        self.sysname.clone()
    }
    fn name(&self) -> String {
        self.0.name.clone()
    }
    fn has_capability(&self, capability: backend::DeviceCapability) -> bool {
        self.has(capability)
    }
    // (Product, vendor: as smithay's libinput backend has it.)
    fn usb_id(&self) -> Option<(u32, u32)> {
        Some((self.product, self.vendor))
    }
    fn syspath(&self) -> Option<PathBuf> {
        self.0.syspath.clone()
    }
}

/// An event: when the kernel stamped it, its device, and what it says.
#[derive(Debug)]
pub struct Ev<T> {
    pub time: u64,
    pub device: Dev,
    pub data: T,
}

impl<T> backend::Event<Threaded> for Ev<T> {
    fn time(&self) -> InputTime {
        InputTime::from_micros(self.time)
    }
    fn device(&self) -> Dev {
        self.device.clone()
    }
}

#[derive(Debug)]
pub struct Key {
    pub code: Keycode,
    pub pressed: bool,
    pub count: u32,
}
impl backend::KeyboardKeyEvent<Threaded> for Ev<Key> {
    fn key_code(&self) -> Keycode {
        self.data.code
    }
    fn state(&self) -> KeyState {
        if self.data.pressed { KeyState::Pressed } else { KeyState::Released }
    }
    fn count(&self) -> u32 {
        self.data.count
    }
}

#[derive(Debug)]
pub struct Motion {
    pub dx: f64,
    pub dy: f64,
    pub ux: f64,
    pub uy: f64,
}
impl backend::PointerMotionEvent<Threaded> for Ev<Motion> {
    fn delta_x(&self) -> f64 {
        self.data.dx
    }
    fn delta_y(&self) -> f64 {
        self.data.dy
    }
    fn delta_x_unaccel(&self) -> f64 {
        self.data.ux
    }
    fn delta_y_unaccel(&self) -> f64 {
        self.data.uy
    }
}

/// A place on an absolute device: in its own units (mm), and as a
/// fraction of its area (libinput's transformed coordinates are linear in
/// the size asked for).
#[derive(Debug, Clone, Copy)]
pub struct Abs {
    pub x: f64,
    pub y: f64,
    pub fx: f64,
    pub fy: f64,
}
impl Abs {
    fn of<B: InputBackend, E: backend::AbsolutePositionEvent<B>>(e: &E) -> Abs {
        // (A size of 2^20: the fraction to 20 bits past what libinput
        // keeps of a device's range.)
        const N: i32 = 1 << 20;
        use backend::AbsolutePositionEvent as A;
        Abs { x: A::x(e), y: A::y(e), fx: A::x_transformed(e, N) / N as f64, fy: A::y_transformed(e, N) / N as f64 }
    }
}
macro_rules! absolute {
    ($t:ty, $f:ident) => {
        impl backend::AbsolutePositionEvent<Threaded> for Ev<$t> {
            fn x(&self) -> f64 {
                self.data.$f.x
            }
            fn y(&self) -> f64 {
                self.data.$f.y
            }
            fn x_transformed(&self, width: i32) -> f64 {
                self.data.$f.fx * width as f64
            }
            fn y_transformed(&self, height: i32) -> f64 {
                self.data.$f.fy * height as f64
            }
        }
    };
}

#[derive(Debug)]
pub struct Absolute {
    pub at: Abs,
}
absolute!(Absolute, at);
impl backend::PointerMotionAbsoluteEvent<Threaded> for Ev<Absolute> {}

#[derive(Debug)]
pub struct Button {
    pub code: u32,
    pub pressed: bool,
}
impl backend::PointerButtonEvent<Threaded> for Ev<Button> {
    fn button_code(&self) -> u32 {
        self.data.code
    }
    fn state(&self) -> ButtonState {
        if self.data.pressed { ButtonState::Pressed } else { ButtonState::Released }
    }
}

/// A scroll: each axis's amount (horizontal, vertical), a wheel's in
/// 120ths of a notch, and whether the device scrolls naturally.
#[derive(Debug)]
pub struct Scroll {
    pub source: AxisSource,
    pub amount: [Option<f64>; 2],
    pub v120: [Option<f64>; 2],
    pub inverted: bool,
}
fn axis_index(axis: Axis) -> usize {
    match axis {
        Axis::Horizontal => 0,
        Axis::Vertical => 1,
    }
}
impl backend::PointerAxisEvent<Threaded> for Ev<Scroll> {
    fn amount(&self, axis: Axis) -> Option<f64> {
        self.data.amount[axis_index(axis)]
    }
    fn amount_v120(&self, axis: Axis) -> Option<f64> {
        self.data.v120[axis_index(axis)]
    }
    fn source(&self) -> AxisSource {
        self.data.source
    }
    fn relative_direction(&self, _axis: Axis) -> AxisRelativeDirection {
        if self.data.inverted { AxisRelativeDirection::Inverted } else { AxisRelativeDirection::Identical }
    }
}

#[derive(Debug)]
pub struct Begin {
    pub fingers: u32,
}
impl backend::GestureBeginEvent<Threaded> for Ev<Begin> {
    fn fingers(&self) -> u32 {
        self.data.fingers
    }
}
impl backend::GestureSwipeBeginEvent<Threaded> for Ev<Begin> {}
impl backend::GesturePinchBeginEvent<Threaded> for Ev<Begin> {}
impl backend::GestureHoldBeginEvent<Threaded> for Ev<Begin> {}

#[derive(Debug)]
pub struct End {
    pub cancelled: bool,
}
impl backend::GestureEndEvent<Threaded> for Ev<End> {
    fn cancelled(&self) -> bool {
        self.data.cancelled
    }
}
impl backend::GestureSwipeEndEvent<Threaded> for Ev<End> {}
impl backend::GesturePinchEndEvent<Threaded> for Ev<End> {}
impl backend::GestureHoldEndEvent<Threaded> for Ev<End> {}

#[derive(Debug)]
pub struct Swipe {
    pub dx: f64,
    pub dy: f64,
}
impl backend::GestureSwipeUpdateEvent<Threaded> for Ev<Swipe> {
    fn delta_x(&self) -> f64 {
        self.data.dx
    }
    fn delta_y(&self) -> f64 {
        self.data.dy
    }
}

#[derive(Debug)]
pub struct Pinch {
    pub dx: f64,
    pub dy: f64,
    pub scale: f64,
    pub rotation: f64,
}
impl backend::GesturePinchUpdateEvent<Threaded> for Ev<Pinch> {
    fn delta_x(&self) -> f64 {
        self.data.dx
    }
    fn delta_y(&self) -> f64 {
        self.data.dy
    }
    fn scale(&self) -> f64 {
        self.data.scale
    }
    fn rotation(&self) -> f64 {
        self.data.rotation
    }
}

/// A finger on a touchscreen: down or moving.
#[derive(Debug)]
pub struct TouchAt {
    pub slot: TouchSlot,
    pub at: Abs,
}
absolute!(TouchAt, at);
impl backend::TouchEvent<Threaded> for Ev<TouchAt> {
    fn slot(&self) -> TouchSlot {
        self.data.slot
    }
}
impl backend::TouchDownEvent<Threaded> for Ev<TouchAt> {}
impl backend::TouchMotionEvent<Threaded> for Ev<TouchAt> {}

/// A finger lifted, or the touch cancelled.
#[derive(Debug)]
pub struct TouchOf {
    pub slot: TouchSlot,
}
impl backend::TouchEvent<Threaded> for Ev<TouchOf> {
    fn slot(&self) -> TouchSlot {
        self.data.slot
    }
}
impl backend::TouchUpEvent<Threaded> for Ev<TouchOf> {}
impl backend::TouchCancelEvent<Threaded> for Ev<TouchOf> {}

#[derive(Debug)]
pub struct TouchFrame;
impl backend::TouchFrameEvent<Threaded> for Ev<TouchFrame> {}

/// A tablet tool's event (axes, proximity, tip, button: one shape).
#[derive(Debug)]
pub struct Tablet {
    pub tool: TabletToolDescriptor,
    pub at: Abs,
    pub dx: f64,
    pub dy: f64,
    /// Each axis's value and whether it changed: distance, pressure,
    /// slider, tilt x, tilt y, rotation.
    pub axes: [(f64, bool); 6],
    pub wheel: (f64, i32, bool),
    pub near: bool,
    pub down: bool,
    pub button: (u32, u32, bool),
}
absolute!(Tablet, at);
impl backend::TabletToolEvent<Threaded> for Ev<Tablet> {
    fn tool(&self) -> TabletToolDescriptor {
        self.data.tool.clone()
    }
    fn delta_x(&self) -> f64 {
        self.data.dx
    }
    fn delta_y(&self) -> f64 {
        self.data.dy
    }
    fn distance(&self) -> f64 {
        self.data.axes[0].0
    }
    fn distance_has_changed(&self) -> bool {
        self.data.axes[0].1
    }
    fn pressure(&self) -> f64 {
        self.data.axes[1].0
    }
    fn pressure_has_changed(&self) -> bool {
        self.data.axes[1].1
    }
    fn slider_position(&self) -> f64 {
        self.data.axes[2].0
    }
    fn slider_has_changed(&self) -> bool {
        self.data.axes[2].1
    }
    fn tilt_x(&self) -> f64 {
        self.data.axes[3].0
    }
    fn tilt_x_has_changed(&self) -> bool {
        self.data.axes[3].1
    }
    fn tilt_y(&self) -> f64 {
        self.data.axes[4].0
    }
    fn tilt_y_has_changed(&self) -> bool {
        self.data.axes[4].1
    }
    fn rotation(&self) -> f64 {
        self.data.axes[5].0
    }
    fn rotation_has_changed(&self) -> bool {
        self.data.axes[5].1
    }
    fn wheel_delta(&self) -> f64 {
        self.data.wheel.0
    }
    fn wheel_delta_discrete(&self) -> i32 {
        self.data.wheel.1
    }
    fn wheel_has_changed(&self) -> bool {
        self.data.wheel.2
    }
}
impl backend::TabletToolAxisEvent<Threaded> for Ev<Tablet> {}
impl backend::TabletToolProximityEvent<Threaded> for Ev<Tablet> {
    fn state(&self) -> ProximityState {
        if self.data.near { ProximityState::In } else { ProximityState::Out }
    }
}
impl backend::TabletToolTipEvent<Threaded> for Ev<Tablet> {
    fn tip_state(&self) -> TabletToolTipState {
        if self.data.down { TabletToolTipState::Down } else { TabletToolTipState::Up }
    }
}
impl backend::TabletToolButtonEvent<Threaded> for Ev<Tablet> {
    fn button(&self) -> u32 {
        self.data.button.0
    }
    fn seat_button_count(&self) -> u32 {
        self.data.button.1
    }
    fn button_state(&self) -> ButtonState {
        if self.data.button.2 { ButtonState::Pressed } else { ButtonState::Released }
    }
}

#[derive(Debug)]
pub struct Toggle {
    pub switch: Option<Switch>,
    pub on: bool,
}
impl backend::SwitchToggleEvent<Threaded> for Ev<Toggle> {
    fn switch(&self) -> Option<Switch> {
        self.data.switch
    }
    fn state(&self) -> SwitchState {
        if self.data.on { SwitchState::On } else { SwitchState::Off }
    }
}

/// The input thread's events, as smithay's input backends are.
#[derive(Debug)]
pub struct Threaded;

impl InputBackend for Threaded {
    type Device = Dev;
    type KeyboardKeyEvent = Ev<Key>;
    type PointerAxisEvent = Ev<Scroll>;
    type PointerButtonEvent = Ev<Button>;
    type PointerMotionEvent = Ev<Motion>;
    type PointerMotionAbsoluteEvent = Ev<Absolute>;
    type GestureSwipeBeginEvent = Ev<Begin>;
    type GestureSwipeUpdateEvent = Ev<Swipe>;
    type GestureSwipeEndEvent = Ev<End>;
    type GesturePinchBeginEvent = Ev<Begin>;
    type GesturePinchUpdateEvent = Ev<Pinch>;
    type GesturePinchEndEvent = Ev<End>;
    type GestureHoldBeginEvent = Ev<Begin>;
    type GestureHoldEndEvent = Ev<End>;
    type TouchDownEvent = Ev<TouchAt>;
    type TouchUpEvent = Ev<TouchOf>;
    type TouchMotionEvent = Ev<TouchAt>;
    type TouchCancelEvent = Ev<TouchOf>;
    type TouchFrameEvent = Ev<TouchFrame>;
    type TabletToolAxisEvent = Ev<Tablet>;
    type TabletToolProximityEvent = Ev<Tablet>;
    type TabletToolTipEvent = Ev<Tablet>;
    type TabletToolButtonEvent = Ev<Tablet>;
    type SwitchToggleEvent = Ev<Toggle>;
    type SpecialEvent = UnusedEvent;
}

/// A tablet event's owned form (what else it says: near, the tip down, a
/// button).
fn tablet<E: backend::TabletToolEvent<LibinputInputBackend>>(e: &E, device: Dev, near: bool, down: bool, button: (u32, u32, bool)) -> Ev<Tablet> {
    use backend::TabletToolEvent as T;
    Ev {
        time: backend::Event::time(e).micros(),
        device,
        data: Tablet {
            tool: T::tool(e),
            at: Abs::of::<LibinputInputBackend, _>(e),
            dx: T::delta_x(e),
            dy: T::delta_y(e),
            axes: [
                (T::distance(e), T::distance_has_changed(e)),
                (T::pressure(e), T::pressure_has_changed(e)),
                (T::slider_position(e), T::slider_has_changed(e)),
                (T::tilt_x(e), T::tilt_x_has_changed(e)),
                (T::tilt_y(e), T::tilt_y_has_changed(e)),
                (T::rotation(e), T::rotation_has_changed(e)),
            ],
            wheel: (T::wheel_delta(e), T::wheel_delta_discrete(e), T::wheel_has_changed(e)),
            near,
            down,
            button,
        },
    }
}

/// libinput's event made owned, with its device's snapshot (`dev`).
/// None for what isn't an input event (devices go apart). Every call is
/// smithay's trait's, as the loop's handling had them: libinput's own
/// types have methods of the same names that answer in libinput's types.
pub fn convert(event: InputEvent<LibinputInputBackend>, dev: impl FnOnce(&libinput::Device) -> Dev) -> Option<InputEvent<Threaded>> {
    use backend::{
        GestureBeginEvent as Begins, GestureEndEvent as Ends, GesturePinchUpdateEvent as Pinches, GestureSwipeUpdateEvent as Swipes, KeyboardKeyEvent as Keys, PointerAxisEvent as Scrolls,
        PointerButtonEvent as Buttons, PointerMotionEvent as Moves, SwitchToggleEvent as Switches, TouchEvent as Touches,
    };
    macro_rules! ev {
        ($e:expr, $data:expr) => {
            Ev { time: backend::Event::time(&$e).micros(), device: dev(&backend::Event::device(&$e)), data: $data }
        };
    }
    Some(match event {
        InputEvent::Keyboard { event: e } => InputEvent::Keyboard { event: ev!(e, Key { code: Keys::key_code(&e), pressed: Keys::state(&e) == KeyState::Pressed, count: Keys::count(&e) }) },
        InputEvent::PointerMotion { event: e } => {
            let data = Motion { dx: Moves::delta_x(&e), dy: Moves::delta_y(&e), ux: Moves::delta_x_unaccel(&e), uy: Moves::delta_y_unaccel(&e) };
            InputEvent::PointerMotion { event: ev!(e, data) }
        }
        InputEvent::PointerMotionAbsolute { event: e } => InputEvent::PointerMotionAbsolute { event: ev!(e, Absolute { at: Abs::of::<LibinputInputBackend, _>(&e) }) },
        InputEvent::PointerButton { event: e } => InputEvent::PointerButton { event: ev!(e, Button { code: Buttons::button_code(&e), pressed: Buttons::state(&e) == ButtonState::Pressed }) },
        InputEvent::PointerAxis { event: e } => {
            let scroll = Scroll {
                source: Scrolls::source(&e),
                amount: [Scrolls::amount(&e, Axis::Horizontal), Scrolls::amount(&e, Axis::Vertical)],
                v120: [Scrolls::amount_v120(&e, Axis::Horizontal), Scrolls::amount_v120(&e, Axis::Vertical)],
                inverted: Scrolls::relative_direction(&e, Axis::Vertical) == AxisRelativeDirection::Inverted,
            };
            InputEvent::PointerAxis { event: ev!(e, scroll) }
        }
        InputEvent::GestureSwipeBegin { event: e } => InputEvent::GestureSwipeBegin { event: ev!(e, Begin { fingers: Begins::fingers(&e) }) },
        InputEvent::GestureSwipeUpdate { event: e } => InputEvent::GestureSwipeUpdate { event: ev!(e, Swipe { dx: Swipes::delta_x(&e), dy: Swipes::delta_y(&e) }) },
        InputEvent::GestureSwipeEnd { event: e } => InputEvent::GestureSwipeEnd { event: ev!(e, End { cancelled: Ends::cancelled(&e) }) },
        InputEvent::GesturePinchBegin { event: e } => InputEvent::GesturePinchBegin { event: ev!(e, Begin { fingers: Begins::fingers(&e) }) },
        InputEvent::GesturePinchUpdate { event: e } => {
            let data = Pinch { dx: Pinches::delta_x(&e), dy: Pinches::delta_y(&e), scale: Pinches::scale(&e), rotation: Pinches::rotation(&e) };
            InputEvent::GesturePinchUpdate { event: ev!(e, data) }
        }
        InputEvent::GesturePinchEnd { event: e } => InputEvent::GesturePinchEnd { event: ev!(e, End { cancelled: Ends::cancelled(&e) }) },
        InputEvent::GestureHoldBegin { event: e } => InputEvent::GestureHoldBegin { event: ev!(e, Begin { fingers: Begins::fingers(&e) }) },
        InputEvent::GestureHoldEnd { event: e } => InputEvent::GestureHoldEnd { event: ev!(e, End { cancelled: Ends::cancelled(&e) }) },
        InputEvent::TouchDown { event: e } => InputEvent::TouchDown { event: ev!(e, TouchAt { slot: Touches::slot(&e), at: Abs::of::<LibinputInputBackend, _>(&e) }) },
        InputEvent::TouchMotion { event: e } => InputEvent::TouchMotion { event: ev!(e, TouchAt { slot: Touches::slot(&e), at: Abs::of::<LibinputInputBackend, _>(&e) }) },
        InputEvent::TouchUp { event: e } => InputEvent::TouchUp { event: ev!(e, TouchOf { slot: Touches::slot(&e) }) },
        InputEvent::TouchCancel { event: e } => InputEvent::TouchCancel { event: ev!(e, TouchOf { slot: Touches::slot(&e) }) },
        InputEvent::TouchFrame { event: e } => InputEvent::TouchFrame { event: ev!(e, TouchFrame) },
        InputEvent::TabletToolAxis { event: e } => InputEvent::TabletToolAxis { event: tablet(&e, dev(&backend::Event::device(&e)), true, false, (0, 0, false)) },
        InputEvent::TabletToolProximity { event: e } => {
            let near = backend::TabletToolProximityEvent::state(&e) == ProximityState::In;
            InputEvent::TabletToolProximity { event: tablet(&e, dev(&backend::Event::device(&e)), near, false, (0, 0, false)) }
        }
        InputEvent::TabletToolTip { event: e } => {
            let down = backend::TabletToolTipEvent::tip_state(&e) == TabletToolTipState::Down;
            InputEvent::TabletToolTip { event: tablet(&e, dev(&backend::Event::device(&e)), true, down, (0, 0, false)) }
        }
        InputEvent::TabletToolButton { event: e } => {
            use backend::TabletToolButtonEvent as B;
            let button = (B::button(&e), B::seat_button_count(&e), B::button_state(&e) == ButtonState::Pressed);
            InputEvent::TabletToolButton { event: tablet(&e, dev(&backend::Event::device(&e)), true, false, button) }
        }
        InputEvent::SwitchToggle { event: e } => InputEvent::SwitchToggle { event: ev!(e, Toggle { switch: Switches::switch(&e), on: Switches::state(&e) == SwitchState::On }) },
        InputEvent::DeviceAdded { .. } | InputEvent::DeviceRemoved { .. } | InputEvent::Special(_) => return None,
    })
}

// ---------------------------------------------------------------------
// Between the loop and the thread.

/// What the loop asks of the thread (it never waits for an answer).
pub enum Command {
    /// Preferences ▸ Touchpad & Mouse, for every pointing device now and
    /// any added later.
    Configure(Box<olo_protocol::Settings>),
    /// The keyboards' lights.
    Leds(libinput::Led),
    /// Start libinput again (the touch filter changed), with Olo's touch
    /// filter's plugins or without.
    Restart { plugins: bool },
    /// Look again: the loop has handled more since this thread took the
    /// cursor (so it lets go now, not at the next event).
    Look,
    /// The session paused (another VT): let go of the devices.
    Suspend,
    /// Stop: libinput goes back to the loop (the switch turned off).
    Quit,
    /// ...and resumed.
    Resume,
}

/// What the thread sends the loop: events, numbered, and when libinput
/// started again (every device it had is gone; they come back added).
pub enum Item {
    Event(u64, InputEvent<Threaded>),
    Restarted,
}

/// What the loop tells the thread of the pointer, under a lock each side
/// takes briefly: after each batch of input and each frame.
#[derive(Default)]
pub struct View {
    /// The newest event the loop handled.
    pub handled: u64,
    /// Where the pointer was after it.
    pub pointer: Point<f64, Logical>,
    /// The screens, to keep the pointer on them as the loop does.
    pub screens: Vec<Rectangle<i32, Logical>>,
    /// The cursor plane, while the pointer is on one this thread may move.
    pub plane: Option<Plane>,
    /// This thread has the cursor (the loop held): the loop says when it
    /// has handled more (`Command::Look`).
    pub taken: bool,
}

/// A display's cursor plane, as the loop last put it there.
#[derive(Clone)]
pub struct Plane {
    pub fd: DrmDeviceFd,
    pub crtc: crtc::Handle,
    pub place: Place,
}

/// Where a cursor plane is, and how the display it's on is laid out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Place {
    /// The display, in the loop's space, at its scale and transform.
    pub output: Rectangle<i32, Logical>,
    pub scale: f64,
    pub transform: Transform,
    /// Where the plane was (the CRTC's coordinates) with the pointer at
    /// `at` (where the frame drew it).
    pub dst: Point<i32, Physical>,
    pub at: Point<f64, Logical>,
}

/// The loop's end of the input thread.
pub struct Handle {
    commands: channel::Sender<Command>,
    pub view: Arc<Mutex<View>>,
}

impl Handle {
    /// False if the thread is gone.
    pub fn send(&self, command: Command) -> bool {
        let sent = self.commands.send(command).is_ok();
        if !sent {
            warn!("the input thread is gone");
        }
        sent
    }
}

/// Start the thread: libinput on `seat`, its devices opened by asking the
/// loop (`requests`), the touchpads it opens told to `pads`, its events
/// sent to `events`.
pub fn spawn(
    seat: String,
    plugins: bool,
    settings: Option<olo_protocol::Settings>,
    requests: channel::Sender<crate::contacts::Request>,
    pads: channel::Sender<crate::contacts::Pad>,
    events: channel::Sender<Vec<Item>>,
) -> std::io::Result<Handle> {
    let (commands, commands_in) = channel::channel::<Command>();
    let view = Arc::new(Mutex::new(View::default()));
    let shared = view.clone();
    std::thread::Builder::new().name("olo-input".into()).spawn(move || {
        prioritise();
        let mut event_loop: EventLoop<'static, Input> = match EventLoop::try_new() {
            Ok(l) => l,
            Err(e) => {
                warn!(%e, "the input thread has no loop");
                return;
            }
        };
        let handle = event_loop.handle();
        let _ = handle.insert_source(commands_in, |event, _, input: &mut Input| {
            if let channel::Event::Msg(command) = event {
                input.command(command);
            }
        });
        let mut input = Input {
            seat,
            handle: handle.clone(),
            requests,
            pads,
            events,
            context: None,
            devices: HashMap::new(),
            keyboards: Vec::new(),
            pointers: Vec::new(),
            settings,
            leds: libinput::Led::empty(),
            next_id: 0,
            seq: 0,
            // (A new thread's devices are all new: the loop forgets any
            // an earlier one said.)
            batch: vec![Item::Restarted],
            cursor: Cursor::new(shared),
            quit: false,
        };
        input.start(plugins);
        while !input.quit {
            let timeout = input.cursor.timeout();
            if let Err(e) = event_loop.dispatch(timeout, &mut input) {
                warn!(%e, "the input thread's loop");
                // (Never round and round on an error.)
                std::thread::sleep(Duration::from_millis(10));
            }
            input.flush();
            input.cursor.check();
        }
    })?;
    Ok(Handle { commands, view })
}

/// The highest nice rtkit gives (`INPUT_NICE`), above the loop's own:
/// read as soon as the kernel has an event, whatever else runs. Not
/// realtime: rtkit's limit on a realtime thread (200 ms of CPU without
/// sleeping) would end app_server, the whole desktop, were this thread
/// ever to spin. (It started at the loop's nice, and on its fast cores.)
fn prioritise() {
    if let Some(why) = olo_system::priority::not_the_persons() {
        info!("the input thread stays at its nice: {why}");
        return;
    }
    match olo_system::priority::raise_self(olo_protocol::performance::INPUT_NICE) {
        Ok(nice) => info!(nice, "the input thread's priority (rtkit)"),
        Err(e) => info!("the input thread stays at its nice: {e}"),
    }
}

/// The thread's state.
struct Input {
    seat: String,
    handle: calloop::LoopHandle<'static, Input>,
    requests: channel::Sender<crate::contacts::Request>,
    pads: channel::Sender<crate::contacts::Pad>,
    events: channel::Sender<Vec<Item>>,
    context: Option<(Libinput, RegistrationToken)>,
    devices: HashMap<libinput::Device, Dev>,
    keyboards: Vec<libinput::Device>,
    pointers: Vec<libinput::Device>,
    settings: Option<olo_protocol::Settings>,
    leds: libinput::Led,
    next_id: u64,
    seq: u64,
    batch: Vec<Item>,
    cursor: Cursor,
    quit: bool,
}

impl Input {
    /// libinput on the seat, with the touch filter's plugins or not.
    fn start(&mut self, plugins: bool) {
        let interface = crate::contacts::Interface::asking(self.requests.clone(), self.pads.clone());
        let context = Libinput::new_with_udev(interface);
        if plugins {
            context.plugin_system_append_path(&crate::touch_filter::dir().to_string_lossy());
        }
        context.plugin_system_append_default_paths();
        if let Err(e) = context.plugin_system_load_plugins(libinput::PluginSystemFlags::empty()) {
            warn!(%e, "libinput plugins not loaded");
        }
        let mut seat_context = context.clone();
        if seat_context.udev_assign_seat(&self.seat).is_err() {
            warn!(seat = self.seat, "libinput could not take the seat");
        }
        let backend = LibinputInputBackend::new(context.clone());
        match self.handle.insert_source(backend, |event, _, input: &mut Input| input.on_event(event)) {
            Ok(token) => self.context = Some((context, token)),
            Err(e) => warn!(%e, "could not watch libinput"),
        }
        info!(plugins, "libinput started, on the input thread");
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::Configure(settings) => {
                for device in self.pointers.iter_mut() {
                    crate::udev::configure_pointer(device, &settings);
                }
                self.settings = Some(*settings);
            }
            Command::Leds(leds) => {
                self.leds = leds;
                for keyboard in self.keyboards.iter_mut() {
                    keyboard.led_update(leds);
                }
            }
            Command::Restart { plugins } => {
                if let Some((context, token)) = self.context.take() {
                    self.handle.remove(token);
                    drop(context);
                }
                self.devices.clear();
                self.keyboards.clear();
                self.pointers.clear();
                self.flush();
                self.batch.push(Item::Restarted);
                self.start(plugins);
            }
            // (What it's for comes after: `Cursor::check`.)
            Command::Look => {}
            Command::Suspend => {
                if let Some((context, _)) = self.context.as_mut() {
                    context.suspend();
                }
            }
            Command::Quit => {
                if let Some((context, token)) = self.context.take() {
                    self.handle.remove(token);
                    drop(context);
                }
                self.devices.clear();
                self.keyboards.clear();
                self.pointers.clear();
                self.quit = true;
                // (After the devices' closes, on the same way.)
                let _ = self.requests.send(crate::contacts::Request::Stopped);
            }
            Command::Resume => {
                if let Some((context, _)) = self.context.as_mut()
                    && context.resume().is_err()
                {
                    warn!("libinput didn't resume");
                }
            }
        }
    }

    fn dev(&mut self, device: &libinput::Device) -> Dev {
        if let Some(dev) = self.devices.get(device) {
            return dev.clone();
        }
        // (Not seen added: libinput says added first, always.)
        self.next_id += 1;
        let dev = Dev(Arc::new(DevInfo::of(device, self.next_id)));
        self.devices.insert(device.clone(), dev.clone());
        dev
    }

    fn on_event(&mut self, event: InputEvent<LibinputInputBackend>) {
        let event = match event {
            InputEvent::DeviceAdded { mut device } => {
                use smithay::backend::input::{Device as D, DeviceCapability as C};
                info!("New device {:?}", device.sysname());
                if D::has_capability(&device, C::Keyboard) {
                    device.led_update(self.leds);
                    self.keyboards.push(device.clone());
                }
                if D::has_capability(&device, C::Pointer) {
                    if let Some(settings) = &self.settings {
                        crate::udev::configure_pointer(&mut device, settings);
                    }
                    self.pointers.push(device.clone());
                }
                InputEvent::DeviceAdded { device: self.dev(&device) }
            }
            InputEvent::DeviceRemoved { device } => {
                info!("Removed device {:?}", device.sysname());
                self.keyboards.retain(|d| *d != device);
                self.pointers.retain(|d| *d != device);
                let dev = self.dev(&device);
                self.devices.remove(&device);
                InputEvent::DeviceRemoved { device: dev }
            }
            event => {
                let Some(event) = convert(event, |d| self.dev(d)) else { return };
                event
            }
        };
        self.seq += 1;
        match &event {
            InputEvent::PointerMotion { event } => self.cursor.moved(self.seq, Op::By(event.delta())),
            InputEvent::PointerMotionAbsolute { event } => self.cursor.moved(self.seq, Op::To(event.data.at.fx, event.data.at.fy)),
            _ => {}
        }
        self.batch.push(Item::Event(self.seq, event));
    }

    /// What was read, to the loop: one wakeup a read.
    fn flush(&mut self) {
        if !self.batch.is_empty() {
            let _ = self.events.send(std::mem::take(&mut self.batch));
        }
    }
}

// ---------------------------------------------------------------------
// The cursor, while the loop is held.

/// A motion: by so much, or to a place on all the screens (an absolute
/// device's fraction of them).
#[derive(Debug, Clone, Copy)]
pub enum Op {
    By(Point<f64, Logical>),
    To(f64, f64),
}

/// Where the loop will put the pointer: where it was after the last event
/// it handled, and the motions since, kept on the screens as it keeps it.
pub fn predict(from: Point<f64, Logical>, screens: &[Rectangle<i32, Logical>], ops: impl IntoIterator<Item = Op>) -> Point<f64, Logical> {
    let mut at = from;
    for op in ops {
        at = match op {
            Op::By(delta) => crate::input_handler::clamp_to_screens(screens, at + delta),
            Op::To(fx, fy) => {
                let Some(all) = screens.iter().copied().reduce(|a, b| a.merge(b)) else { continue };
                crate::input_handler::clamp_to_screens(screens, Point::from((all.loc.x as f64 + fx * all.size.w as f64, all.loc.y as f64 + fy * all.size.h as f64)))
            }
        };
    }
    at
}

/// Where the cursor plane goes for the pointer at `at`: moved from where
/// the loop put it by as much as the pointer moved, in the display's
/// pixels and turned as the display is; the pointer kept on the display
/// (as the frame keeps it).
pub fn plane_at(plane: &Place, at: Point<f64, Logical>) -> Point<i32, Physical> {
    let o = plane.output.to_f64();
    let at = Point::<f64, Logical>::from((at.x.clamp(o.loc.x, o.loc.x + o.size.w - 1.0), at.y.clamp(o.loc.y, o.loc.y + o.size.h - 1.0)));
    let moved = (at - plane.at).to_physical(plane.scale);
    // (A turn about any point: the size cancels.)
    let none = Size::<f64, Physical>::from((0.0, 0.0));
    let turned = plane.transform.transform_point_in(moved, &none) - plane.transform.transform_point_in(Point::from((0.0, 0.0)), &none);
    plane.dst + turned.to_i32_round()
}

struct Cursor {
    view: Arc<Mutex<View>>,
    /// Motions sent to the loop, not yet known handled: their number,
    /// what they did, when they were sent.
    pending: VecDeque<(u64, Op, Instant)>,
    /// The loop is held (a motion waited past `TAKE_OVER`): since when,
    /// and how many moves this thread made meanwhile. Nothing to wait for
    /// then but more input (or none: no timer, so nothing goes round).
    taken: Option<(Instant, u32)>,
    /// Where it was last put (CRTC, place).
    last: Option<(crtc::Handle, Point<i32, Physical>)>,
    /// A move the kernel refused: not again until the loop puts the
    /// plane somewhere new.
    refused: Option<Point<i32, Physical>>,
    /// When it last moved it, and whether a newer place waits for
    /// `MOVE_EVERY` to pass.
    moved_at: Option<Instant>,
    deferred: bool,
}

impl Cursor {
    fn new(view: Arc<Mutex<View>>) -> Cursor {
        Cursor { view, pending: VecDeque::new(), taken: None, last: None, refused: None, moved_at: None, deferred: false }
    }

    fn moved(&mut self, seq: u64, op: Op) {
        self.pending.push_back((seq, op, Instant::now()));
    }

    /// How long the loop has before this thread looks again (None: until
    /// something comes).
    fn timeout(&self) -> Option<Duration> {
        if self.taken.is_some() {
            // (Only a move waiting for its time: nothing goes round.)
            return self.deferred.then(|| MOVE_EVERY.saturating_sub(self.moved_at.map_or(MOVE_EVERY, |t| t.elapsed())));
        }
        let (_, _, sent) = self.pending.front()?;
        Some(TAKE_OVER.saturating_sub(sent.elapsed()))
    }

    /// After each read: let go if the loop has caught up; take over if a
    /// motion has waited too long; follow while taken over.
    fn check(&mut self) {
        if self.pending.is_empty() && self.taken.is_none() {
            return;
        }
        let mut view = self.view.lock().unwrap_or_else(|e| e.into_inner());
        while self.pending.front().is_some_and(|(seq, _, _)| *seq <= view.handled) {
            self.pending.pop_front();
        }
        if self.pending.is_empty() {
            view.taken = false;
            drop(view);
            match self.taken.take() {
                Some((since, moves)) if moves > 0 => info!(ms = since.elapsed().as_millis() as u64, moves, "the loop caught up: the input thread had moved the pointer while it was held"),
                Some((since, _)) => debug!(ms = since.elapsed().as_millis() as u64, "the loop caught up (the pointer wasn't on a plane the input thread may move)"),
                None => {}
            }
            self.last = None;
            self.deferred = false;
            return;
        }
        if self.taken.is_none() && self.pending.front().is_some_and(|(_, _, sent)| sent.elapsed() < TAKE_OVER) {
            return;
        }
        view.taken = true;
        let taken = self.taken.get_or_insert((Instant::now(), 0));
        self.deferred = false;
        let Some(plane) = view.plane.clone() else { return };
        let at = predict(view.pointer, &view.screens, self.pending.iter().map(|(_, op, _)| *op));
        drop(view);
        let to = plane_at(&plane.place, at);
        if self.last == Some((plane.crtc, to)) || self.refused == Some(plane.place.dst) {
            return;
        }
        if self.moved_at.is_some_and(|t| t.elapsed() < MOVE_EVERY) {
            self.deferred = true;
            return;
        }
        self.moved_at = Some(Instant::now());
        // (drm-rs calls the legacy move deprecated, for a cursor plane's
        // atomic commits: here it's the point. It leaves the plane's
        // buffer, and an atomic commit from this thread would race the
        // loop's page flips for the CRTC.)
        #[allow(deprecated)]
        let moved = plane.fd.move_cursor(plane.crtc, (to.x, to.y));
        match moved {
            Ok(()) => {
                taken.1 += 1;
                self.last = Some((plane.crtc, to));
            }
            Err(e) => {
                warn!(%e, "the kernel refused the input thread's cursor move");
                self.refused = Some(plane.place.dst);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(transform: Transform, scale: f64) -> Place {
        Place { output: Rectangle::new((1000, 0).into(), (1000, 800).into()), scale, transform, dst: Point::from((400, 300)), at: Point::from((1200.0, 150.0)) }
    }

    #[test]
    fn the_plane_moves_as_the_pointer_does() {
        let p = plane(Transform::Normal, 2.0);
        assert_eq!(plane_at(&p, Point::from((1200.0, 150.0))), Point::from((400, 300)));
        assert_eq!(plane_at(&p, Point::from((1210.0, 145.5))), Point::from((420, 291)));
    }

    #[test]
    fn a_turned_display_turns_the_motion() {
        // A display turned a quarter: the pointer moving right moves the
        // plane down the panel's own rows or columns, by the same amount.
        for t in [Transform::_90, Transform::_180, Transform::_270, Transform::Flipped] {
            let p = plane(t, 1.0);
            let to = plane_at(&p, Point::from((1210.0, 150.0)));
            let d = to - p.dst;
            assert_eq!(d.x.abs() + d.y.abs(), 10, "{t:?}: {d:?}");
        }
        let p = plane(Transform::_180, 1.0);
        assert_eq!(plane_at(&p, Point::from((1210.0, 150.0))), Point::from((390, 300)));
    }

    #[test]
    fn the_pointer_stays_on_its_display() {
        let p = plane(Transform::Normal, 1.0);
        // Past the display's right edge: kept on its last column.
        assert_eq!(plane_at(&p, Point::from((2500.0, 150.0))), Point::from((400 + 799, 300)));
    }

    #[test]
    fn prediction_follows_the_loop() {
        let screens = [Rectangle::new((0, 0).into(), (1000, 800).into()), Rectangle::new((1000, 0).into(), (1000, 800).into())];
        let from = Point::from((990.0, 10.0));
        // Across to the second screen, then off its top: kept on it.
        let at = predict(from, &screens, [Op::By(Point::from((20.0, 0.0))), Op::By(Point::from((0.0, -50.0)))]);
        assert_eq!(at, Point::from((1010.0, 0.0)));
        // An absolute device: a fraction of all of them.
        let at = predict(from, &screens, [Op::To(0.75, 0.5)]);
        assert_eq!(at, Point::from((1500.0, 400.0)));
    }
}

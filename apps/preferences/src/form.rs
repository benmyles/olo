//! A panel's controls, top to bottom, each bound to its part of the
//! settings: `Ui` collects them as a panel's `ui` function adds them,
//! and a change made with one goes to
//! [`Prefs::edit`] (app_server's copy, and every window showing it). A
//! panel's own state (what it is busy with, what is selected) is changed
//! through [`Ui::on_view`], on its window's view.
//!
//! A box whose settings aren't their defaults has a Reset in its top
//! line, putting back just those: each control bound here (`check`,
//! `radio`, `slider`, `menu`, `choose`, `field`) leaves a probe in the
//! boxes it's in, its setter with the value it shows; set on the
//! defaults, the probes change them only if the box was changed, and
//! where they change them is what Reset puts back.

use std::{collections::HashMap, rc::Rc, time::Duration};

use olo_kit::{
    Child, ClickEvent, Cx, Element, IntoElement, ParentElement as _, SharedString, Styled as _, ViewRef, WindowCx, div, metrics as m, px,
    widgets::{BBox, Button, CheckBox, MenuField, RadioButton, Slider, StringView, TextControl, TextField, TextFieldEvent},
};
use olo_protocol::Settings;

use crate::{PanelView, prefs};

/// A change to the settings, from a control.
type Set<T> = Rc<dyn Fn(&mut Settings, T)>;

/// A control's setter with the value it shows (a box's Reset).
type Probe = Rc<dyn Fn(&mut Settings)>;

thread_local! {
    /// What a box's Reset goes back to, and its `rounded` JSON.
    static DEFAULTS: (Rc<Settings>, Rc<Vec<u8>>) = {
        let d = Settings::default();
        let json = rounded(&d);
        (Rc::new(d), Rc::new(json))
    };
}

/// A panel's text fields, by id, and what each sets as it is typed in
/// (the latest `body`'s: a field's setter can depend on what is shown,
/// a workspace's number).
#[derive(Default)]
pub struct Fields {
    fields: HashMap<SharedString, Child<TextField>>,
    setters: HashMap<SharedString, Set<String>>,
}

pub struct Ui<'a, 'b, 'c> {
    pub cx: &'a mut Cx<'b, 'c, PanelView>,
    /// The settings as they are now.
    pub s: &'a Settings,
    fields: &'a mut Fields,
    items: Vec<Element>,
    enabled: bool,
    /// The boxes being built, the innermost last: their controls' probes.
    boxes: Vec<Vec<Probe>>,
    /// Changed boxes get a Reset (not in panels without Defaults).
    resets: bool,
    /// The titles of the boxes being built (an undo step's name).
    titles: Vec<String>,
    /// Building a row (`row_gap`): its check boxes are a control's height,
    /// centred with the menus and fields beside them.
    in_row: bool,
}

impl<'a, 'b, 'c> Ui<'a, 'b, 'c> {
    pub fn new(cx: &'a mut Cx<'b, 'c, PanelView>, s: &'a Settings, fields: &'a mut Fields) -> Self {
        Ui { cx, s, fields, items: Vec::new(), enabled: true, boxes: Vec::new(), resets: false, titles: Vec::new(), in_row: false }
    }

    /// Boxes whose settings aren't their defaults get a Reset.
    pub fn resets(&mut self, on: bool) {
        self.resets = on;
    }

    /// In each box being built: `set`, the setting as it is now.
    fn probe(&mut self, set: impl Fn(&mut Settings) + 'static) {
        if self.boxes.is_empty() || !self.resets {
            return;
        }
        let p: Probe = Rc::new(set);
        for b in &mut self.boxes {
            b.push(p.clone());
        }
    }

    /// What was added, top to bottom.
    pub fn take(&mut self) -> Vec<Element> {
        std::mem::take(&mut self.items)
    }

    pub fn add(&mut self, element: impl IntoElement) {
        self.items.push(element.into_element());
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// What a change made here is called for Undo ("“Volume”": the
    /// control's label, else its box's title), and (with `run`, for a
    /// slider or a field) the run it's part of.
    fn step(&self, label: Option<&str>, run: bool) -> (String, String) {
        let label = label.map(|l| l.trim().trim_end_matches(':').trim().to_owned()).filter(|l| !l.is_empty());
        let name = match (&label, self.titles.iter().rev().find(|t| !t.is_empty())) {
            (Some(l), _) => format!("“{l}”"),
            (None, Some(t)) => format!("“{t}”"),
            _ => "Change".into(),
        };
        let key = if run { format!("{}/{}", self.titles.join("/"), label.unwrap_or_default()) } else { String::new() };
        (key, name)
    }

    /// A change to the settings, for a control's callback (kept for Undo
    /// by its box's name).
    pub fn edit<T: Clone + 'static>(&self, set: impl Fn(&mut Settings, T) + 'static) -> impl Fn(&T, &mut WindowCx) + 'static {
        let (run, name) = self.step(None, false);
        self.edit_as(run, name, set)
    }

    /// `edit`, kept for Undo as `name`, part of `run` (none: a step of its
    /// own).
    fn edit_as<T: Clone + 'static>(&self, run: String, name: String, set: impl Fn(&mut Settings, T) + 'static) -> impl Fn(&T, &mut WindowCx) + 'static {
        move |v: &T, _: &mut WindowCx| {
            let v = v.clone();
            prefs().edit_as(&run, &name, |s| set(s, v));
        }
    }

    /// A change to the settings on a click.
    pub fn edit_click<E: ?Sized + 'static>(&self, set: impl Fn(&mut Settings) + 'static) -> impl Fn(&E, &mut WindowCx) + 'static {
        let (run, name) = self.step(None, false);
        move |_: &E, _: &mut WindowCx| prefs().edit_as(&run, &name, &set)
    }

    /// [`Ui::edit`] for a control built by hand showing `now`: its box's
    /// Reset puts it back too.
    pub fn bind<T: Clone + 'static>(&mut self, now: T, set: impl Fn(&mut Settings, T) + 'static) -> impl Fn(&T, &mut WindowCx) + 'static {
        let set = Rc::new(set);
        let probe = set.clone();
        self.probe(move |s| probe(s, now.clone()));
        // (Often dragged: a run is one step.)
        let (run, name) = self.step(None, true);
        self.edit_as(run, name, move |s, v| set(s, v))
    }

    /// [`Ui::edit_click`] for one of a set of choices built by hand (a
    /// card, a row): Reset puts back the `chosen` one's.
    pub fn bind_click<E: ?Sized + 'static>(&mut self, chosen: bool, set: impl Fn(&mut Settings) + 'static) -> impl Fn(&E, &mut WindowCx) + 'static {
        let set = Rc::new(set);
        if chosen {
            let probe = set.clone();
            self.probe(move |s| probe(s));
        }
        self.edit_click(move |s| set(s))
    }

    /// Something to do to the panel's window (its state), for a control.
    pub fn on_view<T: ?Sized + 'static>(&self, f: impl Fn(&mut PanelView, &T, &mut Cx<PanelView>) + 'static) -> impl Fn(&T, &mut WindowCx) + 'static {
        self.cx.listener(f)
    }

    /// [`Ui::on_view`] on a click.
    pub fn on_click_view<E: ?Sized + 'static>(&self, f: impl Fn(&mut PanelView, &mut Cx<PanelView>) + 'static) -> impl Fn(&E, &mut WindowCx) + 'static {
        self.cx.listener(move |v: &mut PanelView, _: &E, cx| f(v, cx))
    }

    /// What `build` adds, for the caller to lay out.
    pub fn collect(&mut self, build: impl FnOnce(&mut Self)) -> Vec<Element> {
        let outer = std::mem::take(&mut self.items);
        build(self);
        std::mem::replace(&mut self.items, outer)
    }

    /// An etched box with its title in its top line, and Reset at its
    /// end while its settings aren't their defaults.
    pub fn bbox(&mut self, title: &str, build: impl FnOnce(&mut Self)) {
        self.boxes.push(Vec::new());
        self.titles.push(title.to_owned());
        let items = self.collect(build);
        self.titles.pop();
        let probes = self.boxes.pop().unwrap_or_default();
        let mut b = if title.is_empty() { BBox::plain() } else { BBox::new(SharedString::from(title.to_owned())) };
        if self.enabled && (changed(&probes) || every_reset() && !probes.is_empty()) {
            let name = if title.is_empty() { "Reset".to_owned() } else { format!("Reset of “{title}”") };
            b = b.action("Reset", move |_: &ClickEvent, _: &mut WindowCx| prefs().edit_as("", &name, |s| reset(s, &probes)));
        }
        self.add(b.children(items));
    }

    /// Side by side, `gap` apart, lined up on their middles.
    pub fn row_gap(&mut self, gap: f32, build: impl FnOnce(&mut Self)) {
        let was = std::mem::replace(&mut self.in_row, true);
        let items = self.collect(build);
        self.in_row = was;
        self.add(div().flex().flex_row().items_center().gap(px(gap)).children(items));
    }

    /// Two columns side by side, `SPACING` apart, each as wide as the
    /// other, their tops lined up: a wide panel's boxes in two stacks
    /// rather than one tall one.
    pub fn columns(&mut self, left: impl FnOnce(&mut Self), right: impl FnOnce(&mut Self)) {
        let (l, r) = (self.collect(left), self.collect(right));
        self.add_columns(l, r);
    }

    /// [`Ui::columns`] of what was collected ([`Ui::collect`]) for each.
    pub fn add_columns(&mut self, left: Vec<Element>, right: Vec<Element>) {
        let column = |items: Vec<Element>| div().flex_1().min_w(px(0.0)).flex().flex_col().gap(px(m::SPACING)).children(items);
        self.add(div().flex().flex_row().items_start().gap(px(m::SPACING)).child(column(left)).child(column(right)));
    }

    pub fn row(&mut self, build: impl FnOnce(&mut Self)) {
        self.row_gap(m::SPACING, build);
    }

    /// What `build` adds, usable only if `on` (and this is).
    pub fn enabled_if(&mut self, on: bool, build: impl FnOnce(&mut Self)) {
        let was = self.enabled;
        self.enabled = was && on;
        build(self);
        self.enabled = was;
    }

    pub fn space(&mut self, h: f32) {
        self.add(div().h(px(h)).flex_shrink_0());
    }

    pub fn label(&mut self, text: impl Into<SharedString>) {
        let enabled = self.enabled;
        self.add(StringView::new(text).dim(!enabled));
    }

    /// A note under a control: dim, wrapped, closer to the control it's
    /// about (`TIGHT`) than to what follows (`STACK`).
    pub fn note(&mut self, text: impl Into<SharedString>) {
        // (The first in its box has no control to be close to.)
        let pull = if self.items.is_empty() { 0.0 } else { m::TIGHT - m::STACK };
        self.add(div().mt(px(pull)).child(StringView::new(text).dim(true).wrap(true)));
    }

    pub fn check(&mut self, label: impl Into<SharedString>, on: bool, set: impl Fn(&mut Settings, bool) + 'static) {
        let set = Rc::new(set);
        let probe = set.clone();
        self.probe(move |s| probe(s, on));
        let label: SharedString = label.into();
        let (run, name) = self.step(Some(&label), false);
        let mut c = CheckBox::new(label, on).enabled(self.enabled).on_toggle(self.edit_as(run, name, move |s, v| set(s, v)));
        // (In a row, as tall as the menu or field beside it, so their
        // middles are one: 20 points against 24, the label sat a pixel off
        // the menu's at 1.25× and 1.75×.)
        if self.in_row {
            c = c.height(m::CONTROL_HEIGHT);
        }
        self.add(c);
    }

    pub fn radio(&mut self, label: impl Into<SharedString>, selected: bool, set: impl Fn(&mut Settings) + 'static) {
        let set = Rc::new(set);
        if selected {
            let probe = set.clone();
            self.probe(move |s| probe(s));
        }
        let r = RadioButton::new(label, selected).enabled(self.enabled).on_select(self.edit_click(move |s| set(s)));
        self.add(r);
    }

    /// A slider; `with` sets its label, text and limits.
    pub fn slider(&mut self, id: &str, value: f32, range: std::ops::RangeInclusive<f32>, with: impl FnOnce(Slider) -> Slider, set: impl Fn(&mut Settings, f32) + 'static) {
        let set = Rc::new(set);
        let probe = set.clone();
        self.probe(move |s| probe(s, value));
        let slider = with(Slider::new(SharedString::from(id.to_owned()), value, range));
        // (Dragged: a run is one step.)
        let title = slider.title().map(|t| t.to_string());
        let (_, name) = self.step(title.as_deref(), false);
        let run = format!("slider:{id}");
        let s = slider.enabled(self.enabled).on_change(self.edit_as(run, name, move |s, v| set(s, v)));
        self.add(s);
    }

    /// A pop-up menu of `options`; `with` sets its label, divider, width.
    pub fn menu(&mut self, id: &str, options: Vec<SharedString>, selected: usize, with: impl FnOnce(MenuField) -> MenuField, set: impl Fn(&mut Settings, usize) + 'static) {
        let set = Rc::new(set);
        let probe = set.clone();
        self.probe(move |s| probe(s, selected));
        self.menu_of(id, options, selected, with, move |s, i| set(s, i));
    }

    fn menu_of(&mut self, id: &str, options: Vec<SharedString>, selected: usize, with: impl FnOnce(MenuField) -> MenuField, set: impl Fn(&mut Settings, usize) + 'static) {
        let f = with(MenuField::new(SharedString::from(id.to_owned()), options, selected));
        let title = f.title().map(|t| t.to_string());
        let (run, name) = self.step(title.as_deref(), false);
        let f = f.enabled(self.enabled).on_change(self.edit_as(run, name, set));
        self.add(f);
    }

    /// A pop-up menu of values: `choices` (value, name), the one equal to
    /// `current` chosen (one not among them shown by `other`'s name).
    pub fn choose<T: Clone + PartialEq + 'static>(&mut self, id: &str, choices: &[(T, &str)], current: &T, with: impl FnOnce(MenuField) -> MenuField, set: impl Fn(&mut Settings, T) + 'static) {
        let mut names: Vec<SharedString> = choices.iter().map(|(_, n)| SharedString::from(n.to_string())).collect();
        let selected = choices.iter().position(|(v, _)| v == current).unwrap_or_else(|| {
            names.push("Other".into());
            names.len() - 1
        });
        let values: Vec<T> = choices.iter().map(|(v, _)| v.clone()).collect();
        let set = Rc::new(set);
        // (The value itself, as one not among the choices is changed too.)
        let (probe, now) = (set.clone(), current.clone());
        self.probe(move |s| probe(s, now.clone()));
        self.menu_of(id, names, selected, with, move |s, i| {
            if let Some(v) = values.get(i) {
                set(s, v.clone());
            }
        });
    }

    /// A pop-up menu of several values (`MenuField::many`): `choices`,
    /// those in `chosen` on, `none` what none chosen means; choosing one
    /// turns it on or off, kept in `choices`' order.
    pub fn choose_many(&mut self, id: &str, choices: &[String], chosen: &[String], none: &str, with: impl FnOnce(MenuField) -> MenuField, set: impl Fn(&mut Settings, Vec<String>) + 'static) {
        let names: Vec<SharedString> = choices.iter().map(|c| SharedString::from(c.clone())).collect();
        let on: Vec<bool> = choices.iter().map(|c| chosen.contains(c)).collect();
        let set = Rc::new(set);
        let (probe, now) = (set.clone(), chosen.to_vec());
        self.probe(move |s| probe(s, now.clone()));
        let f = with(MenuField::many(SharedString::from(id.to_owned()), names, on, none.to_owned()));
        let title = f.title().map(|t| t.to_string());
        let (run, name) = self.step(title.as_deref(), false);
        let (choices, chosen) = (choices.to_vec(), chosen.to_vec());
        let f = f.enabled(self.enabled).on_change(self.edit_as(run, name, move |s, i: usize| {
            let mut now: Vec<String> = match choices.get(i) {
                None => Vec::new(),
                Some(c) if chosen.contains(c) => chosen.iter().filter(|o| *o != c).cloned().collect(),
                Some(c) => chosen.iter().cloned().chain([c.clone()]).collect(),
            };
            now.sort_by_key(|o| choices.iter().position(|x| x == o).unwrap_or(usize::MAX));
            set(s, now);
        }));
        self.add(f);
    }

    pub fn button(&mut self, label: impl Into<SharedString>, on_click: impl Fn(&ClickEvent, &mut WindowCx) + 'static) {
        let b = Button::new(label).enabled(self.enabled).on_click(on_click);
        self.add(b);
    }

    /// The text field `id` (made the first time), showing `value` unless
    /// it has the keyboard; typing in it sets what `set` says. (The
    /// field is the panel's: what's handed back shows it, `element`.)
    pub fn field(&mut self, id: &str, value: &str, build: impl FnOnce(TextField) -> TextField, set: impl Fn(&mut Settings, String) + 'static) -> ViewRef<TextField> {
        let id = SharedString::from(id.to_owned());
        let set: Set<String> = Rc::new(set);
        let (probe, now) = (set.clone(), value.to_owned());
        self.probe(move |s| probe(s, now.clone()));
        self.fields.setters.insert(id.clone(), set);
        if !self.fields.fields.contains_key(&id) {
            let f = self.cx.new_view(|_| build(TextField::new()));
            f.update(self.cx, |f, cx| f.set_text(value.to_owned(), cx));
            let key = id.clone();
            // (Typed in: a run is one step.)
            let (_, name) = self.step(None, false);
            let run = format!("field:{id}");
            self.cx.subscribe(&f, move |view: &mut PanelView, e: &TextFieldEvent, cx| {
                if !matches!(e, TextFieldEvent::Changed) {
                    return;
                }
                let Some(text) = view.fields.fields.get(&key).map(|f| f.read(cx).text().to_owned()) else { return };
                let Some(set) = view.fields.setters.get(&key).cloned() else { return };
                prefs().edit_as(&run, &name, |s| set(s, text.clone()));
            });
            self.fields.fields.insert(id.clone(), f);
        }
        let enabled = self.enabled;
        let field = &self.fields.fields[&id];
        let focused = field.is_focused(self.cx);
        field.update(self.cx, |f, cx| {
            if !focused && f.text() != value {
                f.set_text(value.to_owned(), cx);
            }
            f.set_enabled(enabled, cx);
        });
        field.downgrade()
    }

    /// A text control for the field `id` made by [`Ui::field`], for a
    /// row built by hand.
    pub fn control_of(&self, id: &str) -> TextControl {
        TextControl::new(self.cx, &self.fields.fields[id])
    }

    /// A labelled text control bound to a setting; `with` sets its label,
    /// divider and width.
    pub fn text(&mut self, id: &str, value: &str, build: impl FnOnce(TextField) -> TextField, with: impl FnOnce(TextControl) -> TextControl, set: impl Fn(&mut Settings, String) + 'static) {
        self.field(id, value, build, set);
        let control = with(TextControl::new(self.cx, &self.fields.fields[id]));
        self.add(control);
    }
}

/// Every box with settings shows its Reset (the layout lint's second
/// pass: a long title beside it must still fit). (The lint's windows are
/// made on loopers of their own.)
#[cfg(test)]
pub static EVERY_RESET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
fn every_reset() -> bool {
    EVERY_RESET.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(not(test))]
fn every_reset() -> bool {
    false
}

/// Whether a box's settings aren't their defaults: its probes change
/// the defaults (more than a float's rounding: a slider's percent).
fn changed(probes: &[Probe]) -> bool {
    if probes.is_empty() {
        return false;
    }
    let (defaults, probed) = probed(probes);
    // (Settings' own comparison first, 0.6 µs: most boxes are as they
    // were. Then their JSON, floats rounded, 6 µs: a Value tree was 30.)
    *defaults != probed && rounded(&probed) != *DEFAULTS.with(|d| d.1.clone())
}

/// Settings as JSON with floats to five significant digits: the same
/// past a float's rounding (a percent through f32 and back).
fn rounded(s: &Settings) -> Vec<u8> {
    struct Rounded;
    impl serde_json::ser::Formatter for Rounded {
        fn write_f32<W: ?Sized + std::io::Write>(&mut self, w: &mut W, v: f32) -> std::io::Result<()> {
            write!(w, "{v:.4e}")
        }
        fn write_f64<W: ?Sized + std::io::Write>(&mut self, w: &mut W, v: f64) -> std::io::Result<()> {
            write!(w, "{v:.4e}")
        }
    }
    let mut out = Vec::with_capacity(4096);
    let _ = serde::Serialize::serialize(s, &mut serde_json::Serializer::with_formatter(&mut out, Rounded));
    out
}

/// The defaults, and the defaults with each probe set.
fn probed(probes: &[Probe]) -> (Rc<Settings>, Settings) {
    let defaults = DEFAULTS.with(|d| d.0.clone());
    let mut probed = (*defaults).clone();
    for p in probes {
        p(&mut probed);
    }
    (defaults, probed)
}

/// A box's settings back to their defaults: where its probes change
/// the defaults, `s` takes the defaults' value (its other settings stay).
fn reset(s: &mut Settings, probes: &[Probe]) {
    let (defaults, probed) = probed(probes);
    let (Ok(defaults), Ok(probed), Ok(mut now)) = (serde_json::to_value(&*defaults), serde_json::to_value(&probed), serde_json::to_value(&*s)) else { return };
    back(&mut now, &defaults, &probed);
    match serde_json::from_value(now) {
        Ok(n) => *s = n,
        Err(e) => tracing::warn!("a box's Reset: {e}"),
    }
}

/// Where `probed` differs from `defaults`, `now` takes the default:
/// field by field, and item by item in lists of the same length.
fn back(now: &mut serde_json::Value, defaults: &serde_json::Value, probed: &serde_json::Value) {
    use serde_json::Value;
    if same(defaults, probed) {
        return;
    }
    match (defaults, probed, now) {
        (Value::Object(d), Value::Object(p), Value::Object(n)) => {
            for (k, pv) in p {
                let Some(dv) = d.get(k) else { continue };
                match n.get_mut(k) {
                    Some(nv) => back(nv, dv, pv),
                    None => {
                        n.insert(k.clone(), dv.clone());
                    }
                }
            }
        }
        (Value::Array(d), Value::Array(p), Value::Array(n)) if d.len() == p.len() && d.len() == n.len() => {
            for ((dv, pv), nv) in d.iter().zip(p).zip(n.iter_mut()) {
                back(nv, dv, pv);
            }
        }
        (d, _, n) => *n = d.clone(),
    }
}

/// The same settings, numbers within a float's rounding.
fn same(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use serde_json::Value;
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_f64(), y.as_f64()) {
            (Some(x), Some(y)) => (x - y).abs() <= 1e-5 * x.abs().max(1.0),
            _ => x == y,
        },
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w))),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(v, w)| same(v, w)),
        _ => a == b,
    }
}

/// The widest of `labels`, and the gap after it: where controls in a
/// column of labelled ones begin.
pub fn divider_for(labels: &[&str]) -> f32 {
    labels
        .iter()
        .map(|l| f32::from(olo_kit::style::text_width(SharedString::from(l.to_string()), olo_kit::style::PLAIN_SIZE, olo_kit::style::ui_font(olo_kit::FontWeight::NORMAL))).ceil())
        .fold(0.0, f32::max)
        + m::LABEL_GAP
}

/// How long a slider's dragged value is shown in place of what its
/// server says (the server's echoes are a little older: the knob would
/// jump), and after which the panel says what it shows again, so what
/// the server made of it shows (the GPUI kit drew a window again whenever
/// the pointer left a control; a view draws again when told).
pub const HELD: Duration = Duration::from_millis(600);

/// The panel says what it shows again once a value held for [`HELD`] is
/// let go.
pub fn again_once_let_go(cx: &mut Cx<PanelView>) {
    cx.after_do(HELD + Duration::from_millis(50), |_, cx| cx.notify());
}

/// A text field of a panel's own (not bound to a setting: a password, a
/// name to save under): the panel says what it shows again as it's
/// typed in, for what it enables by the text (a Save that wants a name).
/// (A view is told: nothing draws it again because something it read
/// changed, as GPUI did.)
pub fn text_field(cx: &mut Cx<PanelView>, build: impl FnOnce(TextField) -> TextField) -> Child<TextField> {
    let f = olo_kit::widgets::text_field(cx, build);
    cx.subscribe(&f, |_: &mut PanelView, e: &TextFieldEvent, cx| {
        if matches!(e, TextFieldEvent::Changed) {
            cx.notify();
        }
    });
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(f: impl Fn(&mut Settings) + 'static) -> Probe {
        Rc::new(f)
    }

    #[test]
    fn a_box_at_its_defaults_has_no_reset() {
        let d = Settings::default();
        let (speed, follows) = (d.mouse_speed, d.focus_follows_mouse);
        assert!(!changed(&[probe(move |s| s.mouse_speed = speed), probe(move |s| s.focus_follows_mouse = follows)]));
        // (A slider's percent, rounded through f32, is no change.)
        let percent = (speed * 100.0) as f32;
        assert!(!changed(&[probe(move |s| s.mouse_speed = (percent / 100.0) as f64)]));
        assert!(!changed(&[]));
    }

    #[test]
    fn reset_puts_back_just_its_box() {
        let d = Settings::default();
        let mut s = d.clone();
        s.mouse_speed = d.mouse_speed + 0.5;
        s.focus_follows_mouse = !d.focus_follows_mouse;
        // (Another box's.)
        s.tiling.gap = d.tiling.gap + 4;
        let (speed, follows) = (s.mouse_speed, s.focus_follows_mouse);
        let probes = [probe(move |x| x.mouse_speed = speed), probe(move |x| x.focus_follows_mouse = follows)];
        assert!(changed(&probes));
        reset(&mut s, &probes);
        assert_eq!((s.mouse_speed, s.focus_follows_mouse), (d.mouse_speed, d.focus_follows_mouse));
        assert_eq!(s.tiling.gap, d.tiling.gap + 4);
        assert!(!changed(&[probe(move |x| x.mouse_speed = d.mouse_speed)]));
    }

    #[test]
    fn reset_puts_back_one_item_of_a_list() {
        let d = Settings::default();
        assert!(d.workspace_colors.len() >= 2, "the defaults have colours for workspaces");
        let mut s = d.clone();
        s.workspace_colors[0] = [1, 2, 3];
        s.workspace_colors[1] = [4, 5, 6];
        // (A box for the first workspace only.)
        let first = s.workspace_colors[0];
        reset(&mut s, &[probe(move |x| x.workspace_colors[0] = first)]);
        assert_eq!(s.workspace_colors[0], d.workspace_colors[0]);
        assert_eq!(s.workspace_colors[1], [4, 5, 6]);
    }
}

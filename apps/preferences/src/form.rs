//! A panel's controls, top to bottom, each bound to its part of the
//! settings: `Ui` collects them as a panel's `ui` function adds them,
//! and a change made with one goes to
//! [`Prefs::edit`] (app_server's copy, and every window showing it). A
//! panel's own state (what it is busy with, what is selected) is changed
//! through [`Ui::on_view`], on its window's view.

use std::{collections::HashMap, rc::Rc};

use olo_protocol::Settings;
use olo_ui::{
    gpui::{AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, SharedString, Styled as _, WeakEntity, Window, div, px},
    metrics as m,
    widgets::{BBox, Button, CheckBox, MenuField, RadioButton, Slider, StringView, TextControl, TextField, TextFieldEvent},
};

use crate::{PanelView, Prefs};

/// A change to the settings, from a control.
type Set<T> = Rc<dyn Fn(&mut Settings, T)>;

/// A panel's text fields, by id, and what each sets as it is typed in
/// (the latest render's: a field's setter can depend on what is shown,
/// a workspace's number).
#[derive(Default)]
pub struct Fields {
    fields: HashMap<SharedString, Entity<TextField>>,
    setters: HashMap<SharedString, Set<String>>,
}

pub struct Ui<'a, 'b> {
    pub window: &'a mut Window,
    pub cx: &'a mut Context<'b, PanelView>,
    /// The settings as they are now.
    pub s: &'a Settings,
    prefs: WeakEntity<Prefs>,
    fields: &'a mut Fields,
    items: Vec<AnyElement>,
    enabled: bool,
}

impl<'a, 'b> Ui<'a, 'b> {
    pub fn new(window: &'a mut Window, cx: &'a mut Context<'b, PanelView>, s: &'a Settings, prefs: WeakEntity<Prefs>, fields: &'a mut Fields) -> Self {
        Ui { window, cx, s, prefs, fields, items: Vec::new(), enabled: true }
    }

    /// What was added, top to bottom.
    pub fn take(&mut self) -> Vec<AnyElement> {
        std::mem::take(&mut self.items)
    }

    pub fn add(&mut self, element: impl IntoElement) {
        self.items.push(element.into_any_element());
    }

    /// The app's settings, for a callback that changes them later (a
    /// file panel's answer).
    pub fn prefs(&self) -> WeakEntity<Prefs> {
        self.prefs.clone()
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// A change to the settings, for a control's callback.
    pub fn edit<T: 'static>(&self, set: impl Fn(&mut Settings, T) + 'static) -> impl Fn(T, &mut Window, &mut App) + 'static {
        let prefs = self.prefs.clone();
        move |v, _, cx| {
            let _ = prefs.update(cx, |p, cx| p.edit(cx, |s| set(s, v)));
        }
    }

    /// A change to the settings on a click.
    pub fn edit_click(&self, set: impl Fn(&mut Settings) + 'static) -> impl Fn(&mut Window, &mut App) + 'static {
        let prefs = self.prefs.clone();
        move |_, cx| {
            let _ = prefs.update(cx, |p, cx| p.edit(cx, &set));
        }
    }

    /// Something to do to the panel's window (its state), for a control.
    pub fn on_view<T: 'static>(&self, f: impl Fn(&mut PanelView, T, &mut Window, &mut Context<PanelView>) + 'static) -> impl Fn(T, &mut Window, &mut App) + 'static {
        let view = self.cx.entity().downgrade();
        move |v, window, cx| {
            let _ = view.update(cx, |p, cx| f(p, v, window, cx));
        }
    }

    /// [`Ui::on_view`] on a click.
    pub fn on_click_view(&self, f: impl Fn(&mut PanelView, &mut Window, &mut Context<PanelView>) + 'static) -> impl Fn(&mut Window, &mut App) + 'static {
        let view = self.cx.entity().downgrade();
        move |window, cx| {
            let _ = view.update(cx, |p, cx| f(p, window, cx));
        }
    }

    /// What `build` adds, for the caller to lay out.
    pub fn collect(&mut self, build: impl FnOnce(&mut Self)) -> Vec<AnyElement> {
        let outer = std::mem::take(&mut self.items);
        build(self);
        std::mem::replace(&mut self.items, outer)
    }

    /// An etched box with its title in its top line.
    pub fn bbox(&mut self, title: &str, build: impl FnOnce(&mut Self)) {
        let items = self.collect(build);
        let b = if title.is_empty() { BBox::plain() } else { BBox::new(SharedString::from(title.to_owned())) };
        self.add(b.children(items));
    }

    /// Side by side, `gap` apart, lined up on their middles.
    pub fn row_gap(&mut self, gap: f32, build: impl FnOnce(&mut Self)) {
        let items = self.collect(build);
        self.add(div().flex().flex_row().items_center().gap(px(gap)).children(items));
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

    /// A note under a control: dim, wrapped.
    pub fn note(&mut self, text: impl Into<SharedString>) {
        self.add(StringView::new(text).dim(true).wrap(true));
    }

    pub fn check(&mut self, label: impl Into<SharedString>, on: bool, set: impl Fn(&mut Settings, bool) + 'static) {
        let c = CheckBox::new(label, on).enabled(self.enabled).on_toggle(self.edit(set));
        self.add(c);
    }

    pub fn radio(&mut self, label: impl Into<SharedString>, selected: bool, set: impl Fn(&mut Settings) + 'static) {
        let r = RadioButton::new(label, selected).enabled(self.enabled).on_select(self.edit_click(set));
        self.add(r);
    }

    /// A slider; `with` sets its label, text and limits.
    pub fn slider(&mut self, id: &str, value: f32, range: std::ops::RangeInclusive<f32>, with: impl FnOnce(Slider) -> Slider, set: impl Fn(&mut Settings, f32) + 'static) {
        let s = with(Slider::new(SharedString::from(id.to_owned()), value, range)).enabled(self.enabled).on_change(self.edit(set));
        self.add(s);
    }

    /// A pop-up menu of `options`; `with` sets its label, divider, width.
    pub fn menu(&mut self, id: &str, options: Vec<SharedString>, selected: usize, with: impl FnOnce(MenuField) -> MenuField, set: impl Fn(&mut Settings, usize) + 'static) {
        let f = with(MenuField::new(SharedString::from(id.to_owned()), options, selected)).enabled(self.enabled).on_change(self.edit(set));
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
        self.menu(id, names, selected, with, move |s, i| {
            if let Some(v) = values.get(i) {
                set(s, v.clone());
            }
        });
    }

    pub fn button(&mut self, label: impl Into<SharedString>, on_click: impl Fn(&mut Window, &mut App) + 'static) {
        let b = Button::new(label).enabled(self.enabled).on_click(on_click);
        self.add(b);
    }

    /// The text field `id` (made the first time), showing `value` unless
    /// it has the keyboard; typing in it sets what `set` says.
    pub fn field(&mut self, id: &str, value: &str, build: impl FnOnce(TextField) -> TextField, set: impl Fn(&mut Settings, String) + 'static) -> Entity<TextField> {
        let id = SharedString::from(id.to_owned());
        self.fields.setters.insert(id.clone(), Rc::new(set));
        let field = match self.fields.fields.get(&id) {
            Some(f) => f.clone(),
            None => {
                let f = self.cx.new(|cx| build(TextField::new(cx)));
                f.update(self.cx, |f, cx| f.set_text(value.to_owned(), cx));
                let key = id.clone();
                let prefs = self.prefs.clone();
                self.cx
                    .subscribe(&f, move |view, field, e: &TextFieldEvent, cx| {
                        if !matches!(e, TextFieldEvent::Changed) {
                            return;
                        }
                        let text = field.read(cx).text().to_owned();
                        let Some(set) = view.fields.setters.get(&key).cloned() else { return };
                        let _ = prefs.update(cx, |p, cx| p.edit(cx, |s| set(s, text.clone())));
                    })
                    .detach();
                self.fields.fields.insert(id.clone(), f.clone());
                f
            }
        };
        let enabled = self.enabled;
        let focused = field.read(self.cx).is_focused(self.window);
        field.update(self.cx, |f, cx| {
            if !focused && f.text() != value {
                f.set_text(value.to_owned(), cx);
            }
            f.set_enabled(enabled, cx);
        });
        field
    }

    /// A labelled text control bound to a setting; `with` sets its label,
    /// divider and width.
    pub fn text(&mut self, id: &str, value: &str, build: impl FnOnce(TextField) -> TextField, with: impl FnOnce(TextControl) -> TextControl, set: impl Fn(&mut Settings, String) + 'static) {
        let field = self.field(id, value, build, set);
        self.add(with(TextControl::new(field)));
    }
}

/// The widest of `labels`, and the gap after it: where controls in a
/// column of labelled ones begin.
pub fn divider_for(window: &mut Window, labels: &[&str]) -> f32 {
    labels
        .iter()
        .map(|l| f32::from(olo_ui::style::text_width(window, SharedString::from(l.to_string()), olo_ui::style::PLAIN_SIZE, olo_ui::style::ui_font(olo_ui::gpui::FontWeight::NORMAL))).ceil())
        .fold(0.0, f32::max)
        + m::LABEL_GAP
}

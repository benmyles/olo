//! Weather's dialogs: Add Place (find a place by name, or give its
//! coordinates), Rename Place (a name of the person's own for it) and
//! Data Provider (which provider, and its key: typed once and kept in
//! Passwords, or a reference to one already there).

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use olo_kit::{
    ClickEvent, Cx, Element, InteractiveElement as _, IntoElement as _, KeyDownEvent, MouseButton, ParentElement as _, SharedString, Styled as _, View, WindowHandle, WindowOptions, canvas, div,
    metrics as m, px, style, theme,
    widgets::{Button, LinkButton, MenuField, RadioButton, ScrollView, StringView, TextControl, TextField, TextFieldEvent, button_width, text_field},
};

use crate::{
    Msg, ToMain, key,
    places::{self, Place},
    provider::{self, Error},
};

/// The labels' column in both.
const DIVIDER: f32 = 84.0;

/// Add Place: what's typed is looked up as it's typed (a moment after the
/// last key); a result is chosen by a click or the arrow keys, and added
/// with Add or Return.
pub struct AddPlace {
    find: olo_kit::Child<TextField>,
    results: Vec<Place>,
    chosen: usize,
    /// What the list says when it has nothing: searching, nothing found,
    /// what went wrong.
    note: String,
    /// The search under way, by number (an older one's answer is old news).
    searches: u64,
    /// The results' list, five rows tall, scrolling.
    scroll: olo_kit::ScrollHandle,
    to_main: ToMain,
}

/// A result's row in Add Place, and how many show at once.
const RESULT_ROW: f32 = 36.0;
const RESULTS_SHOWN: f32 = 5.0;

pub enum AddMsg {
    /// Look up what's typed, if it's still search `n`'s.
    Search(u64),
    Found(u64, Result<Vec<Place>, Error>),
}

static SEARCHES: AtomicU64 = AtomicU64::new(0);

impl AddPlace {
    /// Scroll the results so the chosen one shows.
    fn reveal_chosen(&self) {
        let top = px(self.chosen as f32 * RESULT_ROW);
        self.scroll.reveal_span(top, top + px(RESULT_ROW));
    }

    pub fn open(to_main: ToMain) -> WindowHandle<AddPlace> {
        olo_kit::open_window(WindowOptions::new("Add Place", 460.0, 300.0).dialog(), move |cx| {
            let find = text_field(cx, |f| f.name("Find").hint("A town, an airfield, or 47.61, -122.33"));
            cx.subscribe(&find, |v: &mut AddPlace, e: &TextFieldEvent, cx| match e {
                TextFieldEvent::Changed => {
                    let n = SEARCHES.fetch_add(1, Ordering::Relaxed) + 1;
                    v.searches = n;
                    cx.after(Duration::from_millis(300), AddMsg::Search(n));
                }
                TextFieldEvent::Entered => v.add(cx),
                _ => {}
            });
            find.focus(cx);
            AddPlace { find, results: Vec::new(), chosen: 0, note: "Type a place’s name, or its latitude and longitude.".into(), searches: 0, scroll: olo_kit::ScrollHandle::new(), to_main }
        })
    }

    fn add(&mut self, cx: &mut Cx<Self>) {
        if let Some(p) = self.results.get(self.chosen) {
            self.to_main.send(Msg::AddPlace(p.clone()));
            cx.close_window();
        }
    }
}

impl View for AddPlace {
    type Message = AddMsg;

    fn message(&mut self, message: AddMsg, cx: &mut Cx<Self>) {
        match message {
            AddMsg::Search(n) => {
                if n != self.searches {
                    return;
                }
                let q = self.find.read(cx).text().trim().to_owned();
                if q.is_empty() {
                    self.results.clear();
                    self.note = "Type a place’s name, or its latitude and longitude.".into();
                } else {
                    self.note = "Searching…".into();
                    cx.work(move || places::search(&q), move |r| AddMsg::Found(n, r));
                }
            }
            AddMsg::Found(n, result) => {
                if n != self.searches {
                    return;
                }
                match result {
                    Ok(found) => {
                        self.note = if found.is_empty() { "Nothing by that name.".into() } else { String::new() };
                        self.results = found;
                        self.chosen = 0;
                        self.reveal_chosen();
                    }
                    Err(e) => {
                        self.results.clear();
                        self.note = e.to_string();
                    }
                }
            }
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let mut list = div().w_full().flex().flex_col();
        if self.results.is_empty() {
            list = list.child(div().p(px(m::TIGHT)).child(StringView::new(self.note.clone()).dim(true).wrap(true)));
        }
        for (i, place) in self.results.iter().enumerate() {
            let chosen = i == self.chosen;
            let name: SharedString = place.name.clone().into();
            let mut about = vec![];
            if !place.region.is_empty() {
                about.push(place.region.clone());
            }
            about.push(place.coordinates());
            let about: SharedString = about.join(" · ").into();
            let (n2, a2) = (name.clone(), about.clone());
            let row = canvas(move |g, b| {
                let p = theme::palette();
                let b = style::snap(g, b);
                let (text, dim) = if chosen {
                    style::selection_plate(g, b, &p);
                    (style::c(p.list_selected_item_text), style::c(p.list_selected_item_text).opacity(0.75))
                } else {
                    (style::c(p.document_text), style::disabled_text(&p))
                };
                // (Inset from the list's outer edge, as the field's text is
                // from its own: the place under what's typed.)
                let s = g.scale_factor();
                let x = b.left() + px(m::TEXT_INSET - style::line_pixels(s) * 2.0 / s);
                style::label_fit(g, x, b.top() + px(11.0), style::Align::Left, n2.clone(), style::PLAIN_SIZE, style::ui_font(olo_kit::FontWeight::BOLD), text, b.size.width - px(12.0));
                style::label_fit(g, x, b.top() + px(26.0), style::Align::Left, a2.clone(), style::SMALL_SIZE, style::ui_font(olo_kit::FontWeight::NORMAL), dim, b.size.width - px(12.0));
                olo_kit::agent::record(
                    g,
                    olo_kit::agent::Node { role: "ListItem", name: Some(n2.clone()), value: Some(a2.clone()), bounds: b, enabled: true, selected: Some(chosen), clickable: true, ..Default::default() },
                );
            })
            .paint_key(style::key((&name, &about, chosen, p.is_dark())))
            .id(SharedString::from(format!("place-{i}")))
            .w_full()
            .h(px(RESULT_ROW))
            .flex_shrink_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |v: &mut AddPlace, e: &olo_kit::MouseDownEvent, cx| {
                    v.chosen = i;
                    if e.click_count >= 2 {
                        v.add(cx);
                    }
                    cx.invalidate();
                }),
            );
            list = list.child(row);
        }
        let bw = button_width("Cancel", false).max(button_width("Add", false));
        div()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::STACK))
            .on_key_down(cx.listener(|v: &mut AddPlace, e: &KeyDownEvent, cx| match e.keystroke.key.as_str() {
                "escape" => cx.close_window(),
                "down" if v.chosen + 1 < v.results.len() => {
                    v.chosen += 1;
                    v.reveal_chosen();
                    cx.invalidate();
                }
                "up" if v.chosen > 0 => {
                    v.chosen -= 1;
                    v.reveal_chosen();
                    cx.invalidate();
                }
                _ => {}
            }))
            .child(TextControl::new(cx, &self.find).label("Find:").divider(DIVIDER))
            // (The first place found a row's pitch under what's typed, at
            // every scale: the list's frame is two device lines, a point or
            // two by the scale, so its top moves by that.)
            .child(
                div()
                    .pl(px(DIVIDER))
                    .mt(px(3.0 - crate::view::edge(cx)))
                    .child(div().h(px(RESULTS_SHOWN * RESULT_ROW + 2.0 * crate::view::edge(cx))).child(ScrollView::new(&self.scroll).background(p.list_background).child(list))),
            )
            .child(
                div()
                    .mt(px(m::SPACING))
                    .flex()
                    .justify_end()
                    .gap(px(m::SPACING))
                    .child(Button::new("Cancel").min_width(bw).on_click(cx.listener(|_: &mut AddPlace, _: &ClickEvent, cx| cx.close_window())))
                    .child(Button::new("Add").default(true).enabled(!self.results.is_empty()).min_width(bw).on_click(cx.listener(|v: &mut AddPlace, _: &ClickEvent, cx| v.add(cx)))),
            )
            .into_element()
    }
}

/// Data Provider: which provider, and its key.
pub struct KeyDialog {
    provider: usize,
    /// A reference rather than a key typed.
    by_reference: bool,
    key: olo_kit::Child<TextField>,
    reference: olo_kit::Child<TextField>,
    /// Where the key is now.
    now: key::Source,
    note: String,
    wrong: bool,
    saving: bool,
    to_main: ToMain,
}

pub enum KeyMsg {
    Kept(Result<(), Error>),
}

impl KeyDialog {
    pub fn open(provider: String, now: key::Source, to_main: ToMain) -> WindowHandle<KeyDialog> {
        olo_kit::open_window(WindowOptions::new("Data Provider", 500.0, 280.0).dialog(), move |cx| {
            let key = text_field(cx, |f| f.name("API key").password(true).hint("Paste the key"));
            let reference = text_field(cx, |f| f.name("Reference").hint("op://vault/item/field"));
            for field in [&key, &reference] {
                cx.subscribe(field, |v: &mut KeyDialog, e: &TextFieldEvent, cx| match e {
                    TextFieldEvent::Entered => v.save(cx),
                    TextFieldEvent::Changed => {
                        if v.wrong {
                            v.wrong = false;
                            v.note.clear();
                        }
                        cx.invalidate();
                    }
                    _ => {}
                });
            }
            let by_reference = matches!(now, key::Source::Reference { .. });
            if let key::Source::Reference { reference: r } = &now {
                let r = r.clone();
                reference.update(cx, move |f, cx| f.set_text(&r, cx));
            }
            if by_reference {
                reference.focus(cx)
            } else {
                key.focus(cx)
            }
            let provider = provider::all().iter().position(|p| p.id() == provider).unwrap_or(0);
            KeyDialog { provider, by_reference, key, reference, now, note: String::new(), wrong: false, saving: false, to_main }
        })
    }

    fn chosen(&self) -> &'static dyn provider::Provider {
        provider::all()[self.provider.min(provider::all().len() - 1)]
    }

    fn save(&mut self, cx: &mut Cx<Self>) {
        let provider = self.chosen();
        if !provider.needs_key() {
            self.to_main.send(Msg::Provider(provider.id().into(), key::Source::None));
            return cx.close_window();
        }
        if self.by_reference {
            let r = self.reference.read(cx).text().trim().to_owned();
            if !key::is_reference(&r) {
                self.note = "A reference looks like op://vault/item/field (Passwords’ Copy Secret Reference gives one).".into();
                self.wrong = true;
                return cx.invalidate();
            }
            self.to_main.send(Msg::Provider(provider.id().into(), key::Source::Reference { reference: r }));
            return cx.close_window();
        }
        let typed = zeroize::Zeroizing::new(self.key.read(cx).text().trim().to_owned());
        if typed.is_empty() {
            // Nothing new: the key there is, if there is one.
            if self.now != key::Source::None {
                self.to_main.send(Msg::Provider(provider.id().into(), self.now.clone()));
                return cx.close_window();
            }
            self.note = "Paste the key first.".into();
            self.wrong = true;
            return cx.invalidate();
        }
        self.saving = true;
        self.note = "Keeping it in Passwords…".into();
        self.wrong = false;
        cx.work(move || key::keep(provider, &typed), KeyMsg::Kept);
        cx.invalidate();
    }
}

impl View for KeyDialog {
    type Message = KeyMsg;

    fn message(&mut self, message: KeyMsg, cx: &mut Cx<Self>) {
        let KeyMsg::Kept(result) = message;
        self.saving = false;
        match result {
            Ok(()) => {
                self.key.update(cx, |f, cx| f.set_text("", cx));
                self.to_main.send(Msg::Provider(self.chosen().id().into(), key::Source::Kept));
                return cx.close_window();
            }
            Err(e) => {
                self.note = e.to_string();
                self.wrong = true;
            }
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let names: Vec<SharedString> = provider::all().iter().map(|p| SharedString::from(p.name())).collect();
        let provider = self.chosen();
        let mut col = div().p(px(m::WINDOW_INSET)).flex().flex_col().gap(px(m::STACK)).on_key_down(cx.listener(|_: &mut KeyDialog, e: &KeyDownEvent, cx| {
            if e.keystroke.key == "escape" {
                cx.close_window();
            }
        }));
        col = col
            .child(MenuField::new("provider", names, self.provider).label("Provider:").divider(DIVIDER).on_change(cx.listener(|v: &mut KeyDialog, i: &usize, cx| {
                v.provider = *i;
                cx.invalidate();
            })))
            .child(div().pl(px(DIVIDER)).child(StringView::new(provider.about()).dim(true).wrap(true)));
        if provider.needs_key() {
            let now = match &self.now {
                key::Source::None => "No key yet.".to_owned(),
                key::Source::Kept => format!("Now: kept in Passwords as “{}”.", key::title(provider)),
                key::Source::Reference { reference } => format!("Now: {reference}, in Passwords."),
            };
            col = col.child(div().pl(px(DIVIDER)).pt(px(m::TIGHT)).child(StringView::new(now).wrap(true)));
            col = col.child(
                div()
                    .pl(px(DIVIDER))
                    .flex()
                    .flex_col()
                    .child(RadioButton::new("Give a new key", !self.by_reference).id("new-key").on_select(cx.listener(|v: &mut KeyDialog, _: &(), cx| {
                        v.by_reference = false;
                        v.key.focus(cx);
                        cx.invalidate();
                    })))
                    .child(RadioButton::new("Use a key already in Passwords", self.by_reference).id("by-reference").on_select(cx.listener(|v: &mut KeyDialog, _: &(), cx| {
                        v.by_reference = true;
                        v.reference.focus(cx);
                        cx.invalidate();
                    }))),
            );
            if self.by_reference {
                col = col.child(TextControl::new(cx, &self.reference).label("Reference:").divider(DIVIDER)).child(div().pl(px(DIVIDER)).child(
                    StringView::new("Its secret reference, as Passwords’ Copy Secret Reference gives it. Passwords asks once, while it runs, whether Weather may have it.").dim(true).wrap(true),
                ));
            } else {
                let url = provider.key_url();
                col = col.child(TextControl::new(cx, &self.key).label("API key:").divider(DIVIDER)).child(
                    div()
                        .pl(px(DIVIDER))
                        .flex()
                        .flex_col()
                        .gap(px(m::TIGHT))
                        .child(StringView::new(format!("It’s kept in Passwords as “{}”, and nowhere else.", key::title(provider))).dim(true).wrap(true))
                        .child(div().flex().child(LinkButton::new(format!("Make a free key at {}", provider.name())).id("make-key").on_click(move |_: &ClickEvent, _| {
                            let _ = olo_xdg::open::open(&olo_xdg::open::Target::Url(url.to_owned()));
                        }))),
                );
            }
        }
        if !self.note.is_empty() {
            let ink = if self.wrong { style::c(p.failure) } else { style::disabled_text(&p) };
            col = col.child(div().pl(px(DIVIDER)).child(StringView::new(self.note.clone()).color(ink).wrap(true)));
        }
        let bw = button_width("Cancel", false).max(button_width("Save", false));
        let saving = self.saving;
        col.child(
            div()
                .mt(px(m::SPACING))
                .flex()
                .justify_end()
                .gap(px(m::SPACING))
                .child(Button::new("Cancel").min_width(bw).on_click(cx.listener(|_: &mut KeyDialog, _: &ClickEvent, cx| cx.close_window())))
                .child(Button::new("Save").default(true).enabled(!saving).min_width(bw).on_click(cx.listener(|v: &mut KeyDialog, _: &ClickEvent, cx| v.save(cx)))),
        )
        .into_element()
    }
}

/// Rename Place: a name of the person's own for a kept place ("Home"),
/// shown for it everywhere; empty, it goes back to the name it was found
/// as (the field's hint).
pub struct RenamePlace {
    index: usize,
    name: olo_kit::Child<TextField>,
    /// What it was found as, with where.
    found: String,
    to_main: ToMain,
}

impl RenamePlace {
    pub fn open(index: usize, place: &Place, to_main: ToMain) -> WindowHandle<RenamePlace> {
        let (label, hint) = (place.label.clone().unwrap_or_default(), place.name.clone());
        let found = if place.region.is_empty() { place.name.clone() } else { format!("{}, {}", place.name, place.region) };
        olo_kit::open_window(WindowOptions { fixed: true, ..WindowOptions::new("Rename Place", 420.0, 130.0).dialog() }, move |cx| {
            let name = text_field(cx, |f| f.name("Name").hint(hint).with_text(label));
            name.update(cx, |f, cx| f.select_all_text(cx));
            name.focus(cx);
            cx.subscribe(&name, |v: &mut RenamePlace, e: &TextFieldEvent, cx| {
                if matches!(e, TextFieldEvent::Entered) {
                    v.done(true, cx);
                }
            });
            RenamePlace { index, name, found, to_main }
        })
    }

    fn done(&mut self, ok: bool, cx: &mut Cx<Self>) {
        if ok {
            let typed = self.name.read(cx).text().trim().to_owned();
            self.to_main.send(Msg::RenamePlace(self.index, typed));
        }
        cx.close_window();
    }
}

impl View for RenamePlace {
    type Message = ();

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let bw = button_width("Rename", false).max(button_width("Cancel", false));
        div()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::STACK))
            .on_key_down(cx.listener(|v: &mut RenamePlace, e: &KeyDownEvent, cx| {
                if e.keystroke.key == "escape" {
                    v.done(false, cx);
                }
            }))
            .child(TextControl::new(cx, &self.name).label("Name:").divider(DIVIDER))
            .child(div().pl(px(DIVIDER)).child(StringView::new(format!("Found as {}. Leave it empty for that name.", self.found)).dim(true).wrap(true)))
            .child(
                div()
                    .mt(px(m::SPACING))
                    .flex()
                    .justify_end()
                    .gap(px(m::SPACING))
                    .child(Button::new("Cancel").min_width(bw).on_click(cx.listener(|v: &mut RenamePlace, _: &ClickEvent, cx| v.done(false, cx))))
                    .child(Button::new("Rename").default(true).min_width(bw).on_click(cx.listener(|v: &mut RenamePlace, _: &ClickEvent, cx| v.done(true, cx))))
                    .into_element(),
            )
    }
}

//! Themes: the whole look, everywhere (Deskbar, Tracker, Twitcher, window
//! tabs and frames, every app, GTK and Chrome too). One list (Ben,
//! 2026-11-17: "let's just have Classic, Platinum and BeOS as their own
//! themes (as well as all the other themes)"): every theme down the left,
//! each with its colours at a glance (light beside dark); the chosen one
//! shown at the right, drawn in itself: a preview, a small desktop (its
//! wallpaper as the mode is now, a window in its colours holding its
//! icons, the desk's own icons at the right), its name and a line about
//! it, what it comes with. Choosing one
//! sets `Appearance::look`, which every app follows at once, and the
//! wallpaper to the theme's (`Appearance::wallpaper_of`), which
//! Backgrounds can change after.
//!
//! **Customize…** (Ben: "click on a theme then press Customize and then
//! i can change everything about it -- any of the colors, the icon set,
//! the wallpaper etc."): the chosen theme's colours (each of
//! `looks::ROLES`, light and dark apart, with a colour control), its
//! icons (one of `looks::ICON_SETS`) and its wallpaper (any of the
//! pack's). What's changed is kept by theme (`Appearance::custom`) and
//! shows everywhere as it's changed; Revert puts the theme's own back.

use std::{rc::Rc, sync::Arc};

use olo_kit::{
    Bounds, Corners, Cx, FontWeight, Hsla, Painter, ParentElement as _, Pixels, RenderImage, ScrollHandle, SharedString, Styled as _, canvas, div, icons, metrics as m,
    palette::{
        Palette, Rgba, ThemeMode,
        looks::{self, ICON_SETS, ROLES, THEMES, Theme},
    },
    pictures, point, px, size,
    style::{self, c},
    theme,
    widgets::{
        BBox, Button, ColorControl, ColorWell, Column, ColumnListView, ListModel, MenuField, RadioButton, SCROLL_BAR, ScrollView, StringView, TabView, Thumbnail, ThumbnailPicker, color_control_height,
    },
};
use olo_protocol::{Appearance, Settings};
use olo_xdg::wallpapers::{self, Wallpaper};

use crate::{PanelView, form::Ui, panel};

/// The themes' list: its width, a row's height, and the colours beside
/// each name (light and dark, side by side).
const LIST_W: f32 = 236.0;
const ROW: f32 = 26.0;
const GLANCE: (f32, f32) = (40.0, 18.0);
/// A theme's preview: a small desktop, the shape of a laptop's screen
/// (16 to 10), with the theme's icons in it (Ben, 2026-11-17, of a row of
/// icons under a smaller preview: "this looks pretty cramped and janky
/// like where the icons are the spacing is shitty. why not just make the
/// preview a bit bigger and have the icons in the preview change").
const PREVIEW_ASPECT: f32 = 0.625;
/// The icons each icon set is shown with in Customize, and their size.
const SAMPLES: &[&str] = &["folder", "text", "application", "terminal", "mail", "disk", "trash-full"];
const ICON: f32 = 32.0;
/// The Olo mark beside a theme's name (the brand's, the same in every
/// theme).
const MARK: f32 = 24.0;
/// Customize: the colours' list (its width, a colour's sample in a row),
/// the wallpaper previews' size (16:9) and what's round them.
const ROLES_W: f32 = 220.0;
const SAMPLE: f32 = 14.0;
const WALLPAPER: (f32, f32) = (96.0, 54.0);
const LIST_PAD: f32 = 6.0;
/// A list's sunken frame, both sides, and a little: its one column is
/// as wide as what's left beside the scroll bar, so it never scrolls
/// sideways.
const LIST_FRAME: f32 = 8.0;

pub struct State {
    themes: ListModel,
    roles: ListModel,
    /// The chosen theme is being customized (Colors, Icons, Wallpaper),
    /// not the list shown.
    customizing: bool,
    tab: usize,
    /// Which mode's colours are being changed: the one in effect, until
    /// the other is chosen.
    mode: Option<ThemeMode>,
    /// The wallpaper pack, read once; its collection shown (the theme's
    /// wallpaper's, until another is chosen); the pictures' list.
    pack: Option<Vec<Wallpaper>>,
    collection: Option<String>,
    scroll: ScrollHandle,
}

impl State {
    pub fn new() -> State {
        let roles = ListModel::new(false);
        roles.state_mut().select(0);
        State { themes: ListModel::new(false), roles, customizing: false, tab: 0, mode: None, pack: None, collection: None, scroll: ScrollHandle::default() }
    }
}

/// For the lint: the chosen theme being customized, at tab `tab`
/// (Colors, Icons, Wallpaper).
#[cfg(test)]
pub fn for_lint(st: &mut State, tab: usize) {
    st.customizing = true;
    st.tab = tab;
}

fn state_of(v: &mut PanelView) -> Option<&mut State> {
    match &mut v.state {
        panel::State::Themes(st) => Some(st),
        _ => None,
    }
}

fn hsla(rgba: Rgba) -> Hsla {
    c(rgba).into()
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let s = ui.s.clone();
    let chosen = looks::resolve(&s.appearance.look);
    // The list's selection is the theme in use, whoever chose it.
    let at = THEMES.iter().position(|t| t.id == chosen.id).unwrap_or(0);
    if st.themes.state().single() != Some(at) {
        st.themes.state_mut().select(at);
    }
    if st.customizing { customize(ui, st, &s, chosen) } else { browse(ui, st, &s, chosen) }
}

/// The themes down the left, the chosen one at the right.
fn browse(ui: &mut Ui, st: &mut State, s: &Settings, chosen: &'static Theme) {
    ui.note("The look of every icon, window and control, in every app, and a wallpaper to match.");
    let a = s.appearance.clone();
    let dark = a.effective_theme == ThemeMode::Dark;
    let wide = f32::from(ui.cx.viewport_size().width) - 2.0 * m::WINDOW_INSET;
    let right_w = wide - LIST_W - m::SPACING;

    // The list: each theme's colours, light and dark, then its name. (Made
    // as the panel is built, not as a row is drawn.)
    let glances: Rc<Vec<[Palette; 2]>> = Rc::new(THEMES.iter().map(|t| [a.palette_of(t.id, ThemeMode::Light), a.palette_of(t.id, ThemeMode::Dark)]).collect());
    let names: Rc<Vec<SharedString>> = Rc::new(THEMES.iter().map(|t| if a.custom_of(t.id).is_some() { format!("{}, customized", t.name).into() } else { t.name.into() }).collect());
    let choose = ui.edit(|s, id: String| choose(s, &id));
    let model = st.themes.clone();
    let list = ColumnListView::new(ui.cx, "themes", &st.themes, vec![Column::new("Theme", LIST_W - SCROLL_BAR - LIST_FRAME)], THEMES.len(), {
        let (glances, names) = (glances.clone(), names.clone());
        move |_: &PanelView, cell, g| {
            let b = cell.bounds;
            let glance = style::snap(g, Bounds { origin: point(b.left() + px(m::CELL_INSET), b.center().y - px(GLANCE.1 / 2.0)), size: size(px(GLANCE.0), px(GLANCE.1)) });
            paint_glance(g, glance, &glances[cell.row]);
            let x = glance.right() + px(m::TIGHT + 2.0);
            style::label_fit(
                g,
                x,
                b.center().y,
                style::Align::Left,
                names[cell.row].clone(),
                style::PLAIN_SIZE,
                style::ui_font(FontWeight::NORMAL),
                cell.text_color,
                b.right() - x - px(m::CELL_INSET),
            );
            cell.say(names[cell.row].clone());
        }
    })
    .no_header()
    .row_height(ROW)
    .type_select(|_: &PanelView, row| THEMES[row].name.into())
    .on_select(move |_: &(), cx| {
        if let Some(theme) = model.state().single().and_then(|i| THEMES.get(i)) {
            choose(&theme.id.to_owned(), cx);
        }
    });

    // The chosen one, drawn in itself.
    let id = chosen.id;
    let look = Arc::new(a.palette());
    let set = a.icons();
    let preview_h = (right_w * PREVIEW_ASPECT).round();
    // (Wide enough that a 16 to 9 picture covers the preview's height.)
    let px_wide = (right_w.max(preview_h * 16.0 / 9.0) * ui.cx.scale_factor()).ceil() as u32;
    // Its wallpaper as the mode is now, made at the preview's device
    // width on a worker (drawn when it's ready).
    let picture = wallpapers::resolve(&a.wallpaper_of(id), (3840, 2160), dark).and_then(|p| {
        pictures::thumbnail_soon(&p, px_wide, {
            let again = ui.cx.invalidator();
            move || again.invalidate()
        })
    });
    let preview = canvas({
        let look = look.clone();
        move |g, b| paint_theme_preview(g, b, &look, set, picture.clone())
    })
    .w_full()
    .h(px(preview_h));
    let mark = canvas(move |g, b| {
        icons::paint_in(g, style::snap_centred(g, b.center(), MARK, MARK), set, "olo");
    })
    .w(px(MARK))
    .h(px(MARK))
    .flex_shrink_0();
    // What it comes with, as the person has it.
    let pack = st.pack.get_or_insert_with(wallpapers::pack);
    let wallpaper = a.wallpaper_of(id);
    let wallpaper = pack.iter().find(|w| w.setting() == wallpaper).map(|w| w.label.clone()).unwrap_or_else(|| "your own picture".into());
    let facts = format!("{} icons. Wallpaper: {wallpaper}.{}", looks::icon_set(set).name, if a.custom_of(id).is_some() { " Customized." } else { "" });
    let customize = ui.on_click_view(|v, cx| {
        if let Some(st) = state_of(v) {
            st.customizing = true;
        }
        cx.notify();
    });
    let right = div()
        .w(px(right_w))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap(px(m::SPACING))
        .child(preview)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(m::TIGHT))
                .child(div().flex().flex_row().items_center().justify_between().child(olo_kit::widgets::heading(chosen.name)).child(mark))
                .child(StringView::new(chosen.about).wrap(true)),
        )
        // What it comes with, and Customize… at its right.
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(m::SPACING))
                .child(div().flex_1().min_w(px(0.0)).child(StringView::new(facts).dim(true).wrap(true)))
                .child(Button::new("Customize…").id("customize").on_click(customize)),
        );
    ui.add(
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_row()
            .gap(px(m::SPACING))
            .child(div().w(px(LIST_W)).flex_shrink_0().flex().flex_col().child(div().flex_1().min_h(px(3.0 * ROW)).child(list)))
            .child(right),
    );
}

/// The chosen theme, customized: its colours, its icons, its wallpaper.
fn customize(ui: &mut Ui, st: &mut State, s: &Settings, t: &'static Theme) {
    let a = s.appearance.clone();
    let id = t.id;
    let tabs: Vec<SharedString> = vec!["Colors".into(), "Icons".into(), "Wallpaper".into()];
    let tab = st.tab.min(tabs.len() - 1);
    let pick = ui.on_view(|v, i: &usize, cx| {
        if let Some(st) = state_of(v) {
            st.tab = *i;
        }
        cx.notify();
    });
    let items = ui.collect(|ui| match tab {
        0 => colors(ui, st, &a, t),
        1 => icon_sets(ui, &a, t),
        _ => wallpaper(ui, st, s, t),
    });
    ui.add(olo_kit::widgets::heading(format!("Customize {}", t.name)));
    ui.add(
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .child(TabView::new("customize-tabs", tabs, tab).grows().on_select(pick).child(div().flex_1().min_h(px(0.0)).flex().flex_col().gap(px(m::SPACING)).p(px(m::SPACING)).children(items))),
    );
    // Everything back as the theme comes; and back to the list.
    let revert = ui.edit_click(move |s| {
        s.appearance.custom.remove(id);
        s.wallpaper = Some(s.appearance.wallpaper_of(id));
    });
    let done = ui.on_click_view(|v, cx| {
        if let Some(st) = state_of(v) {
            st.customizing = false;
        }
        cx.notify();
    });
    ui.add(
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(Button::new(format!("Restore {}’s Own", t.name)).id("revert-theme").enabled(a.custom_of(id).is_some()).on_click(revert))
            .child(Button::new("Done").id("customize-done").default(true).on_click(done)),
    );
    // (Done ends Customize: the panel's own Defaults and Revert, which
    // are about every theme, a section under it.)
    ui.space(m::SECTION - m::WINDOW_INSET);
}

/// Customize ▸ Colors: light or dark; the colours that can be changed,
/// each as it is now; the chosen one's control.
fn colors(ui: &mut Ui, st: &mut State, a: &Appearance, t: &'static Theme) {
    let id = t.id;
    let mode = st.mode.unwrap_or(a.effective_theme);
    // The theme's seeds, and as the person has them.
    let own = t.seeds(mode);
    let mut mine = own;
    if let Some(custom) = a.custom_of(id) {
        for (role, colour) in custom.colors(mode) {
            if let Some(rgba) = looks::parse_hex(colour) {
                mine.set(role, rgba);
            }
        }
    }
    let now: Rc<Vec<Rgba>> = Rc::new(ROLES.iter().map(|r| mine.get(r.id, mode).unwrap_or_default()).collect());
    let changed: Rc<Vec<bool>> = Rc::new(ROLES.iter().map(|r| mine.get(r.id, mode) != own.get(r.id, mode)).collect());
    ui.row(|ui| {
        ui.add(StringView::new("Colors in:"));
        for (m, name) in [(ThemeMode::Light, "Light"), (ThemeMode::Dark, "Dark")] {
            let pick = ui.on_view(move |v, _: &(), cx| {
                if let Some(st) = state_of(v) {
                    st.mode = Some(m);
                }
                cx.notify();
            });
            ui.add(RadioButton::new(name, m == mode).id(format!("colors-in-{}", name.to_lowercase())).on_select(pick));
        }
        if mode != a.effective_theme {
            ui.add(
                StringView::new(format!(
                    "(The desktop is {} now: these show when it's {}.)",
                    if a.effective_theme == ThemeMode::Dark { "dark" } else { "light" },
                    if mode == ThemeMode::Dark { "dark" } else { "light" }
                ))
                .dim(true),
            );
        }
    });
    let chosen = st.roles.state().single().filter(|i| *i < ROLES.len()).unwrap_or(0);
    let list = ColumnListView::new(ui.cx, "theme-colors", &st.roles, vec![Column::new("Color", ROLES_W - SCROLL_BAR - LIST_FRAME)], ROLES.len(), {
        let (now, changed) = (now.clone(), changed.clone());
        move |_: &PanelView, cell, g| {
            let b = cell.bounds;
            let p = theme::palette();
            let well = style::snap(g, Bounds { origin: point(b.left() + px(m::CELL_INSET), b.center().y - px(SAMPLE / 2.0)), size: size(px(SAMPLE), px(SAMPLE)) });
            style::fill_rect(g, well, c(now[cell.row]));
            style::frame_rect(g, well, c(p.control_border));
            let x = well.right() + px(m::TIGHT + 2.0);
            // (One that's changed: in bold.)
            let font = style::ui_font(if changed[cell.row] { FontWeight::BOLD } else { FontWeight::NORMAL });
            style::label_fit(g, x, b.center().y, style::Align::Left, ROLES[cell.row].name, style::PLAIN_SIZE, font, cell.text_color, b.right() - x - px(m::CELL_INSET));
            cell.say(if changed[cell.row] { format!("{}, changed", ROLES[cell.row].name) } else { ROLES[cell.row].name.to_owned() });
        }
    })
    .no_header()
    .type_select(|_: &PanelView, row| ROLES[row].name.into())
    .on_select(ui.cx.listener(|_: &mut PanelView, _: &(), cx| cx.notify()));

    // The chosen colour: what it's for, its control, and the theme's own back.
    let role = &ROLES[chosen];
    let [r, g, b, _] = now[chosen];
    let set = ui.bind([r, g, b], move |s, rgb: [u8; 3]| set_colour(s, id, mode, role.id, Some(rgb)));
    let h = color_control_height();
    let wide = f32::from(ui.cx.viewport_size().width) - 2.0 * (m::WINDOW_INSET + m::SPACING) - ROLES_W - m::SPACING - 2.0 * m::BOX_INSET;
    let ramps = (wide - h - m::SPACING - 96.0 - m::SPACING).clamp(160.0, 260.0);
    let control = ColorControl::new(format!("theme-colour-{}-{}-{}", id, if mode == ThemeMode::Dark { "dark" } else { "light" }, role.id), [r, g, b]).ramp_width(ramps).on_change(set).build(ui.cx);
    let back = ui.edit_click(move |s| set_colour(s, id, mode, role.id, None));
    let about = if role.about.is_empty() {
        format!("One of the colours things are told apart by: tags, graphs, a tab group's colour, Terminal's {}.", role.name.to_lowercase())
    } else {
        role.about.to_owned()
    };
    let editor = BBox::new(role.name)
        .child(StringView::new(about).wrap(true))
        .child(div().flex().flex_row().gap(px(m::SPACING)).child(ColorWell::new(hsla(now[chosen]), h)).child(control))
        .child(StringView::new("What is made from it (shades, edges, text that has to read on it) follows.").dim(true).wrap(true))
        .child(div().flex().flex_row().child(Button::new("The Theme’s Own").id("revert-colour").enabled(changed[chosen]).on_click(back)));
    ui.add(
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_row()
            .gap(px(m::SPACING))
            .child(div().w(px(ROLES_W)).flex_shrink_0().flex().flex_col().child(div().flex_1().min_h(px(3.0 * m::LIST_ROW)).child(list)))
            .child(div().flex_1().min_w(px(0.0)).flex().flex_col().child(editor)),
    );
}

/// Customize ▸ Icons: the sets, each with a few of its icons.
fn icon_sets(ui: &mut Ui, a: &Appearance, t: &'static Theme) {
    let id = t.id;
    let now = a.icons_of(id);
    ui.add(StringView::new(format!("The icons {} is drawn with, everywhere.", t.name)).wrap(true));
    for set in ICON_SETS {
        let (set_id, own) = (set.id, set.id == t.icons);
        let pick = ui.bind_click(set_id == now, move |s| {
            let custom = s.appearance.custom.entry(id.to_owned()).or_default();
            custom.icons = (!own).then(|| set_id.to_owned());
            if custom.is_empty() {
                s.appearance.custom.remove(id);
            }
        });
        let samples = canvas(move |g, b| {
            for (k, name) in SAMPLES.iter().enumerate() {
                let at = Bounds { origin: point(b.left() + px(k as f32 * (ICON + m::SPACING)), b.top()), size: size(px(ICON), px(ICON)) };
                icons::paint_in(g, at, set_id, name);
            }
        })
        .w(px(SAMPLES.len() as f32 * (ICON + m::SPACING) - m::SPACING))
        .h(px(ICON))
        .flex_shrink_0();
        let name = if own { format!("{} (the theme’s own)", set.name) } else { set.name.to_owned() };
        ui.add(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(m::SECTION))
                .child(
                    div()
                        .w(px(250.0))
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .child(RadioButton::new(name, set_id == now).id(format!("icons-{set_id}")).on_select(pick))
                        .child(div().pl(px(m::MARK_SIZE + m::MARK_GAP)).child(StringView::new(set.about).dim(true).wrap(true))),
                )
                .child(samples),
        );
    }
}

/// Customize ▸ Wallpaper: the theme's own, or any of the pack's (a
/// collection at a time).
fn wallpaper(ui: &mut Ui, st: &mut State, s: &Settings, t: &'static Theme) {
    let id = t.id;
    let a = &s.appearance;
    let dark = a.effective_theme == ThemeMode::Dark;
    let all = st.pack.get_or_insert_with(wallpapers::pack).clone();
    let mut collections: Vec<String> = Vec::new();
    for w in all.iter().filter(|w| w.tile.is_none()) {
        if !collections.contains(&w.collection) {
            collections.push(w.collection.clone());
        }
    }
    let now = a.wallpaper_of(id);
    let own = olo_xdg::themes::wallpaper(id);
    let current = all.iter().find(|p| p.setting() == now).map(|p| p.collection.clone());
    let shown = st.collection.clone().filter(|c| collections.contains(c)).or(current).or_else(|| collections.first().cloned()).unwrap_or_default();
    let pack: Vec<Wallpaper> = all.iter().filter(|w| w.collection == shown && w.tile.is_none()).cloned().collect();
    ui.add(StringView::new(format!("The picture on the desktop when {} is chosen. Backgrounds changes the desktop’s after, a workspace at a time.", t.name)).wrap(true));
    if collections.len() > 1 {
        let at = collections.iter().position(|c| *c == shown).unwrap_or(0);
        let names: Vec<SharedString> = collections.iter().map(|c| c.clone().into()).collect();
        let pick = ui.on_view({
            let collections = collections.clone();
            move |v, i: &usize, cx: &mut Cx<PanelView>| {
                if let Some(st) = state_of(v) {
                    st.collection = collections.get(*i).cloned();
                }
                cx.notify();
            }
        });
        ui.add(MenuField::new("theme-wallpaper-collection", names, at).label("Collection:").divider(80.0).on_change(pick));
    }
    let px_wide = (WALLPAPER.0 * ui.cx.scale_factor()).round() as u32;
    let thumb = |name: &str, label: String| {
        let picture = wallpapers::thumbnail(name, dark).and_then(|p| {
            pictures::thumbnail_soon(&p, px_wide, {
                let again = ui.cx.invalidator();
                move || again.invalidate()
            })
        });
        Thumbnail { picture, color: None, label: label.into(), key: None }
    };
    // The theme's own first, then the collection's.
    let mut items = vec![thumb(t.wallpaper, "The Theme’s Own".into())];
    items.extend(pack.iter().map(|w| thumb(&w.name, w.label.clone())));
    let selected = if now == own { Some(0) } else { pack.iter().position(|p| p.setting() == now).map(|i| i + 1) };
    let set = ui.bind(selected.unwrap_or(0), {
        let pack = pack.clone();
        move |s, i: usize| {
            let picked = i.checked_sub(1).and_then(|i| pack.get(i)).map(Wallpaper::setting).filter(|w| *w != olo_xdg::themes::wallpaper(id));
            let custom = s.appearance.custom.entry(id.to_owned()).or_default();
            custom.wallpaper = picked;
            if custom.is_empty() {
                s.appearance.custom.remove(id);
            }
            // (The theme is the one in use: its wallpaper is the desktop's.)
            s.wallpaper = Some(s.appearance.wallpaper_of(id));
        }
    });
    let wide = f32::from(ui.cx.viewport_size().width) - 2.0 * (m::WINDOW_INSET + m::SPACING) - SCROLL_BAR - 2.0 * LIST_PAD - 4.0;
    let picker = ThumbnailPicker::new("theme-wallpaper", items, selected).picture_size(WALLPAPER.0, WALLPAPER.1).width(wide).on_select(move |i: &usize, cx| set(i, cx));
    let row = olo_kit::widgets::thumbs::cell_size(WALLPAPER).1 + 2.0 * LIST_PAD + 2.0;
    ui.add(div().flex_1().min_h(px(row)).child(ScrollView::new(&st.scroll).child(div().p(px(LIST_PAD)).child(picker))));
}

/// Colour `role` of theme `theme` in `mode` set to `rgb` (none, or the
/// theme's own colour: the theme's own again).
fn set_colour(s: &mut Settings, theme: &str, mode: ThemeMode, role: &str, rgb: Option<[u8; 3]>) {
    let own = looks::resolve(theme).seeds(mode).get(role, mode);
    let custom = s.appearance.custom.entry(theme.to_owned()).or_default();
    match rgb {
        Some([r, g, b]) if own != Some([r, g, b, 255]) => {
            custom.colors_mut(mode).insert(role.to_owned(), format!("#{r:02x}{g:02x}{b:02x}"));
        }
        _ => {
            custom.colors_mut(mode).remove(role);
        }
    }
    if custom.is_empty() {
        s.appearance.custom.remove(theme);
    }
}

/// A theme's colours at a glance in `r`: its light palette on the left
/// and its dark one on the right, each a panel with a focused tab and a
/// document with a chosen row, framed.
fn paint_glance(g: &mut Painter, r: Bounds<Pixels>, pair: &[Palette; 2]) {
    let frame = theme::palette().control_border;
    let half = (f32::from(r.size.width) / 2.0).floor();
    for (k, p) in pair.iter().enumerate() {
        let w = if k == 0 { half } else { f32::from(r.size.width) - half };
        let side = Bounds { origin: point(r.left() + px(k as f32 * half), r.top()), size: size(px(w), r.size.height) };
        style::fill_rect(g, side, c(p.panel_background));
        let at = |x: f32, y: f32, w: f32, h: f32| Bounds { origin: point(side.left() + px(x), side.top() + px(y)), size: size(px(w), px(h)) };
        let (w, h) = (f32::from(side.size.width), f32::from(side.size.height));
        // Its tab, then a document with its highlight under it.
        style::fill_rect(g, at(3.0, 3.0, (w * 0.5).round(), 4.0), c(p.tab_focused));
        style::fill_rect(g, at(3.0, 7.0, w - 6.0, h - 10.0), c(p.document_background));
        style::fill_rect(g, at(3.0, h - 3.0 - ((h - 10.0) / 2.0).floor(), w - 6.0, ((h - 10.0) / 2.0).floor()), c(p.menu_selected_background));
    }
    style::frame_rect(g, r, c(frame));
}

/// The icons in a preview's window, in the order they're shown (as many
/// as its rows and columns hold), and on its desk.
const WINDOW_ICONS: &[&str] = &[
    "folder",
    "text",
    "image",
    "audio",
    "application",
    "terminal",
    "mail",
    "archive",
    "video",
    "pdf",
    "font",
    "spreadsheet",
    "calculator",
    "styled-edit",
    "weather",
    "preferences",
    "browser",
    "tracker",
    "passwords",
    "camera",
    "printer",
    "games",
    "music",
    "people",
    "home",
    "security",
    "installer",
    "keyboard",
];
/// An icon in a preview, and the cell it sits in (its name's bar under it).
const PREVIEW_ICON: f32 = 32.0;
const CELL: (f32, f32) = (56.0, 50.0);

/// A theme's preview in `r`, a small desktop drawn in the theme: its
/// wallpaper (`picture`, once it's made; till then the desk's colour), a
/// window in its colours (`look`) holding its icons (icon set `icons`),
/// and the desk's own icons at the right, as the desktop has them;
/// framed as a picture well. On a grid of its own: nothing in it is laid
/// out with the panel.
fn paint_theme_preview(g: &mut Painter, r: Bounds<Pixels>, look: &Palette, icons: &'static str, picture: Option<Arc<RenderImage>>) {
    let p = olo_kit::theme::palette();
    let dark = look.mode == ThemeMode::Dark;
    let frame = style::snap(g, r);
    let well = style::inset(g, frame, 2.0);
    olo_kit::review::own_grid(g, frame);
    style::fill_rect(g, well, c(olo_kit::palette::tint(look.panel_background, if dark { 0.8 } else { 1.1 })));
    if let Some(img) = picture {
        // A texture pixel to a device pixel, centred, clipped to the well.
        let s = g.scale_factor();
        let isz = img.size(0);
        let (w, h) = (isz.width.0 as f32 / s, isz.height.0 as f32 / s);
        let at = style::snap_centred(g, well.center(), w, h);
        g.with_content_mask(Some(olo_kit::ContentMask { bounds: well }), |g| {
            let _ = g.paint_image(at, Corners::default(), img, 0, false);
        });
    }
    let (w, h) = (f32::from(well.size.width), f32::from(well.size.height));
    // The desk's icons down the right, the bin at the bottom, each with
    // its name's bar; the window in what's left, a margin round it.
    let desk = 2.0 * PREVIEW_ICON + 8.0;
    let margin = (h * 0.09).round();
    let win = Bounds { origin: point(well.left() + px(margin), well.top() + px(margin)), size: size(px(w - desk - 2.0 * margin), px(h - 2.0 * margin)) };
    g.with_content_mask(Some(olo_kit::ContentMask { bounds: well }), |g| {
        preview_window(g, win, look, icons);
        let x = well.right() - px((desk / 2.0 + PREVIEW_ICON / 2.0).round());
        let spots = [("disk", margin), ("home", margin + CELL.1 + 6.0), ("trash-full", h - margin - CELL.1 + 6.0)];
        for (name, y) in spots {
            let at = Bounds { origin: point(x, well.top() + px(y)), size: size(px(PREVIEW_ICON), px(PREVIEW_ICON)) };
            icons::paint_in(g, at, icons, name);
            // Its name, as the desktop writes one: light, a shadow under it.
            let bar = |dy: f32| Bounds { origin: point(at.center().x - px(13.0), at.bottom() + px(5.0 + dy)), size: size(px(26.0), px(3.0)) };
            let (shadow, text) = (style::snap(g, bar(1.0)), style::snap(g, bar(0.0)));
            style::fill_rect(g, shadow, c(look.desktop_text_shadow));
            style::fill_rect(g, text, c(look.desktop_text));
        }
    });
    let inner = style::inset(g, frame, 1.0);
    style::bevel(g, inner, style::shade(&p, p.panel_background, 1.45), c(p.shine));
    style::frame_rect(g, frame, style::shade(&p, p.panel_background, 1.2));
}

/// A window in palette `p`, small but whole, showing icon set `set`'s
/// icons as Tracker shows a folder: its focused tab with a title, the
/// frame, a menu bar, the icons in rows with a bar for each one's name
/// (one chosen), and two buttons, the default one in the highlight
/// colour.
fn preview_window(g: &mut Painter, r: Bounds<Pixels>, p: &Palette, set: &'static str) {
    let fill = |g: &mut Painter, b: Bounds<Pixels>, rgba| {
        let b = style::snap(g, b);
        style::fill_rect(g, b, c(rgba));
    };
    let at = |x: Pixels, y: Pixels, w: f32, h: f32| Bounds { origin: point(x, y), size: size(px(w), px(h)) };
    let shrink = |b: Bounds<Pixels>, d: f32| b.dilate(px(-d));
    // A bar of "text", `w` long, centred on `y`.
    let text = |g: &mut Painter, x: Pixels, y: Pixels, w: f32, rgba| fill(g, at(x, px((f32::from(y) - 1.5).round()), w.round(), 3.0), rgba);
    let tab_h = 14.0;
    // The tab: two fifths of the width, the look's own colour.
    let tab = at(r.left(), r.top(), (f32::from(r.size.width) * 0.4).round(), tab_h + 1.0);
    fill(g, tab, p.frame_focused[0]);
    let face = Bounds::from_corners(point(tab.left() + px(1.0), tab.top() + px(1.0)), point(tab.right() - px(1.0), tab.bottom()));
    fill(g, face, p.tab_focused);
    fill(g, at(face.left(), face.top(), f32::from(face.size.width), 1.0), p.tab_focused_light);
    // Its close box, then its title.
    let close = at(face.left() + px(3.0), face.top() + px(3.0), 8.0, 8.0);
    fill(g, close, p.tab_focused_shadow);
    fill(g, shrink(close, 1.0), p.tab_focused_light);
    text(g, close.right() + px(4.0), face.center().y, f32::from(face.size.width) * 0.45, p.tab_text_focused);
    // The frame and its panel.
    let win = Bounds::from_corners(point(r.left(), tab.bottom()), r.bottom_right());
    fill(g, win, p.frame_focused[0]);
    fill(g, shrink(win, 1.0), p.frame_focused[1]);
    let panel = Bounds::from_corners(point(win.left() + px(2.0), win.top() + px(2.0)), point(win.right() - px(1.0), win.bottom() - px(1.0)));
    fill(g, panel, p.panel_background);
    // The menu bar, a few titles in it.
    let bar = at(panel.left(), panel.top(), f32::from(panel.size.width), 12.0);
    fill(g, bar, p.menu_background);
    fill(g, at(bar.left(), bar.bottom() - px(1.0), f32::from(bar.size.width), 1.0), p.frame_focused[4]);
    let mut x = bar.left() + px(5.0);
    for w in [12.0, 10.0, 14.0, 11.0] {
        text(g, x, bar.center().y - px(0.5), w, p.menu_item_text);
        x += px(w + 7.0);
    }
    // Its icons, as many rows and columns as fit, centred in the list;
    // the second chosen.
    let list = Bounds::from_corners(point(panel.left() + px(5.0), bar.bottom() + px(5.0)), point(panel.right() - px(5.0), panel.bottom() - px(24.0)));
    fill(g, list, p.control_border);
    let doc = shrink(list, 1.0);
    fill(g, doc, p.list_background);
    let (dw, dh) = (f32::from(doc.size.width), f32::from(doc.size.height));
    let cols = (((dw - 8.0) / CELL.0).floor().max(1.0) as usize).min(WINDOW_ICONS.len());
    // (Whole rows only: no row part filled.)
    let rows = (((dh - 6.0) / CELL.1).floor().max(1.0) as usize).min(WINDOW_ICONS.len() / cols);
    let step_x = ((dw - 8.0) / cols as f32).floor();
    let top = ((dh - rows as f32 * CELL.1) / 2.0).floor();
    for (i, name) in WINDOW_ICONS.iter().take(cols * rows).enumerate() {
        let (col, row) = (i % cols, i / cols);
        let cell = at(doc.left() + px(4.0 + col as f32 * step_x), doc.top() + px(top + row as f32 * CELL.1), step_x, CELL.1);
        let icon = at(cell.center().x - px(PREVIEW_ICON / 2.0), cell.top() + px(4.0), PREVIEW_ICON, PREVIEW_ICON);
        icons::paint_in(g, style::snap(g, icon), set, name);
        let name_w = [30.0, 22.0, 34.0, 26.0, 38.0][i % 5];
        let label = at(cell.center().x - px(name_w / 2.0), icon.bottom() + px(3.0), name_w, 7.0);
        let ink = if i == 1 {
            fill(g, label.dilate(px(2.0)), p.list_selected_background);
            p.list_selected_item_text
        } else {
            p.list_item_text
        };
        text(g, label.left(), label.center().y, name_w, ink);
    }
    // The buttons, at the bottom right.
    let bw = 34.0;
    let bh = 13.0;
    let by = panel.bottom() - px(bh + 5.0);
    let default = at(panel.right() - px(bw + 5.0), by, bw, bh);
    let other = at(default.left() - px(bw + 5.0), by, bw, bh);
    fill(g, other, p.control_border);
    fill(g, shrink(other, 1.0), p.control_background);
    text(g, other.center().x - px(bw * 0.25), other.center().y, bw * 0.5, p.control_text);
    fill(g, default, p.control_border);
    fill(g, shrink(default, 1.0), p.menu_selected_background);
    text(g, default.center().x - px(bw * 0.25), default.center().y, bw * 0.5, p.menu_selected_item_text);
}

/// Switch to theme `id`, and to its wallpaper (as the person has it).
fn choose(s: &mut Settings, id: &str) {
    s.appearance.look = id.to_owned();
    s.wallpaper = Some(s.appearance.wallpaper_of(id));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_theme_brings_its_wallpaper() {
        // A new install starts in the default theme on its wallpaper.
        let mut s = Settings::default();
        assert_eq!(s.appearance.look, looks::DEFAULT);
        assert_eq!(s.wallpaper, Some(olo_xdg::themes::wallpaper(looks::DEFAULT)));
        choose(&mut s, "platinum");
        assert_eq!((s.appearance.look.as_str(), s.wallpaper.as_deref()), ("platinum", Some("pack:quantum-foam")));
        // One the person gave a theme comes with it instead.
        s.appearance.custom.entry("beos".into()).or_default().wallpaper = Some("pack:tub".into());
        choose(&mut s, "beos");
        assert_eq!((s.appearance.look.as_str(), s.wallpaper.as_deref()), ("beos", Some("pack:tub")));
    }

    #[test]
    fn a_colour_changed_and_put_back() {
        let mut s = Settings::default();
        let own = looks::palette("classic", ThemeMode::Light);
        set_colour(&mut s, "classic", ThemeMode::Light, "tab", Some([255, 0, 128]));
        assert_eq!(s.appearance.custom["classic"].light["tab"], "#ff0080");
        assert_eq!(s.appearance.palette().tab_focused, [255, 0, 128, 255]);
        assert_eq!(s.appearance.palette_of("classic", ThemeMode::Dark).tab_focused, looks::palette("classic", ThemeMode::Dark).tab_focused);
        // The theme's own colour set again is nothing changed, and
        // nothing kept.
        let [r, g, b, _] = own.tab_focused;
        set_colour(&mut s, "classic", ThemeMode::Light, "tab", Some([r, g, b]));
        assert!(s.appearance.custom.is_empty());
        set_colour(&mut s, "classic", ThemeMode::Dark, "panel", Some([1, 2, 3]));
        set_colour(&mut s, "classic", ThemeMode::Dark, "panel", None);
        assert!(s.appearance.custom.is_empty());
        assert_eq!(s.appearance.palette(), own);
    }
}

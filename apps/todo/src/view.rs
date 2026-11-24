//! The window, one split view under its menu bar (as Tracker's and a
//! mail program's are, not boxes set on a panel): down the left, flush,
//! Today, Upcoming, All and Done, then the person's lists, each a dot of
//! its colour with how many it has to do, and New List at the foot;
//! beside them the place chosen, its name and what it has to do over
//! its to-dos, a list (list.rs) or, for a list, a board of its sections
//! (board.rs); and behind one line at the right, the to-do chosen
//! (details.rs). The list is the window's one bright ground.

use olo_kit::{
    Bounds, ClickEvent, Cx, Element, FontWeight, InteractiveElement as _, IntoElement as _, KeyDownEvent, MenuBar, MouseButton, MouseDownEvent, ParentElement as _, Pixels, SharedString, Styled as _,
    View as _, WindowOptions, agent, canvas, control_canvas, div, metrics as m, point, px, size, style, theme,
    widgets::{
        ArrowButton, Button, MenuField, Panes, StringView, TextControl, TextField, TextFieldEvent, button_width,
        sidebar::{self, Row as SideRow, Sidebar},
        text_field,
    },
};
use olo_theme::Hue;
use olo_todo::view::Place;

use crate::{Layout, Msg, ToDoApp, board, details, list, words};

/// The places' and lists' column.
pub const SIDEBAR: f32 = 184.0;
/// The chosen to-do's column: one width in every view (it jumped, and
/// what's in it moved, when it took what a board left it).
pub const DETAILS: f32 = 292.0;
/// What the to-dos keep beside a column: a title and its due on a row;
/// a board, two of its lanes. Less, the column gives way (CLAUDE.md,
/// "Every tiling layout").
const MAIN_BESIDE: f32 = 340.0;
const BOARD_BESIDE: f32 = 2.0 * board::LANE_LEAST + board::GAP + 2.0 * BOARD_INSET;
/// The window's least: a third of the smallest screen, either way.
pub const MIN_WIDTH: f32 = 400.0;
pub const MIN_HEIGHT: f32 = 220.0;
/// The place's header over its to-dos, and where its name's baseline is
/// in it; the chosen to-do's title starts so its first line sits on the
/// same line.
pub const HEADER: f32 = 46.0;
const TITLE_BASE: f32 = 31.0;
pub const TITLE_TOP: f32 = 15.0;
/// A board's inset in its pane: the list's own, so a list's lanes end
/// where its rows' rules and dues do, and the header's switch with them
/// (four points apart, what's in the pane jumped from List to Board).
const BOARD_INSET: f32 = list::CHECK_X;
/// Over a board, under the header: its line, and a little air.
const BOARD_TOP: f32 = 5.0;

/// A line between the window's parts, on the panel's ground.
fn line(p: &olo_theme::Palette) -> olo_kit::Hsla {
    style::c(olo_theme::mix(p.panel_background, p.panel_text, 0.22))
}

/// The line over a pane's foot, drawn over the whole pane (lay it over
/// the pane, last): a foot's height over the pane's bottom, so on the
/// same device pixel in every pane side by side, and never apart from
/// what it's the foot of. (Each foot's own top, as laid out, was a pixel
/// under the column over it at 1.75×, the panel showing between.)
/// `edge`: the pane ends in a line down its right (the sidebar's), which
/// the line stops at.
pub fn foot_line(edge: bool) -> Element {
    canvas(move |g, b| {
        let p = theme::palette();
        let y = b.bottom() - px(details::FOOT + 1.0);
        if edge {
            let w = style::hairline(g);
            style::hline(g, b.left(), b.right() - w, y, line(&p));
            style::vline_left_of(g, b.right(), y, y + px(1.0), sidebar::edge(&p));
        } else {
            style::hline(g, b.left(), b.right(), y, line(&p));
        }
    })
    .paint_key(style::key((4, edge)))
    .absolute()
    .inset_0()
    .into_element()
}

/// A place's name as the window says it.
pub fn place_name(v: &ToDoApp, place: &Place) -> SharedString {
    match place {
        Place::Today => olo_kit::words::tr("Today").into(),
        Place::Upcoming => olo_kit::words::tr("Upcoming").into(),
        Place::All => olo_kit::words::tr("All").into(),
        Place::Done => olo_kit::words::tr("Done").into(),
        Place::List(id) => v.book.list(id).map_or_else(|| SharedString::from(""), |l| SharedString::from(l.name.clone())),
    }
}

/// A place's icon, of the four that aren't lists.
fn place_icon(place: &Place) -> &'static str {
    match place {
        Place::Today => "calendar",
        Place::Upcoming => "datetime",
        Place::All => "todo",
        Place::Done => "todo-done",
        Place::List(_) => "",
    }
}

pub fn body(v: &mut ToDoApp, cx: &mut Cx<ToDoApp>) -> Element {
    let p = theme::palette();
    let vs = cx.viewport_size();
    let (w, h) = (f32::from(vs.width), f32::from(vs.height));
    // What's beside the to-dos gives way to them, never squeezing them
    // (CLAUDE.md, "Every tiling layout"): the chosen one's column where
    // there's room for it beside them and the places', else the places'
    // alone, else neither. A column without the room gives way; View ▸
    // Show brings it in front of them, alone, with a way back.
    v.details_pane.set_collapsed(!v.settings.details);
    v.sidebar_pane.set_collapsed(!v.settings.sidebar);
    let beside = if v.layout() == Layout::Board { BOARD_BESIDE } else { MAIN_BESIDE };
    let with_sidebar = if v.settings.sidebar { SIDEBAR } else { 0.0 };
    let details = v.details_pane.panes(w - with_sidebar, beside);
    let left = w - details.width();
    let sidebar = if details == Panes::Pane { Panes::Rest } else { v.sidebar_pane.panes(left, beside) };
    let main_w = left - sidebar.width();
    // (What a board has under the menu bar and the place's header.)
    let board_h = h - olo_kit::menu::BAR_HEIGHT - HEADER - BOARD_TOP - BOARD_INSET;
    let mut row = div().flex_1().min_h_0().flex().flex_row();
    if details == Panes::Pane {
        row = row.child(div().flex_1().min_w_0().h_full().child(details::pane(v, true, w, h - olo_kit::menu::BAR_HEIGHT, cx)));
    } else if sidebar == Panes::Pane {
        row = row.child(div().flex_1().min_w_0().h_full().child(sidebar_column(v, true, cx)));
    } else {
        if let Panes::Both(sw) = sidebar {
            row = row.child(div().w(px(sw)).flex_shrink_0().h_full().child(sidebar_column(v, false, cx)));
        }
        row = row.child(div().flex_1().min_w_0().h_full().child(main_column(v, main_w, board_h, cx)));
        if let Panes::Both(dw) = details {
            // (Behind one line at its left.)
            let edge = canvas(|g, b| {
                let p = theme::palette();
                style::vline(g, b.left(), b.top(), b.bottom(), line(&p));
            })
            .paint_key(5)
            .absolute()
            .inset_0();
            row = row.child(div().relative().w(px(dw)).flex_shrink_0().h_full().child(edge).child(div().absolute().inset_0().left(px(1.0)).child(details::pane(
                v,
                false,
                dw - 1.0,
                h - olo_kit::menu::BAR_HEIGHT,
                cx,
            ))));
        }
    }
    // (The menus after what shows is settled: View ▸ Show Sidebar and
    // Show Details are checked as they show now.)
    let bar = MenuBar::of(v, cx, |v, _| v.menus());
    div().size_full().flex().flex_col().bg(style::c(p.panel_background)).child(bar).child(row).into_element()
}

/// The back arrow of a column shown alone in front of the to-dos (a
/// window too narrow for both).
pub fn back(cx: &mut Cx<ToDoApp>, which: Which) -> Element {
    ArrowButton::back(match which {
        Which::Sidebar => "Back to the To-Dos",
        Which::Details => "Back to the List",
    })
    .on_click(cx.listener(move |v: &mut ToDoApp, _: &ClickEvent, cx| {
        match which {
            Which::Sidebar => v.sidebar_pane.show_rest(),
            Which::Details => v.details_pane.show_rest(),
        }
        cx.invalidate();
    }))
    .into_element()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    Sidebar,
    Details,
}

/// How many a place has to do, for its row.
pub fn count(v: &ToDoApp, place: &Place) -> usize {
    v.counts.at(place)
}

/// The four places, then the lists in their colours; New List at the
/// column's foot.
fn sidebar_column(v: &mut ToDoApp, alone: bool, cx: &mut Cx<ToDoApp>) -> Element {
    const PLACES: [(Place, &str); 4] = [(Place::Today, "Today"), (Place::Upcoming, "Upcoming"), (Place::All, "All"), (Place::Done, "Done")];
    let mut rows: Vec<SideRow> = Vec::new();
    let mut places: Vec<Option<Place>> = Vec::new();
    for (place, name) in PLACES {
        let n = count(v, &place);
        let mut row = SideRow::new(place_icon(&place), name);
        if n > 0 {
            row = row.value(n.to_string());
        }
        rows.push(row);
        places.push(Some(place));
    }
    rows.push(SideRow::heading("Lists"));
    places.push(None);
    let first_list = rows.len();
    for l in v.lists() {
        // (No count where there's nothing to do, as the places above.)
        let n = count(v, &Place::List(l.id.clone()));
        let row = SideRow::new("", l.name.clone()).dot(l.hue);
        rows.push(if n > 0 { row.value(n.to_string()) } else { row });
        places.push(Some(Place::List(l.id.clone())));
    }
    let chosen = places.iter().position(|pl| pl.as_ref() == Some(&v.place)).unwrap_or(0);
    let chooser = places.clone();
    let menu_places = places.clone();
    let list = Sidebar::new("todo-places", rows, chosen)
        .on_choose(cx.listener(move |v: &mut ToDoApp, i: &usize, cx| {
            if let Some(Some(place)) = chooser.get(*i) {
                v.choose(place.clone(), cx);
            }
        }))
        // (The lists dragged into another order: the person's own.)
        .on_reorder(cx.listener(move |_: &mut ToDoApp, (from, to): &(usize, usize), cx| {
            if *from >= first_list && *to >= first_list {
                cx.send(Msg::Reorder(from - first_list, to - first_list));
            }
        }))
        .on_menu(cx.listener(move |v: &mut ToDoApp, (i, at): &(usize, olo_kit::Point<Pixels>), cx| {
            let Some(Some(Place::List(id))) = menu_places.get(*i) else { return };
            let Some(list) = v.book.list(id).cloned() else { return };
            let mut colours = olo_kit::Menu::new("Colour");
            for hue in Hue::ALL {
                colours = colours.item(hue_name(hue), format!("colour:{}", hue_name(hue))).checked(list.hue == hue);
            }
            let menu = olo_kit::Menu::new("List").item("Rename…", "rename").item("New Section…", "section").submenu(colours).separator().item("Delete List", "delete");
            let view = cx.view_ref();
            let id = id.clone();
            olo_kit::menu::context_menu(cx, menu, *at, move |cmd, cx| {
                let (cmd, id) = (cmd.to_owned(), id.clone());
                view.update(cx, move |v: &mut ToDoApp, cx| match cmd.as_str() {
                    "rename" => Naming::ask(v, Asking::RenameList(id), cx),
                    "section" => Naming::ask(v, Asking::NewSection(id), cx),
                    "delete" => v.trash_list(&id, cx),
                    c => {
                        if let Some(hue) = c.strip_prefix("colour:").and_then(|n| Hue::ALL.into_iter().find(|h| hue_name(*h) == n)) {
                            cx.send(Msg::Recolour(id, hue));
                        }
                    }
                });
            });
        }))
        .build(cx);
    // Its foot, on the column's own ground: a row to make a list with,
    // a + where a list's dot is.
    let new_list = control_canvas(|g, b, state| {
        let p = theme::palette();
        // (Its line over it is the column's: `foot_line`.)
        let b = style::snap(g, b);
        style::fill_rect(g, b, sidebar::ground(&p));
        style::vline_left_of(g, b.right(), b.top(), b.bottom(), sidebar::edge(&p));
        let row = Bounds { origin: point(b.left(), b.center().y - px(sidebar::ROW / 2.0)), size: size(b.size.width - px(1.0), px(sidebar::ROW)) };
        let ground = if state.hovered { style::shade(&p, p.panel_background, 1.1) } else { sidebar::ground(&p) };
        if state.hovered {
            style::fill_rect(g, row, ground);
        }
        // (Its words as the rows over it: a thing to press, not one
        // turned off; its + quiet, in a ring as the new to-do's is: one
        // mark for making something new.)
        let plus = Bounds { origin: point(b.left() + px(m::WINDOW_INSET), b.center().y - px(8.0)), size: size(px(16.0), px(16.0)) };
        list::paint_new_circle(g, plus, list::CHECK, style::secondary_text(&p), ground);
        let said = olo_kit::words::tr("New List");
        style::label(g, plus.right() + px(6.0), b.center().y, style::Align::Left, said, style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::c(p.panel_text));
        g.agent(agent::Node::button(olo_kit::words::tr("New List…"), row));
    })
    .paint_key(6)
    .w_full()
    .h(px(details::FOOT + 1.0))
    .flex_shrink_0()
    .on_mouse_down(MouseButton::Left, cx.listener(|v: &mut ToDoApp, _: &MouseDownEvent, cx| Naming::ask(v, Asking::NewList, cx)));
    div()
        .relative()
        .size_full()
        .flex()
        .flex_col()
        .children(alone.then(|| {
            div().flex().flex_row().items_center().gap(px(m::TIGHT)).h(px(HEADER)).px(px(m::WINDOW_INSET)).flex_shrink_0().child(back(cx, Which::Sidebar)).child(StringView::new("Lists").bold(true))
        }))
        .child(div().flex_1().min_h_0().child(list))
        .child(new_list)
        .child(foot_line(true))
        .into_element()
}

/// A colour's name, as the person picks it.
pub fn hue_name(hue: Hue) -> &'static str {
    match hue {
        Hue::Red => "Red",
        Hue::Orange => "Orange",
        Hue::Yellow => "Yellow",
        Hue::Green => "Green",
        Hue::Teal => "Teal",
        Hue::Blue => "Blue",
        Hue::Purple => "Purple",
        Hue::Pink => "Pink",
        Hue::Brown => "Brown",
        Hue::Gray => "Gray",
    }
}

/// The place's header: its mark (a list's dot, a place's icon) where
/// the rows' check circles are, its name where their titles start, and
/// quieter on the same line what it has to do (Today: the day too); a
/// list's layout at the right.
fn header(v: &mut ToDoApp, width: f32, cx: &mut Cx<ToDoApp>) -> Element {
    let name = place_name(v, &v.place);
    let n = count(v, &v.place);
    let to_do = (n > 0).then(|| olo_kit::words::count(n as u64, "1 to do", "{n} to do"));
    let said = match (&v.place, to_do) {
        (Place::Today, Some(c)) => olo_kit::tr!("{} · {}", words::long_day(v.now.date()), c),
        (Place::Today, None) => words::long_day(v.now.date()),
        (Place::Done, _) => match v.counts.done {
            0 => String::new(),
            n => olo_kit::words::count(n as u64, "1 to-do", "{n} to-dos"),
        },
        (_, Some(c)) => c,
        (_, None) => olo_kit::words::tr("Nothing to do").into(),
    };
    let (icon, hue) = (place_icon(&v.place), if let Place::List(id) = &v.place { Some(v.hue_of(id)) } else { None });
    let mut row = div().w_full().h(px(HEADER)).flex_shrink_0().flex().flex_row().items_center();
    // (In front of the lists, or with them put away: a way to them.)
    let back_w = if v.sidebar_pane.alone() || !v.settings.sidebar {
        row = row.child(
            div().pl(px(m::WINDOW_INSET)).child(
                ArrowButton::back("Lists")
                    .on_click(cx.listener(|v: &mut ToDoApp, _: &ClickEvent, cx| {
                        if v.sidebar_pane.alone() {
                            v.sidebar_pane.show_pane();
                        } else {
                            v.settings.sidebar = true;
                            v.save();
                        }
                        cx.invalidate();
                    }))
                    .into_element(),
            ),
        );
        38.0
    } else {
        0.0
    };
    let key = style::key((&name, &said, icon, hue));
    let (title, detail) = (name.clone(), said.clone());
    let words_el = canvas(move |g, b| {
        let p = theme::palette();
        let base = b.top() + px(TITLE_BASE);
        // (Its mark's middle on the middle of its name's capitals, on
        // whole pixels: seven points over the baseline was a pixel off
        // them at 1.5×.)
        let base = style::snap_coord(g, base);
        let middle = base - px((style::LEAD_SIZE + 4.0) * style::CAP_HEIGHT / 2.0);
        let mark = style::snap_centred(g, point(b.left() + px(list::CHECK_X + 9.0), middle), 16.0, 16.0);
        match hue {
            Some(hue) => sidebar::paint_dot(g, mark, p.hue(hue), &p),
            None => {
                olo_kit::icons::paint(g, mark, icon);
            }
        }
        let left = b.left() + px(list::TEXT_X);
        let bold = style::ui_font(FontWeight::BOLD);
        let room = b.right() - px(8.0) - left;
        let at = style::label_on_baseline_fit(g, left, base, style::Align::Left, title.clone(), style::LEAD_SIZE + 4.0, bold, style::c(p.panel_text), room);
        let rest = b.right() - px(8.0) - at.right() - px(10.0);
        if !detail.is_empty() && rest > px(60.0) {
            style::label_on_baseline_fit(g, at.right() + px(10.0), base, style::Align::Left, detail.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::secondary_text(&p), rest);
        }
        g.agent(agent::Node { role: "Heading", name: Some(title.clone()), value: Some(detail.clone().into()), bounds: at, enabled: true, ..Default::default() });
    })
    .paint_key(key)
    .flex_1()
    .min_w_0()
    .h_full();
    row = row.child(words_el);
    if let Place::List(_) = &v.place
        && width - back_w >= 300.0
    {
        let wide = button_width("List", false).max(button_width("Board", false));
        row = row.child(div().flex_shrink_0().pr(px(BOARD_INSET)).child(layout_switch(v.layout(), wide, cx)));
    }
    row.into_element()
}

/// The switch between the two ways a list is shown: one control of two
/// halves, a line between them; the half in force sunken on the
/// selection's ground (which way is on must be plain in every look: a
/// pressed grey beside a raised grey wasn't, by night least of all), the
/// other a raised face to press.
fn layout_switch(layout: Layout, half: f32, cx: &mut Cx<ToDoApp>) -> Element {
    control_canvas(move |g, b, state| {
        let p = theme::palette();
        let b = style::snap(g, b);
        let line = style::hairline(g);
        let mid = style::snap_coord(g, b.center().x);
        style::frame_rect(g, b, style::shade(&p, p.panel_background, 1.55));
        for (i, (label, on)) in [("List", layout == Layout::List), ("Board", layout == Layout::Board)].into_iter().enumerate() {
            let said = olo_kit::words::tr(label);
            let part = if i == 0 {
                Bounds::from_corners(b.origin + point(line, line), point(mid, b.bottom() - line))
            } else {
                Bounds::from_corners(point(mid + line, b.top() + line), b.bottom_right() - point(line, line))
            };
            let ink = if on {
                style::fill_rect(g, part, style::c(p.list_selected_background));
                // (Pressed in: dark above and at its left, light below.)
                style::bevel(g, part, style::shade(&p, p.list_selected_background, 1.35), style::c(p.shine));
                style::c(p.list_selected_item_text)
            } else {
                let base = p.menu_background;
                style::vgradient(g, part, style::tint(base, if state.hovered { 0.9 } else { 0.82 }), style::shade(&p, base, 1.05));
                style::bevel(g, part, style::c(p.shine), style::shade(&p, base, 1.2));
                style::c(p.panel_text)
            };
            style::label(g, part.center().x, part.center().y, style::Align::Center, said, style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), ink);
            g.agent(agent::Node { role: "RadioButton", name: Some(said.into()), bounds: part, enabled: true, clickable: true, checked: Some(on), ..Default::default() });
        }
        style::vline(g, mid, b.top(), b.bottom(), style::shade(&p, p.panel_background, 1.55));
    })
    .paint_key(style::key(layout == Layout::List))
    .w(px(2.0 * half))
    .h(px(m::CONTROL_HEIGHT))
    .flex_shrink_0()
    .on_mouse_down(
        MouseButton::Left,
        cx.listener(move |v: &mut ToDoApp, e: &MouseDownEvent, cx| {
            // (Which half was pressed: by where in the control.)
            let b = cx.listener_bounds();
            v.command(if e.position.x < b.center().x { "layout:list" } else { "layout:board" }, cx);
        }),
    )
}

/// The place: its header, and its to-dos as a list (the window's bright
/// ground) or a board (on the panel).
fn main_column(v: &mut ToDoApp, width: f32, board_h: f32, cx: &mut Cx<ToDoApp>) -> Element {
    let p = theme::palette();
    let board = matches!((v.layout(), &v.place), (Layout::Board, Place::List(_)));
    let content = if board {
        // (A board's new to-do row is the list's own, on a sheet of its
        // own over the lanes.)
        // (Under the header's line, as the list is: without it the header
        // ran into the lanes, and was another header from view to view.)
        let top = canvas(|g, b| {
            let p = theme::palette();
            style::hline(g, b.left(), b.right(), b.top(), line(&p));
        })
        .paint_key(8)
        .absolute()
        .inset_0();
        div()
            .relative()
            .size_full()
            .child(top)
            .child(div().absolute().inset_0().top(px(BOARD_TOP)).px(px(BOARD_INSET)).pb(px(BOARD_INSET)).child(board::view(v, width - 2.0 * BOARD_INSET, board_h, cx)))
            .into_element()
    } else {
        let top = canvas(|g, b| {
            let p = theme::palette();
            style::fill_rect(g, b, style::c(p.document_background));
            style::hline(g, b.left(), b.right(), b.top(), line(&p));
        })
        .paint_key(7)
        .absolute()
        .inset_0();
        div().relative().size_full().child(top).child(div().absolute().inset_0().top(px(1.0)).child(list::view(v, cx))).into_element()
    };
    let mut col = div().size_full().flex().flex_col().child(header(v, width, cx));
    col = col.child(div().flex_1().min_h_0().child(content));
    if let Some(problem) = &v.problem {
        col = col.child(div().px(px(list::CHECK_X)).py(px(4.0)).child(StringView::new(olo_kit::tr!("Couldn't save the change: {}", problem)).small(true).color(style::c(p.failure)).cut(true)));
    }
    // Keys, where nothing that's typed in has the keyboard: ↑ and ↓ step
    // through the to-dos (on a board down a lane, ← and → across the
    // lanes), Space checks the chosen one off, Return opens it,
    // Delete deletes it, Escape lets go of it.
    let keys = cx.listener(|v: &mut ToDoApp, e: &KeyDownEvent, cx| {
        if e.keystroke.modifiers.modified() {
            return;
        }
        match e.keystroke.key.as_str() {
            // (On a board, down its lane, and ← → across the lanes.)
            "up" if v.layout() == Layout::Board => v.step_board(-1, false, cx),
            "down" if v.layout() == Layout::Board => v.step_board(1, false, cx),
            "left" if v.layout() == Layout::Board => v.step_board(-1, true, cx),
            "right" if v.layout() == Layout::Board => v.step_board(1, true, cx),
            "up" => v.step(-1, cx),
            "down" => v.step(1, cx),
            "space" if v.selected.is_some() => {
                let uid = v.selected.clone().unwrap_or_default();
                v.check(&uid, cx);
            }
            "enter" if v.selected.is_some() => {
                v.details_pane.show_pane();
                v.title.focus(cx);
                cx.invalidate();
            }
            "delete" | "backspace" if v.selected.is_some() => v.delete_selected(cx),
            // (From a board's new to-do, typed or not: its card goes.)
            "escape" if v.entry_section.is_some() && v.layout() == Layout::Board => {
                v.entry_section = None;
                v.entry.update(cx, |f, cx| f.set_text(String::new(), cx));
                cx.invalidate();
            }
            "escape" => v.select(None, cx),
            _ => return,
        }
        cx.stop_propagation();
    });
    div().id("to-dos").size_full().track_focus(&v.focus).on_key_down(keys).child(col).into_element()
}

/// What a name is asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum Asking {
    NewList,
    RenameList(String),
    NewSection(String),
    RenameSection(String, String),
    /// A link for the to-do with this UID.
    Link(String),
}

/// A small window asking for a name (a list's, with its colour; a
/// section's) or a link's address, as Weather's Rename Place does.
pub struct Naming {
    asking: Asking,
    name: olo_kit::Child<TextField>,
    hue: usize,
    to_main: olo_kit::Messenger<Msg>,
}

impl Naming {
    pub fn ask(v: &mut ToDoApp, asking: Asking, cx: &mut Cx<ToDoApp>) {
        if olo_kit::raise(v.dialog.as_ref()) {
            return;
        }
        // (A new list's colour: the next no list has yet, and never the
        // accent's blue first, which reads as a link or a control.)
        let free = [Hue::Green, Hue::Orange, Hue::Purple, Hue::Teal, Hue::Red, Hue::Pink, Hue::Yellow, Hue::Brown, Hue::Blue, Hue::Gray];
        let next = free.into_iter().find(|h| !v.book.lists.iter().any(|l| l.hue == *h)).unwrap_or(Hue::Green);
        let (title, text, hue) = match &asking {
            Asking::NewList => ("New List", String::new(), next),
            Asking::RenameList(id) => ("Rename List", v.book.list(id).map(|l| l.name.clone()).unwrap_or_default(), Hue::Blue),
            Asking::NewSection(_) => ("New Section", String::new(), Hue::Blue),
            Asking::RenameSection(_, old) => ("Rename Section", old.clone(), Hue::Blue),
            Asking::Link(_) => ("Attach Link", String::new(), Hue::Blue),
        };
        let to_main = cx.messenger();
        let hue = Hue::ALL.iter().position(|h| *h == hue).unwrap_or(0);
        v.dialog = Some(olo_kit::open_window(WindowOptions::new(title, 380.0, 120.0).dialog().fits_content(), move |cx| {
            let name = text_field(cx, |f| f.name("Name").with_text(text));
            name.update(cx, |f, cx| f.select_all_text(cx));
            name.focus(cx);
            cx.subscribe(&name, |v: &mut Naming, e: &TextFieldEvent, cx| {
                if matches!(e, TextFieldEvent::Entered) {
                    v.done(true, cx);
                }
            });
            Naming { asking, name, hue, to_main }
        }));
    }

    fn done(&mut self, ok: bool, cx: &mut Cx<Self>) {
        if ok {
            let typed = self.name.read(cx).text().trim().to_owned();
            if typed.is_empty() {
                return;
            }
            let hue = Hue::ALL[self.hue.min(Hue::ALL.len() - 1)];
            self.to_main.send(match self.asking.clone() {
                Asking::NewList => Msg::NewList(typed, hue),
                Asking::RenameList(id) => Msg::Rename(id, typed),
                Asking::NewSection(id) => Msg::AddSection(id, typed),
                Asking::RenameSection(id, old) => Msg::RenameSection(id, old, typed),
                Asking::Link(uid) => {
                    // (Not an address: the field keeps it, to be mended.)
                    if details::link(&typed).is_none() {
                        return;
                    }
                    Msg::AddLink(uid, typed)
                }
            });
        }
        cx.close_window();
    }
}

impl olo_kit::View for Naming {
    type Message = usize;

    fn message(&mut self, hue: usize, cx: &mut Cx<Self>) {
        self.hue = hue;
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        const DIVIDER: f32 = 68.0;
        let verb = match self.asking {
            Asking::NewList | Asking::NewSection(_) => "Make",
            Asking::Link(_) => "Attach",
            _ => "Rename",
        };
        let label = if matches!(self.asking, Asking::Link(_)) { "Address:" } else { "Name:" };
        let bw = button_width(verb, false).max(button_width("Cancel", false));
        let mut col = div()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::STACK))
            .on_key_down(cx.listener(|v: &mut Naming, e: &KeyDownEvent, cx| {
                if e.keystroke.key == "escape" {
                    v.done(false, cx);
                }
            }))
            .child(TextControl::new(cx, &self.name).label(label).divider(DIVIDER));
        if self.asking == Asking::NewList {
            let names: Vec<SharedString> = Hue::ALL.iter().map(|h| SharedString::from(hue_name(*h))).collect();
            col = col.child(MenuField::new("list-colour", names, self.hue).label("Colour:").divider(DIVIDER).width(DIVIDER + 140.0).on_change(cx.msg_with(|i: &usize| *i)));
        }
        col.child(
            div()
                .mt(px(m::SPACING))
                .flex()
                .justify_end()
                .gap(px(m::SPACING))
                .child(Button::new("Cancel").min_width(bw).on_click(cx.listener(|v: &mut Naming, _: &ClickEvent, cx| v.done(false, cx))))
                .child(Button::new(verb).default(true).min_width(bw).on_click(cx.listener(|v: &mut Naming, _: &ClickEvent, cx| v.done(true, cx)))),
        )
        .into_element()
    }
}

//! The kit's tab strip (`widgets::TabStrip`): along the top and down the
//! side, with two groups in the look's group colours
//! (`Palette::group_colors`), tabs in none, the chosen one open onto the
//! content. Click a tab to choose it, × or the middle button to close it
//! (it comes back), + for another. And a tab view (`widgets::TabView`)
//! in the same look, with more tabs than fit at their ease.

use std::rc::Rc;

use olo_kit::{
    Cx, Element, ParentElement as _, Styled as _, View, WindowOptions, div, menu, metrics as m, px, style, theme,
    widgets::{STRIP_HEIGHT, STRIP_SIDE_WIDTH, StringView, StripAction, StripGroup, StripTab, TabStrip, TabView},
};

struct Tabs {
    titles: Vec<(&'static str, Option<usize>)>,
    active: usize,
    added: usize,
    page: usize,
}

#[derive(Clone)]
enum Msg {
    Act(StripAction),
    Page(usize),
}

impl Tabs {
    fn strip(&self, id: &str, cx: &mut Cx<Self>) -> TabStrip {
        let p = theme::palette();
        let icon: olo_kit::widgets::strip::PaintFn = Rc::new(|g, b| {
            let p = theme::palette();
            style::fill_rect(g, b, style::c(p.document_background));
            style::frame_rect(g, b, style::c(p.panel_text));
        });
        // (The second with something unread: its red dot.)
        let tabs = self
            .titles
            .iter()
            .enumerate()
            .map(|(i, (t, group))| StripTab { label: (*t).into(), icon: Some(icon.clone()), group: *group, unread: (i == 1).then_some(None), ..Default::default() })
            .collect();
        let groups = vec![StripGroup { name: "olo".into(), color: style::c(p.group_color(0).fill) }, StripGroup { name: "shiftup".into(), color: style::c(p.group_color(5).fill) }];
        let (fill, ink) = (style::c(p.terminal.background), style::c(p.terminal.foreground));
        TabStrip::new(id.to_owned(), tabs, self.active).groups(groups).content(fill, ink).on_action(cx.msg_with(|a: &StripAction| Msg::Act(*a)))
    }
}

impl View for Tabs {
    type Message = Msg;

    fn message(&mut self, msg: Msg, cx: &mut Cx<Self>) {
        let a = match msg {
            Msg::Act(a) => a,
            Msg::Page(i) => {
                self.page = i;
                return cx.invalidate();
            }
        };
        match a {
            StripAction::Select(i) => self.active = i,
            StripAction::Close(i) if self.titles.len() > 1 => {
                self.titles.remove(i);
                self.active = self.active.min(self.titles.len() - 1);
            }
            StripAction::New => {
                self.added += 1;
                self.titles.push((if self.added % 2 == 1 { "ben@fedora:~" } else { "htop" }, None));
                self.active = self.titles.len() - 1;
            }
            _ => {}
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let ground = style::c(p.terminal.background);
        let top = self.strip("tabs-top", cx);
        let side = self.strip("tabs-side", cx).side();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(style::c(p.panel_background))
            .child(div().h(px(menu::BAR_HEIGHT)).flex_shrink_0())
            .child(div().h(px(STRIP_HEIGHT)).flex_shrink_0().child(top))
            .child(div().h(px(120.0)).flex_shrink_0().bg(ground))
            .child(div().flex().flex_1().child(div().w(px(STRIP_SIDE_WIDTH)).h_full().flex_shrink_0().child(side)).child(div().flex_1().h_full().bg(ground).p(px(m::WINDOW_INSET)).child(
                div().w(px(560.0)).h(px(150.0)).p(px(m::WINDOW_INSET)).bg(style::c(p.panel_background)).child(
                    TabView::new("pages", PAGES.iter().map(|&t| t.into()).collect::<Vec<_>>(), self.page).on_select(cx.msg_with(|i: &usize| Msg::Page(*i))).child(StringView::new(PAGES[self.page])),
                ),
            )))
    }
}

/// A tab view's pages: Preferences ▸ Network's, more than fit at ease.
const PAGES: [&str; 8] = ["Wi-Fi", "Ethernet", "VPN", "Service Order", "DNS", "Hosts", "Shaping", "Details"];

fn main() {
    olo_kit::run("olo.tabs-example", |_| {
        olo_kit::open_window(WindowOptions::new("Tabs", 1030.0, 560.0), |_| Tabs {
            titles: vec![("Tab style options (HTML)", Some(0)), ("cargo build --release", Some(0)), ("ShiftUp V2: sign-in", Some(1)), ("notes — ~/src/notes", None)],
            active: 0,
            added: 0,
            page: 3,
        });
    });
}

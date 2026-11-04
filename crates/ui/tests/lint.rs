//! The kit's own widgets through the layout lint (`cargo ui-lint`): a
//! form laid out as the kit lays one out is clean in every look at every
//! scale (a box with an action in its top line too), and controls put
//! over each other are found; a tab strip under a menu bar, along the top
//! and down the side, with groups and a faded tab, is clean too.

use olo_ui::{
    WindowOptions,
    gpui::{AppContext as _, Context, IntoElement, ParentElement as _, Render, Styled as _, Window, div, px},
    lint::{Lint, assert_clean},
    metrics as m,
    widgets::{BBox, Button, CheckBox, RadioButton, TextControl, text_field},
};

/// A form: a labelled field, choices in a box with Reset, and buttons
/// along the bottom.
struct Form {
    field: olo_ui::gpui::Entity<olo_ui::widgets::TextField>,
    /// The buttons put over each other (to be found).
    stacked: bool,
}

impl Render for Form {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let buttons = if self.stacked {
            div().relative().h(px(m::CONTROL_HEIGHT)).child(div().absolute().left_0().child(Button::new("Revert"))).child(div().absolute().left(px(20.0)).child(Button::new("Defaults")))
        } else {
            div().flex().flex_row().gap(px(m::SPACING)).child(Button::new("Revert")).child(Button::new("Defaults"))
        };
        div()
            .size_full()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(TextControl::new(self.field.clone()).label("Name:").divider(60.0))
            .child(CheckBox::new("Show the date in the menu bar", true))
            .child(BBox::new("Set the Time").action("Reset", |_, _| {}).child(RadioButton::new("Automatically", true)).child(RadioButton::new("By hand", false)))
            .child(buttons)
    }
}

fn form(stacked: bool) -> Vec<olo_ui::lint::Found> {
    let mut lint = Lint::new();
    let options = WindowOptions::new("Form", 360.0, 226.0);
    lint.window(&options, move |_, cx| {
        let field = text_field(cx, |f| f);
        cx.new(|_| Form { field, stacked })
    })
}

#[test]
fn ui_lint_a_kit_form_is_clean() {
    assert_clean("the form", &form(false));
}

#[test]
fn ui_lint_finds_controls_over_each_other() {
    let found = form(true);
    assert!(found.iter().any(|f| f.kind == "overlap" && f.text.contains("Revert") && f.text.contains("Defaults")), "{found:#?}");
}

/// GPUI's own text elements, some cut: one-line text running past its
/// box, text truncated with "…", a paragraph clamped to two lines; and
/// some not: a line that fits, a paragraph that wraps in full.
struct Texts;

const PAST: &str = "A long line that runs past the side of its box";
const TRUNCATED: &str = "Truncated to the width of its box with an ellipsis";
const CLAMPED: &str = "A paragraph wrapped to its box and clamped to two lines, so the rest of it never shows at all";
const FITS: &str = "Fits";
const WRAPS: &str = "A paragraph that wraps and shows in full";

impl Render for Texts {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(div().w(px(90.0)).overflow_hidden().whitespace_nowrap().child(PAST))
            .child(div().w(px(90.0)).truncate().child(TRUNCATED))
            .child(div().w(px(90.0)).line_clamp(2).child(CLAMPED))
            .child(div().w(px(200.0)).child(FITS))
            .child(div().w(px(200.0)).child(WRAPS))
    }
}

/// What GPUI's text elements cut is read whole (and shown whole in a tip,
/// `olo_ui::cut`): agents find each as a label by its whole text; what
/// shows in full isn't one.
#[test]
fn ui_lint_cut_gpui_text_is_read_whole() {
    let mut lint = Lint::new();
    let trees = lint.trees(&WindowOptions::new("Texts", 360.0, 300.0), |_, cx| cx.new(|_| Texts));
    let labels: Vec<String> = trees[0].1.iter().filter(|n| n.role == "Label").filter_map(|n| n.name.clone()).collect();
    for cut in [PAST, TRUNCATED, CLAMPED] {
        assert!(labels.iter().any(|l| l == cut), "{cut:?} isn't read whole: {labels:#?}");
    }
    for whole in [FITS, WRAPS] {
        assert!(!labels.iter().any(|l| l == whole), "{whole:?} shows in full but is read as cut: {labels:#?}");
    }
}

/// A view with one timer for what it has due.
#[derive(Default)]
struct Waits {
    due: olo_ui::due::Due,
    woke: u32,
}

/// `Due` wakes its view once, at the time set; set to the same time
/// again it keeps its timer, set to none it's gone, and `by` keeps the
/// soonest.
#[test]
fn ui_lint_due_wakes_once_at_its_time() {
    use std::time::{Duration, Instant};
    let cx = olo_ui::gpui::TestAppContext::single();
    let view = cx.update(|cx| cx.new(|_| Waits::default()));
    let at = Instant::now() + Duration::from_secs(5);
    let set = |at: Option<Instant>| {
        view.update(&mut cx.clone(), |w, cx| {
            let mut due = std::mem::take(&mut w.due);
            due.at(at, cx, |w: &mut Waits, _| w.woke += 1);
            w.due = due;
        })
    };
    set(Some(at));
    set(Some(at));
    cx.executor().advance_clock(Duration::from_secs(4));
    assert_eq!(view.read_with(&cx, |w, _| w.woke), 0);
    cx.executor().advance_clock(Duration::from_secs(2));
    assert_eq!(view.read_with(&cx, |w, _| w.woke), 1);
    // None: no timer.
    set(Some(Instant::now() + Duration::from_secs(5)));
    set(None);
    cx.executor().advance_clock(Duration::from_secs(10));
    assert_eq!(view.read_with(&cx, |w, _| (w.woke, w.due.when())), (1, None));
    // `by`: the soonest asked.
    view.update(&mut cx.clone(), |w, cx| {
        w.due.by(Instant::now() + Duration::from_secs(9), cx);
        w.due.by(Instant::now() + Duration::from_secs(3), cx);
        w.due.by(Instant::now() + Duration::from_secs(6), cx);
        let left = w.due.when().unwrap().saturating_duration_since(Instant::now());
        assert!(left <= Duration::from_secs(3) && left > Duration::from_secs(2), "{left:?}");
    });
}

/// A window's menu bar over a tab strip (Terminal's), along the top or
/// down the side: two groups, a tab in none, one tab faded, New Tab.
struct Strip {
    side: bool,
}

impl Render for Strip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use olo_ui::{
            gpui::{App, Bounds, Hsla, Pixels},
            menu::{BAR_HEIGHT, Menu, MenuBar},
            widgets::{STRIP_HEIGHT, STRIP_SIDE_WIDTH, StripGroup, StripTab, TabStrip},
        };
        let icon: olo_ui::widgets::strip::PaintFn = std::rc::Rc::new(|window: &mut Window, _: &mut App, r: Bounds<Pixels>| {
            olo_ui::icons::paint(window, r, "terminal");
        });
        let tab = |label: &str, group: Option<usize>, faded: bool| StripTab {
            label: label.to_owned().into(),
            icon: Some(icon.clone()),
            group,
            faded: faded.then(|| "hidden when tiled".into()),
            ..Default::default()
        };
        let tabs = vec![tab("ben@aurora:~/src/olo", Some(0), false), tab("ben@aurora:~/notes", Some(0), true), tab("ben@aurora:~/web", Some(1), false), tab("ben@aurora:~", None, false)];
        let color = |h: f32| Hsla { h, s: 0.7, l: 0.55, a: 1.0 };
        let groups = vec![StripGroup { name: "olo".into(), color: color(0.0) }, StripGroup { name: "notes".into(), color: color(0.08) }];
        let mut strip = TabStrip::new("lint-strip", tabs, 2).groups(groups).group_selection(vec![true, false]);
        if self.side {
            strip = strip.side();
        }
        let menus = ["Terminal", "Edit", "Tabs", "Settings"].map(|t| Menu::new(t).item("Close", "close")).to_vec();
        let _ = cx;
        let place = if self.side {
            // (Down to an inset over the bottom: the lint's window isn't
            // Terminal's, whose strip runs to its foot.)
            div().absolute().left_0().top(px(BAR_HEIGHT)).bottom(px(m::WINDOW_INSET)).w(px(STRIP_SIDE_WIDTH))
        } else {
            div().absolute().left_0().right_0().top(px(BAR_HEIGHT)).h(px(STRIP_HEIGHT))
        };
        div().size_full().relative().child(place.child(strip)).child(div().absolute().top_0().left_0().right_0().child(MenuBar::new(menus, |_, _, _| {})))
    }
}

/// Terminal's tab strip, along the top and down the side, as agents and
/// the review read it: clean at every scale in every look (the side
/// strip's close boxes once crowded its chips and New Tab, its first
/// chip the menu bar's titles, and its icons and chips' names sat off
/// the menu bar's first title).
#[test]
fn ui_lint_a_tab_strip_is_clean() {
    for side in [false, true] {
        let mut lint = Lint::new();
        // (Along the top, as tall as the bar, the strip and an inset.)
        let height = if side { 260.0 } else { olo_ui::menu::BAR_HEIGHT + olo_ui::widgets::STRIP_HEIGHT + m::WINDOW_INSET };
        let found = lint.window(&WindowOptions::new("Strip", 900.0, height), move |_, cx| cx.new(|_| Strip { side }));
        assert_clean(if side { "the side strip" } else { "the strip along the top" }, &found);
    }
}

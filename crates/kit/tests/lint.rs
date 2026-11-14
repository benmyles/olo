//! The kit's own widgets through the layout lint (`cargo ui-lint`): a
//! form laid out as the kit lays one out is clean in every look at every
//! scale (a box with an action in its top line too), and controls put
//! over each other are found; text the interface cuts is read whole; a
//! view's one timer (`Due`) wakes it once, at its time; Terminal's tab
//! strip, along the top and down the side, is clean.

use olo_kit::{
    Child, Cx, Element, ParentElement as _, Styled as _, View, WindowOptions, div,
    lint::{Found, Lint, assert_clean},
    metrics as m, px,
    widgets::{BBox, Button, CheckBox, RadioButton, TextControl, TextField, text_field},
};

/// A form: a labelled field, choices in a box with Reset, and buttons
/// along the bottom.
struct Form {
    field: Child<TextField>,
    /// The buttons put over each other (to be found).
    stacked: bool,
}

impl View for Form {
    type Message = ();

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let buttons = if self.stacked {
            div().relative().h(px(m::CONTROL_HEIGHT)).child(div().absolute().left(px(0.0)).child(Button::new("Revert"))).child(div().absolute().left(px(20.0)).child(Button::new("Defaults")))
        } else {
            div().flex().flex_row().gap(px(m::SPACING)).child(Button::new("Revert")).child(Button::new("Defaults"))
        };
        div()
            .size_full()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(TextControl::new(cx, &self.field).label("Name:").divider(60.0))
            .child(CheckBox::new("Show the date in the menu bar", true))
            .child(BBox::new("Set the Time").action("Reset", |_, _| {}).child(RadioButton::new("Automatically", true)).child(RadioButton::new("By hand", false)))
            .child(buttons)
    }
}

fn form(stacked: bool) -> Vec<Found> {
    let mut lint = Lint::new();
    let options = WindowOptions::new("Form", 360.0, 226.0);
    lint.window(&options, move |cx| Form { field: text_field(cx, |f| f), stacked })
}

#[test]
fn ui_lint_a_kit_form_is_clean() {
    assert_clean("the form", &form(false));
}

#[test]
fn ui_lint_finds_controls_over_each_other() {
    let found = form(true);
    assert!(found.iter().any(|f| f.kind == "overlap" && f.text.contains("Revert") && f.text.contains("Defaults")), "{found:#?}");
    // In every look at every scale: the window with no display took each.
    let mut where_found: Vec<(&str, u32)> = found.iter().filter(|f| f.kind == "overlap").map(|f| (f.look, (f.scale * 100.0) as u32)).collect();
    where_found.sort();
    where_found.dedup();
    assert_eq!(where_found, [("dark", 100), ("dark", 150), ("dark", 200), ("light", 100), ("light", 150), ("light", 200)]);
}

/// The interface's own text, some cut: one-line text running past its
/// box, text truncated with "…", a paragraph clamped to two lines; and
/// some not: a line that fits, a paragraph that wraps in full.
struct Texts;

const PAST: &str = "A long line that runs past the side of its box";
const TRUNCATED: &str = "Truncated to the width of its box with an ellipsis";
const CLAMPED: &str = "A paragraph wrapped to its box and clamped to two lines, so the rest of it never shows at all";
const FITS: &str = "Fits";
const WRAPS: &str = "A paragraph that wraps and shows in full";

impl View for Texts {
    type Message = ();

    fn body(&mut self, _: &mut Cx<Self>) -> Element {
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

/// Text is read whole whether or not all of it shows (agents find each
/// as a label by its whole text; what's cut is shown whole in a tip,
/// `olo_kit::cut`): the interface's own text is in the window's tree,
/// where the GPUI kit's lint read only what GPUI's text elements cut.
#[test]
fn ui_lint_text_is_read_whole() {
    let mut lint = Lint::new();
    let trees = lint.trees(&WindowOptions::new("Texts", 360.0, 300.0), |_| Texts);
    let labels: Vec<String> = trees[0].1.iter().filter(|n| n.role == "Label").filter_map(|n| n.name.clone()).collect();
    for text in [PAST, TRUNCATED, CLAMPED, FITS, WRAPS] {
        assert!(labels.iter().any(|l| l == text), "{text:?} isn't read whole: {labels:#?}");
    }
}

/// A view with one timer for what it has due.
#[derive(Default)]
struct Waits {
    due: olo_kit::due::Due,
    woke: u32,
}

impl View for Waits {
    type Message = ();

    fn body(&mut self, _: &mut Cx<Self>) -> Element {
        div().size_full()
    }
}

/// What `f` says of the window's view, on its looper.
fn ask<V: View, R: Send + 'static>(window: &olo_kit::WindowHandle<V>, f: impl FnOnce(&mut V, &mut Cx<V>) -> R + Send + 'static) -> R {
    let (tx, rx) = std::sync::mpsc::channel();
    window.update(move |v, cx| {
        let _ = tx.send(f(v, cx));
    });
    rx.recv_timeout(std::time::Duration::from_secs(10)).expect("the window's looper answers")
}

/// `Due` wakes its view once, at the time set; set to the same time
/// again it keeps its timer, set to none it's gone, and `by` keeps the
/// soonest. (In real time, on the window's looper: each answer comes
/// with the time it was given at, so a slow machine fails nothing.)
#[test]
fn ui_lint_due_wakes_once_at_its_time() {
    use std::time::{Duration, Instant};
    let _lint = Lint::new();
    let window = olo_kit::open_window(WindowOptions::new("Waits", 120.0, 60.0), |_| Waits::default());
    let set = |at: Option<Instant>| ask(&window, move |w, cx| w.due.at(at, cx, |w: &mut Waits, _| w.woke += 1));
    let woke = || ask(&window, |w, _| (w.woke, Instant::now()));
    let at = Instant::now() + Duration::from_millis(400);
    set(Some(at));
    set(Some(at));
    // Not before its time.
    let (n, when) = woke();
    assert!(when >= at || n == 0, "woke {n} times before its time");
    // Once, at it: and, set twice to the one time, only once.
    let woken = Instant::now();
    while woke().0 == 0 && woken.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let (n, when) = woke();
    assert!(n == 1 && when >= at, "woke {n} times");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(woke().0, 1);
    // None: no timer.
    set(Some(Instant::now() + Duration::from_millis(200)));
    set(None);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(ask(&window, |w, _| (w.woke, w.due.when())), (1, None));
    // `by`: the soonest asked.
    let left = ask(&window, |w, cx| {
        w.due.by(Instant::now() + Duration::from_secs(9), cx);
        w.due.by(Instant::now() + Duration::from_secs(3), cx);
        w.due.by(Instant::now() + Duration::from_secs(6), cx);
        w.due.when().unwrap().saturating_duration_since(Instant::now())
    });
    assert!(left <= Duration::from_secs(3) && left > Duration::from_secs(2), "{left:?}");
    window.close();
}

/// A window's menu bar over a tab strip (Terminal's), along the top or
/// down the side: two groups, a tab in none, one tab faded, New Tab.
struct Strip {
    side: bool,
    /// The tab showing: the one last clicked (the lint clicks each, as
    /// it does a tab view's, and waits for it to show).
    active: usize,
}

impl View for Strip {
    type Message = usize;

    fn message(&mut self, tab: usize, cx: &mut Cx<Self>) {
        self.active = tab;
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        use olo_kit::{
            Hsla,
            menu::{BAR_HEIGHT, Menu, MenuBar},
            widgets::{STRIP_HEIGHT, STRIP_SIDE_WIDTH, StripAction, StripGroup, StripTab, TabStrip},
        };
        let icon: olo_kit::widgets::strip::PaintFn = std::rc::Rc::new(|g, r| {
            olo_kit::icons::paint(g, r, "terminal");
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
        let chosen = cx.messenger();
        let mut strip = TabStrip::new("lint-strip", tabs, self.active).groups(groups).group_selection(vec![true, false]).on_action(move |action, _| {
            if let StripAction::Select(tab) = action {
                chosen.send(*tab);
            }
        });
        if self.side {
            strip = strip.side();
        }
        let menus = vec![
            Menu::new("Terminal").app_items(&["terminal"]).close_quit(),
            Menu::new("Edit").item("Copy", "copy"),
            Menu::new("Tabs").item("Close Tab", "close-tab"),
            Menu::new("Settings").item("Font Size", "font-size"),
        ];
        let place = if self.side {
            // (Down to an inset over the bottom: the lint's window isn't
            // Terminal's, whose strip runs to its foot.)
            div().absolute().left_0().top(px(BAR_HEIGHT)).bottom(px(m::WINDOW_INSET)).w(px(STRIP_SIDE_WIDTH))
        } else {
            div().absolute().left_0().right_0().top(px(BAR_HEIGHT)).h(px(STRIP_HEIGHT))
        };
        div().size_full().relative().child(place.child(strip)).child(div().absolute().top_0().left_0().right_0().child(MenuBar::new(cx, menus)))
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
        let height = if side { 260.0 } else { olo_kit::menu::BAR_HEIGHT + olo_kit::widgets::STRIP_HEIGHT + m::WINDOW_INSET };
        let found = lint.window(&WindowOptions::new("Strip", 900.0, height), move |_| Strip { side, active: 2 });
        assert_clean(if side { "the side strip" } else { "the strip along the top" }, &found);
    }
}

/// What a window's looper is sent of the pointer's moves: a box that
/// lights under it (`on_hover`), one that follows it (`on_mouse_move`),
/// one that does only with ⌘ held, one only while a button is, a
/// drawing that notes an edge of its own (and hears moves only across
/// it, `moves_matter_at_edges`), and the window counting every move it's
/// sent (`on_drag_move`).
struct Moves {
    heard: std::rc::Rc<std::cell::Cell<u32>>,
    over: std::rc::Rc<std::cell::Cell<bool>>,
    /// The moves the drawing heard.
    drawn: std::rc::Rc<std::cell::Cell<u32>>,
    /// Where the lit box's top is (moved under a still pointer).
    top: f32,
}

impl View for Moves {
    type Message = ();

    fn body(&mut self, _: &mut Cx<Self>) -> Element {
        use olo_kit::InteractiveElement as _;
        let (heard, over, drawn) = (self.heard.clone(), self.over.clone(), self.drawn.clone());
        let boxed = |left: f32, top: f32| div().absolute().left(px(left)).top(px(top)).w(px(40.0)).h(px(20.0));
        div()
            .size_full()
            .relative()
            .on_drag_move(move |_, _| heard.set(heard.get() + 1))
            .child(boxed(20.0, self.top).on_hover(move |o, _| over.set(*o)))
            .child(boxed(100.0, 20.0).on_mouse_move(|_, _| {}))
            .child(boxed(20.0, 50.0).moves_matter_with_command().on_mouse_move(|_, _| {}))
            .child(boxed(80.0, 50.0).moves_matter_while_pressed().on_mouse_move(|_, _| {}))
            .child(
                olo_kit::canvas(|g, _| g.note_pointer_edge(olo_kit::Bounds { origin: olo_kit::point(px(150.0), px(60.0)), size: olo_kit::size(px(30.0), px(20.0)) }))
                    .absolute()
                    .left(px(140.0))
                    .top(px(50.0))
                    .w(px(60.0))
                    .h(px(50.0))
                    .moves_matter_at_edges()
                    .on_mouse_move(move |_, _| drawn.set(drawn.get() + 1)),
            )
    }
}

/// A window's looper is sent the pointer's moves only where they mean
/// something: across the edges of what lights under it (and of what a
/// drawing notes), every move over what follows it, every move while a
/// button is held, and the first after anything else; elsewhere the
/// display thread keeps them, and keeps where the pointer is for when
/// the looper looks again (a box moved under a still pointer lights).
#[test]
fn ui_lint_moves_reach_the_window_only_where_they_matter() {
    use olo_kit::{Modifiers, MouseButton, point};
    let _lint = Lint::new();
    let window = olo_kit::open_window(WindowOptions::new("Moves", 200.0, 100.0), |_| Moves { heard: Default::default(), over: Default::default(), drawn: Default::default(), top: 20.0 });
    // (Laid out and committed: the stage has where moves matter.)
    ask(&window, |_, _| ());
    ask(&window, |_, _| ());
    let to_with = |x: f32, y: f32, button: Option<MouseButton>, modifiers: Modifiers| {
        let decided = ask(&window, move |_, cx| cx.headless_move(point(px(x), px(y)), button, modifiers));
        decided.recv_timeout(std::time::Duration::from_secs(10)).expect("the display thread decides");
        ask(&window, |v, _| (v.heard.get(), v.over.get()))
    };
    let to = |x: f32, y: f32, button: Option<MouseButton>| to_with(x, y, button, Modifiers::default());
    // The first move: sent. Then over nothing: kept.
    assert_eq!(to(10.0, 80.0, None), (1, false));
    assert_eq!(to(12.0, 80.0, None), (1, false));
    assert_eq!(to(14.0, 82.0, None), (1, false));
    // Over the box that lights: sent, and it's lit; within it, kept.
    assert_eq!(to(40.0, 30.0, None), (2, true));
    assert_eq!(to(42.0, 31.0, None), (2, true));
    // Near its edge: sent (the looper's own edge may be a pixel off).
    assert_eq!(to(59.5, 31.0, None), (3, true));
    // Out of it: sent, and it isn't lit.
    assert_eq!(to(10.0, 80.0, None), (4, false));
    // Over what follows the pointer: every move.
    assert_eq!(to(120.0, 30.0, None), (5, false));
    assert_eq!(to(122.0, 31.0, None), (6, false));
    // Across the edge a drawing noted: sent; within it, kept.
    assert_eq!(to(165.0, 70.0, None), (7, false));
    assert_eq!(to(166.0, 71.0, None), (7, false));
    assert_eq!(to(10.0, 80.0, None), (8, false));
    assert_eq!(to(12.0, 82.0, None), (8, false));
    // A button held: every move, anywhere.
    assert_eq!(to(14.0, 84.0, Some(MouseButton::Left)), (9, false));
    assert_eq!(to(16.0, 86.0, Some(MouseButton::Left)), (10, false));
    // Let go (the first move after): sent; then kept again.
    assert_eq!(to(18.0, 86.0, None), (11, false));
    assert_eq!(to(16.0, 86.0, None), (11, false));
    // Over what follows the pointer with ⌘ held: its edge, then only
    // with ⌘.
    assert_eq!(to(40.0, 60.0, None), (12, false));
    assert_eq!(to(41.0, 61.0, None), (12, false));
    let command = Modifiers { control: true, ..Default::default() };
    assert_eq!(to_with(42.0, 61.0, None, command), (13, false));
    assert_eq!(to_with(43.0, 62.0, None, command), (14, false));
    // Over what follows it only while a button is held: nothing sent
    // without (not even its edges: only a drag's moves matter there).
    assert_eq!(to(16.0, 86.0, None), (15, false));
    assert_eq!(to(100.0, 60.0, None), (15, false));
    assert_eq!(to(101.0, 61.0, None), (15, false));
    assert_eq!(to(102.0, 61.0, Some(MouseButton::Left)), (16, false));
    assert_eq!(to(16.0, 86.0, None), (17, false));
    assert_eq!(to(30.0, 80.0, None), (17, false));
    // The looper is told where the pointer is, kept moves and all.
    assert_eq!(ask(&window, |_, cx| cx.mouse_position()), point(px(30.0), px(80.0)));
    // The box moved under the still pointer (which the looper last saw at
    // 18, 86, beside it): lit, where the pointer is now.
    ask(&window, |v, cx| {
        v.top = 70.0;
        cx.invalidate();
    });
    assert_eq!(ask(&window, |v, _| v.over.get()), true);
    // What hears moves only across the edges its drawing notes: its own
    // edge and the noted one, not the moves between (it heard the one
    // into the noted edge above).
    let drawn = || ask(&window, |v, _| v.drawn.get());
    assert_eq!(drawn(), 1);
    assert_eq!(to(145.0, 55.0, None), (18, false));
    assert_eq!(drawn(), 2);
    assert_eq!(to(147.0, 57.0, None), (18, false));
    assert_eq!(to(165.0, 70.0, None), (19, false));
    assert_eq!(drawn(), 3);
    window.close();
}

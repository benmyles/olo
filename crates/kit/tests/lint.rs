//! The kit's own widgets through the layout lint (`cargo ui-lint`): a
//! form laid out as the kit lays one out is clean in every look at every
//! scale (a box with an action in its top line too), and controls put
//! over each other are found; text the interface cuts is read whole; a
//! view's one timer (`Due`) wakes it once, at its time; Terminal's tab
//! strip, along the top and down the side, is clean; rows with keys are
//! kept as they are when the view says what it shows again; a window
//! that fits what it shows, asked to be taller than the screen has room
//! for, keeps what fills it; a list scrolled, hidden and shown again
//! scrolls from where it was; a list acted on by what's checked
//! (`chosen`) is clean, its button as wide whatever it counts; text
//! measured without a window counts what another font draws; a month to
//! pick a day from under a pair of arrow buttons and a title with a cut
//! status on its line is clean.

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
    // (Its buttons' words as drawn: the lint runs in any language.)
    let said = olo_kit::words::tr;
    assert!(found.iter().any(|f| f.kind == "overlap" && f.text.contains(said("Revert")) && f.text.contains(said("Defaults"))), "{found:#?}");
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

/// What `f` says of the window's view, on its looper, answered once the
/// window has drawn what `f` changed (`WindowCx::after_flush`). Answered
/// at once, the next question could reach the looper before it drew:
/// it draws when its queue is dry, and a question sent straight back
/// kept it from being so, read before the drawing ("only the changed
/// row is drawn again" found none drawn, now and then).
fn ask<V: View, R: Send + 'static>(window: &olo_kit::WindowHandle<V>, f: impl FnOnce(&mut V, &mut Cx<V>) -> R + Send + 'static) -> R {
    let (tx, rx) = std::sync::mpsc::channel();
    window.update(move |v, cx| {
        let answer = f(v, cx);
        cx.after_flush(move |_| {
            let _ = tx.send(answer);
        });
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
            // (One with something unread: its red dot after the title.)
            unread: (label == "ben@aurora:~/web").then_some(None),
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
        div().size_full().relative().child(place.child(strip)).child(div().absolute().top_0().left_0().right_0().child(MenuBar::fixed(cx, menus)))
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

/// A tab view whose names don't fit its window: each name cut short is
/// found (as French's Network tabs were); in a window wide enough, none.
struct Pages {
    /// The tab showing: the one last clicked (the lint clicks each, and
    /// waits for it to show).
    chosen: usize,
}

impl View for Pages {
    type Message = usize;

    fn message(&mut self, tab: usize, cx: &mut Cx<Self>) {
        self.chosen = tab;
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let names: Vec<olo_kit::SharedString> = ["Wi-Fi", "Ordre des services", "Mise en forme du trafic", "Détails"].into_iter().map(Into::into).collect();
        let chosen = cx.messenger();
        let tabs = olo_kit::widgets::TabView::new("pages", names, self.chosen)
            .on_select(move |i, _| {
                chosen.send(*i);
            })
            .child(div().h(px(40.0)));
        div().size_full().p(px(m::WINDOW_INSET)).child(tabs)
    }
}

#[test]
fn ui_lint_a_tab_view_cut_short_is_found() {
    let mut lint = Lint::new();
    let found = lint.window(&WindowOptions::new("Pages", 300.0, 140.0), |_| Pages { chosen: 0 });
    let cut: Vec<&Found> = found.iter().filter(|f| f.kind == "text-cut").collect();
    assert!(cut.iter().any(|f| f.text.contains("Mise en forme du trafic")), "{found:#?}");
    // As wide as `tabs_width` says (and the window's inset): whole.
    let wide = olo_kit::widgets::tabs_width(&["Wi-Fi", "Ordre des services", "Mise en forme du trafic", "Détails"]) + 2.0 * m::WINDOW_INSET;
    let found = lint.window(&WindowOptions::new("Pages", wide.ceil(), 140.0), |_| Pages { chosen: 0 });
    assert!(found.iter().all(|f| f.kind != "text-cut"), "{wide}: {found:#?}");
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

/// A hundred rows of a scroller, each a drawing keyed by what it shows;
/// the rows too with `keyed` (`row_key_by`). Which rows are made and
/// which drawn, as they are.
struct Rows {
    keys: Vec<u64>,
    keyed: bool,
    made: std::rc::Rc<std::cell::RefCell<Vec<usize>>>,
    drawn: std::rc::Rc<std::cell::RefCell<Vec<usize>>>,
}

impl View for Rows {
    type Message = ();

    fn body(&mut self, _: &mut Cx<Self>) -> Element {
        use olo_kit::InteractiveElement as _;
        let made = self.made.clone();
        let mut rows = olo_kit::uniform_rows(self.keys.len(), px(20.0), move |near| {
            near.map(|row| {
                made.borrow_mut().push(row);
                olo_kit::view_canvas::<Rows>(move |v, _, _| v.drawn.borrow_mut().push(row)).key_by::<Rows>(move |v| v.keys[row]).id(row).w_full()
            })
            .collect()
        });
        if self.keyed {
            rows = rows.row_key_by::<Rows>(|v, row| v.keys[row]);
        }
        div().size_full().overflow_y_scroll().child(rows)
    }
}

/// Rows keyed by what they are: when the view says what it shows again,
/// those whose key is the same are kept, not made again (nor drawn), and
/// a row whose key changed is made and drawn alone. Without keys every
/// row near what shows is made again (and only the changed one drawn).
#[test]
fn ui_lint_keyed_rows_are_kept() {
    let _lint = Lint::new();
    for keyed in [true, false] {
        let window = olo_kit::open_window(WindowOptions::new("Rows", 200.0, 100.0), move |_| Rows { keys: (0..100).collect(), keyed, made: Default::default(), drawn: Default::default() });
        ask(&window, |_, _| ());
        ask(&window, |_, _| ());
        let taken = |v: &mut Rows| (std::mem::take(&mut *v.made.borrow_mut()), std::mem::take(&mut *v.drawn.borrow_mut()));
        let (made, drawn) = ask(&window, move |v, _| taken(v));
        assert!(made.len() >= 5 && drawn.len() >= 5, "the rows that show, at first: made {made:?}, drawn {drawn:?}");
        let near = made.len();
        // Row 3 changes.
        ask(&window, |v, cx| {
            v.keys[3] = 1000;
            cx.invalidate();
        });
        let (made, drawn) = ask(&window, move |v, _| taken(v));
        assert_eq!(drawn, [3], "only the changed row is drawn again (keyed: {keyed})");
        if keyed {
            assert_eq!(made, [3], "only the changed row is made again");
        } else {
            assert_eq!(made.len(), near, "every row near what shows is made again");
        }
        // Nothing changes: with keys nothing is made.
        ask(&window, |_, cx| cx.invalidate());
        let (made, drawn) = ask(&window, move |v, _| taken(v));
        assert!(drawn.is_empty(), "{drawn:?}");
        assert_eq!(made.len(), if keyed { 0 } else { near });
        window.close();
    }
}

/// Rows of their own heights (`sized_rows`): 20 points, and 40 for every
/// fifth (a heading with air over it). Which are made, and where each
/// was drawn and how tall.
#[derive(Default)]
struct Sized {
    scroll: olo_kit::ScrollHandle,
    made: std::rc::Rc<std::cell::RefCell<Vec<usize>>>,
    drawn: std::rc::Rc<std::cell::RefCell<Vec<(usize, f32, f32)>>>,
}

impl View for Sized {
    type Message = ();

    fn body(&mut self, _: &mut Cx<Self>) -> Element {
        use olo_kit::InteractiveElement as _;
        let made = self.made.clone();
        let tops = olo_kit::row_tops((0..1000).map(|row| if row % 5 == 0 { 40.0 } else { 20.0 }));
        let rows = olo_kit::sized_rows(tops, move |near| {
            near.map(|row| {
                made.borrow_mut().push(row);
                olo_kit::view_canvas::<Sized>(move |v, _, b| v.drawn.borrow_mut().push((row, f32::from(b.top()), f32::from(b.size.height)))).id(row).w_full()
            })
            .collect()
        });
        div().size_full().overflow_y_scroll().track_scroll(&self.scroll).child(rows)
    }
}

/// Rows of their own heights are made only near what shows, each where
/// the heights before it put it and as tall as its own, at the list's
/// top and scrolled far down it.
#[test]
fn ui_lint_sized_rows_are_made_where_they_show() {
    use olo_kit::point;
    let _lint = Lint::new();
    let window = olo_kit::open_window(WindowOptions::new("Sized", 200.0, 100.0), |_| Sized::default());
    let settle = || {
        ask(&window, |_, _| ());
        ask(&window, |_, _| ());
    };
    let taken = |v: &mut Sized| (std::mem::take(&mut *v.made.borrow_mut()), std::mem::take(&mut *v.drawn.borrow_mut()));
    let check = |first: usize, made: &[usize], drawn: &[(usize, f32, f32)]| {
        assert!(made.contains(&first) && made.len() < 100, "only rows near what shows are made: {made:?}");
        let at = |row: usize| drawn.iter().rev().find(|d| d.0 == row).copied().unwrap_or_else(|| panic!("row {row} drawn: {drawn:?}"));
        for row in first..first + 5 {
            let ((_, top, height), (_, next, _)) = (at(row), at(row + 1));
            assert_eq!(height, if row % 5 == 0 { 40.0 } else { 20.0 }, "row {row}'s height");
            assert_eq!(next - top, height, "row {} starts where row {row} ends", row + 1);
        }
    };
    settle();
    let (made, drawn) = ask(&window, move |v, _| taken(v));
    check(0, &made, &drawn);
    // Scrolled to row 500 (a hundred fives of 120 points each down).
    ask(&window, |v, cx| v.scroll.set_offset(cx, point(px(0.0), px(-12_000.0))));
    settle();
    let (made, drawn) = ask(&window, move |v, _| taken(v));
    check(500, &made, &drawn);
    assert!(!made.contains(&0) && !made.contains(&999), "{made:?}");
    window.close();
}

/// A window that fits what it shows whose body fills it over a footer,
/// as Quick Look's picture over its bar.
#[derive(Default)]
struct Fills {
    /// The body's height as last drawn.
    body: std::rc::Rc<std::cell::Cell<f32>>,
}

impl View for Fills {
    type Message = ();

    fn body(&mut self, _: &mut Cx<Self>) -> Element {
        let body = self.body.clone();
        div().size_full().flex().flex_col().child(olo_kit::canvas(move |_, b| body.set(f32::from(b.size.height))).w_full().flex_1().min_h(px(0.0))).child(div().w_full().h(px(40.0)).flex_shrink_0())
    }
}

/// A window that fits what it shows, asked by its app to be taller than
/// the screen has room for (Quick Look sized to a picture on a taller
/// screen than the shortest), is brought back to the room and its body
/// still fills it: scrolled, its body was laid out to nothing (Ben,
/// 2026-11-14: Quick Look "i just get an empty window" after ↓, an empty
/// body over a scroll bar).
#[test]
fn ui_lint_a_fitted_window_past_the_screen_keeps_what_fills_it() {
    struct Room;
    impl Drop for Room {
        fn drop(&mut self) {
            olo_kit::lint::set_room_for_tests(None);
        }
    }
    let _lint = Lint::new();
    olo_kit::lint::set_room_for_tests(Some(300.0));
    let _room = Room;
    let window = olo_kit::open_window(WindowOptions::new("Fills", 400.0, 200.0).dialog(), |_| Fills::default());
    ask(&window, |_, _| ());
    // (As the Frame follows: until the window and its body stay put.)
    let settled = || {
        let mut seen = (0.0, 0.0);
        for _ in 0..200 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            let now = ask(&window, |v, cx| (f32::from(cx.viewport_size().height), v.body.get()));
            if now == seen && now.1 > 0.0 {
                break;
            }
            seen = now;
        }
        seen
    };
    // Twice, as Quick Look sizes itself to each picture it steps to.
    for tall in [500.0, 480.0] {
        ask(&window, move |_, cx| cx.resize_window(olo_kit::size(px(400.0), px(tall))));
        let (height, body) = settled();
        assert!(height <= 300.5, "asked {tall} tall, the window is {height}, past the room (300)");
        assert!(body >= height - 40.5, "asked {tall} tall, its body is {body} of a window {height} tall: laid out to nothing");
    }
    window.close();
}

/// Two hundred rows in a scroller that can be hidden (a list on a tab
/// that isn't chosen), kept by its view; which rows are made.
struct Hides {
    shown: bool,
    scroll: olo_kit::ScrollHandle,
    made: std::rc::Rc<std::cell::RefCell<Vec<usize>>>,
}

impl View for Hides {
    type Message = ();

    fn body(&mut self, _: &mut Cx<Self>) -> Element {
        use olo_kit::InteractiveElement as _;
        if !self.shown {
            return div().size_full();
        }
        let made = self.made.clone();
        let rows = olo_kit::uniform_rows(200, px(20.0), move |near| {
            near.map(|row| {
                made.borrow_mut().push(row);
                div().id(row).w_full().h(px(20.0))
            })
            .collect()
        });
        div().size_full().overflow_y_scroll().track_scroll(&self.scroll).child(rows)
    }
}

/// A list scrolled, hidden and shown again, scrolls from where it was:
/// the display thread, which let it go while it wasn't shown, starts it
/// where its looper has it. (It started it at its top, where none of its
/// rows were made, and the looper took all it said of it after for
/// stale: Agent Screens' History, back from another tab, was blank, its
/// scroll bar at the end, and the wheel did nothing; Ben, 2026-11-14.)
#[test]
fn ui_lint_a_list_shown_again_scrolls_from_where_it_was() {
    use olo_kit::point;
    let _lint = Lint::new();
    let window = olo_kit::open_window(WindowOptions::new("Hides", 200.0, 100.0), |_| Hides { shown: true, scroll: Default::default(), made: Default::default() });
    let settle = || {
        ask(&window, |_, _| ());
        ask(&window, |_, _| ());
    };
    settle();
    let wheel = |down: f32| {
        let turned = ask(&window, move |_, cx| cx.headless_wheel(point(px(100.0), px(50.0)), point(px(0.0), px(down))));
        turned.recv_timeout(std::time::Duration::from_secs(10)).expect("the display thread scrolls");
        ask(&window, |v, _| f32::from(v.scroll.offset().y))
    };
    // Scrolled by its view, then by the wheel: the display thread has it
    // where the looper put it.
    ask(&window, |v, cx| v.scroll.set_offset(cx, point(px(0.0), px(-2000.0))));
    settle();
    assert_eq!(wheel(40.0), -2040.0, "the wheel scrolls on from where the view put it");
    // Hidden (its layer left out of what's committed), then shown again.
    for shown in [false, true] {
        ask(&window, move |v, cx| {
            v.shown = shown;
            cx.invalidate();
        });
        settle();
    }
    let made = ask(&window, |v, _| std::mem::take(&mut *v.made.borrow_mut()));
    assert!(made.contains(&102), "the rows where it was are made: {made:?}");
    // The wheel moves it from there, both ways, and the rows it comes to
    // are made.
    assert_eq!(wheel(-100.0), -1940.0, "the wheel up, shown again");
    assert_eq!(wheel(60.0), -2000.0, "the wheel down, shown again");
    let made = ask(&window, |v, _| std::mem::take(&mut *v.made.borrow_mut()));
    assert!(made.contains(&97), "the rows it came to are made: {made:?}");
    window.close();
}

/// A list acted on by what's checked (`olo_kit::chosen`): three items,
/// each a check box beside its title, and under them Choose and the
/// counted button, `checked` of each kind (of two problems and one
/// suggestion).
struct Picks {
    checked: [usize; 2],
}

const PROBLEM: olo_kit::chosen::Kind = olo_kit::chosen::Kind { verb: "Fix", one: "Problem", many: "Problems", by_default: true };
const SUGGESTION: olo_kit::chosen::Kind = olo_kit::chosen::Kind { verb: "Apply", one: "Suggestion", many: "Suggestions", by_default: false };

impl View for Picks {
    type Message = ();

    fn body(&mut self, _: &mut Cx<Self>) -> Element {
        use olo_kit::{chosen, style, widgets::StringView};
        let counts = [(&PROBLEM, self.checked[0], 2), (&SUGGESTION, self.checked[1], 1)];
        let item = |title: &'static str, on: bool| {
            div()
                .flex()
                .items_start()
                .gap(px(m::MARK_GAP))
                .child(CheckBox::new(title, on).mark_only().height(style::line_row(style::PLAIN_SIZE)))
                .child(div().flex_1().min_w_0().child(StringView::new(title).bold(true).wrap(true)))
        };
        div()
            .size_full()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(item("Rtkit isn't running", self.checked[0] > 0))
            .child(item("USB mice may sleep", self.checked[0] > 1))
            .child(item("Install just", self.checked[1] > 0))
            .child(div().flex().child(div().flex_1()).child(chosen::bar("chosen", &counts, true, |_, _| {}, |_, _| {})))
    }
}

/// A list acted on by what's checked is clean in every look at every
/// scale, with nothing, some or everything checked; agents read each
/// item's box by its title, the Choose links and the button by what
/// they say; and the button keeps its width as it counts, so nothing
/// beside it moves.
#[test]
fn ui_lint_a_list_acted_on_by_whats_checked_is_clean() {
    // (Wide enough for the bar in one row, its links set apart by dots,
    // as `chosen::bar_width` measures it in the language the lint runs in:
    // French's was 2 points wider than the 440 English has room in; a
    // narrower window has `chosen::bar_within` fold them, as Doctor's.)
    let lint = Lint::new();
    let bar = olo_kit::chosen::bar_width(&[(&PROBLEM, 0, 2), (&SUGGESTION, 0, 1)]) + 2.0 * m::WINDOW_INSET;
    drop(lint);
    let options = WindowOptions::new("Picks", bar.ceil().max(440.0), 146.0);
    let mut widths = Vec::new();
    for checked in [[0, 0], [2, 0], [1, 1], [2, 1], [0, 1]] {
        let mut lint = Lint::new();
        assert_clean(&format!("{checked:?} checked"), &lint.window(&options, move |_| Picks { checked }));
        let (_, nodes) = lint.trees(&options, move |_| Picks { checked }).remove(0);
        let named =
            |role: &str, name: &str| nodes.iter().find(|n| n.role == role && n.name.as_deref() == Some(name)).unwrap_or_else(|| panic!("{checked:?}: no {role} {name:?} in {nodes:#?}")).clone();
        // (By what they say, in the language the lint runs in.)
        let said = olo_kit::words::tr;
        assert_eq!(named("CheckBox", said("Install just")).checked, Some(checked[1] > 0));
        for link in ["All", "Problems", "Suggestions", "None"] {
            named("Button", said(link));
        }
        let words = olo_kit::chosen::label(&[(&PROBLEM, checked[0]), (&SUGGESTION, checked[1])]);
        let button = named("Button", &words);
        assert_eq!(button.enabled, checked != [0, 0], "{words}: dimmed only with nothing checked");
        widths.push((words, button.x, button.width));
    }
    assert!(widths.iter().all(|w| (w.1, w.2) == (widths[0].1, widths[0].2)), "the button moved or changed width as it counted: {widths:?}");
}

/// Text measured without a window (`style::text_width_app`) counts the
/// characters its font hasn't, drawn from another: Deskbar's "↓12K ↑3K"
/// beside a widget's name was measured without its arrows, and drawn
/// over the name.
#[test]
fn ui_lint_text_measured_without_a_window_counts_every_character() {
    use olo_kit::{FontWeight, style};
    let _lint = Lint::new();
    let font = style::ui_font(FontWeight::NORMAL);
    let digits = style::text_width_app("12K 3K", style::SMALL_SIZE, &font);
    let arrows = style::text_width_app("↓12K ↑3K", style::SMALL_SIZE, &font);
    assert!(arrows >= digits + 6.0, "two arrows add {:.1} pt", arrows - digits);
}

/// A month to pick a day from, under what steps through it: a pair of
/// arrow buttons, Today, a title and a status cut to what room is left,
/// on the title's line.
struct Month;

impl View for Month {
    type Message = ();

    fn body(&mut self, _: &mut Cx<Self>) -> Element {
        use olo_kit::{
            style,
            widgets::{ArrowButton, MonthGrid, StringView, month::Day},
        };
        let nav = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::SPACING))
            .child(div().flex().flex_row().flex_shrink_0().child(ArrowButton::back("Previous Month")).child(ArrowButton::on("Next Month")))
            .child(Button::new("Today"))
            .child(div().flex_shrink_0().child(StringView::new("November 2026").size(style::LEAD_SIZE).bold(true).height(m::CONTROL_HEIGHT).beside(style::PLAIN_SIZE)))
            .child(div().flex_1().min_w_0().child(StringView::new("Updated a long while ago, longer than there is room to say").small(true).dim(true).cut(true).beside(style::PLAIN_SIZE)));
        let month = MonthGrid::new("Month", 2026, 11, Day::new(2026, 11, 18))
            .week_start(6)
            .chosen(Day::new(2026, 11, 15), Day::new(2026, 11, 21))
            .marked(vec![Day::new(2026, 11, 3), Day::new(2026, 11, 18), Day::new(2026, 11, 20), Day::new(2026, 12, 1)])
            .on_pick(|_, _| {})
            .on_turn(|_, _| {});
        div().size_full().p(px(m::WINDOW_INSET)).flex().flex_col().gap(px(m::SPACING)).child(nav).child(div().flex().child(month))
    }
}

#[test]
fn ui_lint_a_month_and_its_arrows_are_clean() {
    use olo_kit::widgets::month;
    let mut lint = Lint::new();
    let height = 2.0 * m::WINDOW_INSET + m::CONTROL_HEIGHT + m::SPACING + month::HEIGHT;
    let found = lint.window(&WindowOptions::new("Month", 420.0, height), |_| Month);
    assert_clean("a month under its arrows", &found);
}

/// A tab view given every tab's page (`TabView::pages`): a short one, a
/// tall one, and one that fills the panel to its frame with a list longer
/// than the panel (`fill_page`), scrolled there.
struct Paged {
    chosen: usize,
    scroll: olo_kit::ScrollHandle,
}

impl View for Paged {
    type Message = usize;

    fn message(&mut self, tab: usize, cx: &mut Cx<Self>) {
        self.chosen = tab;
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        use olo_kit::IntoElement as _;
        use olo_kit::widgets::{ScrollView, StringView, TabView, tabs};
        let names: Vec<olo_kit::SharedString> = ["Short", "Tall", "List"].into_iter().map(Into::into).collect();
        let chosen = cx.messenger();
        let short = div().w_full().p(px(m::SPACING)).child(StringView::new("One line")).into_element();
        let tall =
            div().w_full().p(px(m::SPACING)).flex().flex_col().gap(px(m::SPACING)).children(["First", "Second", "Third", "Fourth", "Fifth", "Sixth"].into_iter().map(StringView::new)).into_element();
        let inset = tabs::fill_reach() + m::SPACING;
        let rows = (0..30).map(|i| StringView::new(format!("Row {i}")));
        let list = ScrollView::new(&self.scroll).border(false).auto_hide(true).child(div().w_full().p(px(inset)).flex().flex_col().gap(px(m::SPACING)).children(rows)).into_element();
        let paged = TabView::new("paged", names, self.chosen)
            .grows()
            .on_select(move |i, _| {
                chosen.send(*i);
            })
            .pages([short, tall, list])
            .fill_page(2);
        div().w_full().p(px(m::WINDOW_INSET)).child(paged)
    }
}

/// Every tab's page laid out: the window as tall with each tab chosen
/// (the tallest page's height: Rex's Host window's foot moved by 290
/// points from tab to tab before, 2026-11-24); nothing of a page not
/// chosen reaches the lint (its words were read over the chosen page's);
/// the page that fills the panel scrolls its list inside the frame, its
/// bar against it, nothing past the pane.
#[test]
fn ui_lint_a_tab_views_pages_keep_one_height_and_show_one() {
    let mut lint = Lint::new();
    let options = WindowOptions::new("Paged", 360.0, 120.0).fits_content();
    assert_clean("the pages", &lint.window(&options, |_| Paged { chosen: 0, scroll: olo_kit::ScrollHandle::new() }));
    let window = olo_kit::open_window(options, |_| Paged { chosen: 0, scroll: olo_kit::ScrollHandle::new() });
    let mut heights = Vec::new();
    for tab in [0, 1, 2, 0] {
        ask(&window, move |v, cx| {
            v.chosen = tab;
            cx.invalidate();
        });
        // (As the window follows what it shows: until it stays put.)
        let mut seen = -1.0;
        for _ in 0..100 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            let now = ask(&window, |_, cx| f32::from(cx.viewport_size().height));
            if now == seen {
                break;
            }
            seen = now;
        }
        heights.push(seen);
    }
    window.close();
    assert!(heights.iter().all(|h| (h - heights[0]).abs() < 0.5), "the window's height with each tab chosen: {heights:?}");
    assert!(heights[0] > 120.0, "as tall as the tall page: {heights:?}");
}

/// A text field with the keyboard and a segmented control beside it.
struct Segments {
    field: Child<TextField>,
    chosen: usize,
}

impl View for Segments {
    type Message = usize;

    fn message(&mut self, i: usize, cx: &mut Cx<Self>) {
        self.chosen = i;
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let chosen = cx.messenger();
        let segments = olo_kit::widgets::SegmentedControl::new("views", ["Sessions", "Activity"], self.chosen).on_choose(move |i, _| {
            chosen.send(*i);
        });
        div().size_full().p(px(m::WINDOW_INSET)).flex().flex_col().gap(px(m::SPACING)).child(segments).child(TextControl::new(cx, &self.field).label("Name:").divider(60.0))
    }
}

/// A click on a segment chooses it and leaves the keyboard where it was,
/// as a button's does (BeOS's controls): Rex's Sessions and Activity, given
/// the keyboard by a click, took the Space typed next from the pane.
#[test]
fn ui_lint_a_segment_clicked_leaves_the_keyboard_where_it_was() {
    let _lint = Lint::new();
    let window = olo_kit::open_window(WindowOptions::new("Segments", 360.0, 160.0), |cx| Segments { field: text_field(cx, |f| f), chosen: 0 });
    ask(&window, |v, cx| v.field.focus(cx));
    assert!(ask(&window, |v, cx| v.field.is_focused(cx)), "the field has the keyboard to begin with");
    // (The second segment: right of the control's middle.)
    let at = ask(&window, |_, cx| {
        let c = cx.agent_nodes().into_iter().find(|n| n.role == "Segment" && n.name.as_ref().is_some_and(|s| s.as_ref() == "Activity")).map(|n| n.bounds).expect("the Activity segment");
        olo_kit::point(c.origin.x + c.size.width / 2.0, c.origin.y + c.size.height / 2.0)
    });
    ask(&window, move |_, cx| cx.headless_click(at));
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert_eq!(ask(&window, |v, _| v.chosen), 1, "the segment is chosen");
    assert!(ask(&window, |v, cx| v.field.is_focused(cx)), "and the field keeps the keyboard");
    window.close();
}

//! Acting on what's chosen (`olo_kit::chosen`): a list of problems and
//! suggestions, each checked to be fixed or applied with the others (the
//! problems to begin with), Choose: All, Problems, Suggestions, None, and
//! one button saying what it will do ("Fix 2 and Apply 1"). The same in
//! the Fix menu, Choose All on ⌘A. Each item keeps its own button too.
//! Olo Doctor's list is this.

use olo_kit::{
    ClickEvent, Cx, Element, Menu, MenuBar, ParentElement as _, Styled as _, View, WindowOptions,
    chosen::{self, Choice, Chosen, Kind},
    div, metrics as m, px, style,
    widgets::{Button, CheckBox, StringView},
};

const PROBLEM: Kind = Kind { verb: "Fix", one: "Problem", many: "Problems", by_default: true };
const SUGGESTION: Kind = Kind { verb: "Apply", one: "Suggestion", many: "Suggestions", by_default: false };
const KINDS: [Kind; 2] = [PROBLEM, SUGGESTION];

/// The list: an id, a title, its kind, and its own button's words.
const ITEMS: &[(&str, &str, Kind, &str)] = &[
    ("rtkit", "Real-time scheduling isn't allowed", PROBLEM, "Allow It"),
    ("usb-sleep", "Two USB mice may fall asleep", PROBLEM, "Keep Them Awake"),
    ("just", "Install just, a command runner", SUGGESTION, "Install"),
    ("zram", "Compress memory before swapping", SUGGESTION, "Turn On"),
];

struct Example {
    chosen: Chosen,
    done: Vec<&'static str>,
}

impl Example {
    /// What's still to do.
    fn left(&self) -> impl Iterator<Item = &'static (&'static str, &'static str, Kind, &'static str)> + '_ {
        ITEMS.iter().filter(|(id, ..)| !self.done.contains(id))
    }

    /// How many of each kind are checked, of how many.
    fn counts(&self) -> [chosen::Count<'static>; 2] {
        let n = |k: &'static Kind| (k, self.left().filter(|(id, _, kind, _)| kind == k && self.chosen.is(id, kind)).count(), ITEMS.iter().filter(|(_, _, kind, _)| kind == k).count());
        [n(&KINDS[0]), n(&KINDS[1])]
    }

    fn choose(&mut self, choice: Choice, cx: &mut Cx<Self>) {
        let items = self.left().map(|(id, _, kind, _)| (*id, kind)).collect::<Vec<_>>();
        self.chosen.choose(items, |k| KINDS.iter().position(|o| o == k).is_some_and(|i| choice.picks(i)));
        cx.invalidate();
    }

    /// The button: what's checked, done.
    fn act(&mut self, cx: &mut Cx<Self>) {
        let checked: Vec<&'static str> = self.left().filter(|(id, _, kind, _)| self.chosen.is(id, kind)).map(|(id, ..)| *id).collect();
        self.done.extend(checked);
        cx.invalidate();
    }
}

impl View for Example {
    type Message = ();

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        match command {
            "act" => self.act(cx),
            c if Choice::of_command(c).is_some() => self.choose(Choice::of_command(c).expect("just seen"), cx),
            "again" => {
                self.done.clear();
                cx.invalidate();
            }
            "close" | "quit" => olo_kit::quit(),
            _ => return false,
        }
        true
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let counts = self.counts();
        let menus = vec![Menu::new("File").app_items(&[]).item("Start Again", "again").close_quit(), chosen::menu(Menu::new("Fix"), "act", &counts, true)];
        let mut list = div().flex().flex_col().gap(px(m::SPACING));
        for &(id, title, kind, does) in self.left() {
            let mark = CheckBox::new(title, self.chosen.is(id, &kind)).id(format!("choose-{id}")).mark_only().height(style::line_row(style::PLAIN_SIZE)).on_toggle(cx.listener(
                move |e: &mut Example, on: &bool, cx| {
                    e.chosen.set(id, &kind, *on);
                    cx.invalidate();
                },
            ));
            let own = Button::new(does).id(format!("do-{id}")).on_click(cx.listener(move |e: &mut Example, _: &ClickEvent, cx| {
                e.done.push(id);
                cx.invalidate();
            }));
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(m::MARK_GAP))
                    .child(mark)
                    .child(div().flex_1().min_w_0().child(StringView::new(title).bold(true).wrap(true)))
                    .child(StringView::new(if kind == PROBLEM { "Problem" } else { "Suggestion" }).dim(true))
                    .child(own),
            );
        }
        if self.left().next().is_none() {
            list = list.child(StringView::new("Everything is done. File ▸ Start Again.").dim(true));
        }
        div().size_full().flex().flex_col().child(MenuBar::new(cx, menus)).child(div().flex().flex_col().gap(px(m::SECTION)).p(px(m::WINDOW_INSET)).child(list).child(
            div().flex().child(div().flex_1()).child(chosen::bar(
                "chosen",
                &counts,
                true,
                cx.listener(|e: &mut Example, c: &Choice, cx| e.choose(*c, cx)),
                cx.listener(|e: &mut Example, _: &ClickEvent, cx| e.act(cx)),
            )),
        ))
    }
}

fn main() {
    olo_kit::run("olo.chosen-example", |_| {
        olo_kit::open_window(WindowOptions::new("Chosen", 560.0, 120.0).fits_content(), |_| Example { chosen: Chosen::default(), done: Vec::new() });
    });
}

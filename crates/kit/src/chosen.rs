//! Acting on what's chosen: a list whose items can each be acted on,
//! of a few kinds that are acted on differently (Olo Doctor's problems,
//! fixed, and its suggestions, applied), where the person wants all of
//! it, one kind, either, some, or one.
//!
//! BeOS had a button on each thing and nothing for many at once. The
//! pattern here is Mac OS X's Software Update (each update with a check
//! box, one button "Install 3 Items"), Ubuntu's Software Updater, and
//! Xcode's Update to Recommended Settings (a check box on each change, an
//! optional one left unchecked, one Perform Changes):
//!
//! - Each item that can be acted on has a check box (`CheckBox::mark_only`
//!   beside its own title, named by it for agents), checked to begin
//!   with by its kind ([`Kind::by_default`]: what's needed, checked; what's
//!   only suggested, not), the person's own choice kept from then on
//!   ([`Chosen`]), as the list is looked at again.
//! - One default button does what's checked and says so, counted, in
//!   each kind's verb ([`label`]): "Fix 2 Problems", "Apply 3
//!   Suggestions", "Fix 2 and Apply 3"; dimmed with nothing checked.
//! - Beside it, Choose: All, each kind, None ([`choose`]; the two in a
//!   row, [`bar`]; the button alone, [`act`]): one click for both, either
//!   or neither, before choosing one by one. The menus have the same,
//!   the button's command first ([`menu`]), Choose All on ⌘A (the app's
//!   `choose-all` in `app_keys`, so it's configurable; not Select All's
//!   command, which is Edit's).
//! - Each item keeps its own button too, to do just that one now.

use std::{collections::BTreeMap, rc::Rc};

use olo_interface::{ClickEvent, Element, IntoElement as _, ParentElement as _, Styled as _, WindowCx, div, px};
use olo_kit_base::metrics as m;

use crate::{
    menu::Menu,
    style,
    widgets::{Button, LinkButton, StringView, button_width},
};

/// A kind of item, and what acting on it is called.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kind {
    /// What the button does to it: "Fix", "Apply".
    pub verb: &'static str,
    /// Its name, one and many: "Problem", "Problems".
    pub one: &'static str,
    pub many: &'static str,
    /// Checked to begin with: what's needed is, what's only suggested
    /// isn't.
    pub by_default: bool,
}

/// What the person checked and unchecked, by item: an item they haven't
/// touched goes by its kind's default. What's gone from the list is
/// forgotten ([`Chosen::keep`]).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Chosen {
    changed: BTreeMap<String, bool>,
}

impl Chosen {
    /// Whether item `id`, of `kind`, is checked.
    pub fn is(&self, id: &str, kind: &Kind) -> bool {
        self.changed.get(id).copied().unwrap_or(kind.by_default)
    }

    /// Check item `id` (`on`) or uncheck it.
    pub fn set(&mut self, id: &str, kind: &Kind, on: bool) {
        if on == kind.by_default {
            self.changed.remove(id);
        } else {
            self.changed.insert(id.to_owned(), on);
        }
    }

    /// Choose: each of `items` (its id and kind) checked if `pick` picks
    /// its kind (All: every one; a kind: those of it; None: none).
    pub fn choose<'a>(&mut self, items: impl IntoIterator<Item = (&'a str, &'a Kind)>, pick: impl Fn(&Kind) -> bool) {
        for (id, kind) in items {
            self.set(id, kind, pick(kind));
        }
    }

    /// Only the items still listed are remembered.
    pub fn keep<'a>(&mut self, ids: impl IntoIterator<Item = &'a str>) {
        let ids: std::collections::BTreeSet<&str> = ids.into_iter().collect();
        self.changed.retain(|id, _| ids.contains(id.as_str()));
    }
}

/// The button's words for `counts` checked of each kind: "Fix 2
/// Problems", "Apply 1 Suggestion", "Fix 2 and Apply 3" (kinds with one
/// verb counted together); none, the first kind's verb alone ("Fix").
pub fn label(counts: &[(&Kind, usize)]) -> String {
    // (Kinds that share a verb are counted together, in the order met.)
    let mut verbs: Vec<(&str, usize, Vec<&Kind>)> = Vec::new();
    for &(kind, n) in counts.iter().filter(|(_, n)| *n > 0) {
        match verbs.iter_mut().find(|(v, ..)| *v == kind.verb) {
            Some((_, total, kinds)) => {
                *total += n;
                if !kinds.contains(&kind) {
                    kinds.push(kind);
                }
            }
            None => verbs.push((kind.verb, n, vec![kind])),
        }
    }
    match &verbs[..] {
        [] => counts.first().map_or("Do It", |(k, _)| k.verb).to_owned(),
        [(verb, n, kinds)] if kinds.len() == 1 => format!("{verb} {n} {}", if *n == 1 { kinds[0].one } else { kinds[0].many }),
        [(verb, n, _)] => format!("{verb} {n}"),
        more => {
            let parts: Vec<String> = more.iter().map(|(verb, n, _)| format!("{verb} {n}")).collect();
            let (last, rest) = parts.split_last().expect("two or more");
            format!("{} and {last}", rest.join(", "))
        }
    }
}

/// What a Choose link or menu item picks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    All,
    /// The kinds' index in what was given.
    Kind(usize),
    None,
}

impl Choice {
    /// Whether it picks `kind`, the `i`th.
    pub fn picks(self, i: usize) -> bool {
        match self {
            Choice::All => true,
            Choice::Kind(k) => k == i,
            Choice::None => false,
        }
    }

    /// A menu command's choice ([`menu`]'s).
    pub fn of_command(command: &str) -> Option<Choice> {
        match command {
            "choose-all" => Some(Choice::All),
            "choose-none" => Some(Choice::None),
            c => c.strip_prefix("choose-kind-").and_then(|k| k.parse().ok()).map(Choice::Kind),
        }
    }
}

type OnChoice = Rc<dyn Fn(&Choice, &mut WindowCx)>;

/// How many of a kind are checked, of how many there are.
pub type Count<'a> = (&'a Kind, usize, usize);

fn checked<'a>(counts: &[Count<'a>]) -> Vec<(&'a Kind, usize)> {
    counts.iter().map(|&(k, n, _)| (k, n)).collect()
}

/// "Choose: All · Problems · Suggestions · None", as links: one click
/// for both, either or neither (`counts` gives the kinds, in order).
/// `id` names the links; `enabled` false while nothing can be chosen (a
/// fix running).
pub fn choose(id: &str, counts: &[Count], enabled: bool, on_choice: impl Fn(&Choice, &mut WindowCx) + 'static) -> Element {
    let on_choice: OnChoice = Rc::new(on_choice);
    let link = |label: &str, key: String, choice: Choice| {
        let f = on_choice.clone();
        LinkButton::new(label.to_owned()).id(format!("{id}-{key}")).beside(style::PLAIN_SIZE).enabled(enabled).on_click(move |_: &ClickEvent, cx| f(&choice, cx)).into_element()
    };
    // (Its words and the links' on a push button's baseline: the row
    // reads as one line with the buttons beside it.)
    let mut row = div().flex().items_center().gap(px(m::TIGHT)).child(StringView::new("Choose:").dim(true).small(true).height(m::LABEL_ROW).beside(style::PLAIN_SIZE));
    row = row.child(link("All", "all".into(), Choice::All));
    for (i, (kind, ..)) in counts.iter().enumerate() {
        row = row.child(link(kind.many, format!("kind-{i}"), Choice::Kind(i)));
    }
    row.child(link("None", "none".into(), Choice::None))
}

/// The counted default button: [`label`] of what's checked, dimmed with
/// nothing checked or `enabled` false. As wide as its widest words could
/// be (every one of each kind checked, in any mix), so nothing beside it
/// moves as it counts.
pub fn act(id: &str, counts: &[Count], enabled: bool, on_act: impl Fn(&ClickEvent, &mut WindowCx) + 'static) -> Element {
    let any = counts.iter().any(|(_, n, _)| *n > 0);
    let widest = (0u32..1 << counts.len())
        .map(|mask| {
            let all: Vec<(&Kind, usize)> = counts.iter().enumerate().map(|(i, &(k, _, of))| (k, if mask & (1 << i) != 0 { of } else { 0 })).collect();
            button_width(&label(&all), false)
        })
        .fold(0.0, f32::max);
    Button::new(label(&checked(counts))).id(format!("{id}-act")).min_width(widest).default(any && enabled).enabled(any && enabled).on_click(on_act).into_element()
}

/// The two in a row under such a list, the button at its right.
pub fn bar(id: &str, counts: &[Count], enabled: bool, on_choice: impl Fn(&Choice, &mut WindowCx) + 'static, on_act: impl Fn(&ClickEvent, &mut WindowCx) + 'static) -> Element {
    div().flex().items_center().gap(px(m::SPACING)).child(choose(id, counts, enabled, on_choice)).child(act(id, counts, enabled, on_act))
}

/// The same in a menu: the button's command (`act`, its words as the
/// button's), then Choose All (⌘A: list the app's `choose-all` in
/// `app_keys`), each kind, None.
pub fn menu(mut m: Menu, act: &str, counts: &[Count], enabled: bool) -> Menu {
    let any = counts.iter().any(|(_, n, _)| *n > 0);
    m = m.item(label(&checked(counts)), act.to_owned()).enabled(any && enabled).separator();
    // (⌘A unless the app's keys say otherwise: its own `choose-all`.)
    m = m.item("Choose All", "choose-all").key("a").enabled(enabled);
    for (i, (kind, ..)) in counts.iter().enumerate() {
        m = m.item(format!("Choose {}", kind.many), format!("choose-kind-{i}")).enabled(enabled);
    }
    m.item("Choose None", "choose-none").enabled(enabled)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIX: Kind = Kind { verb: "Fix", one: "Problem", many: "Problems", by_default: true };
    const APPLY: Kind = Kind { verb: "Apply", one: "Suggestion", many: "Suggestions", by_default: false };

    /// The button says what it will do, counted, in each kind's verb.
    #[test]
    fn the_button_says_what_it_does() {
        assert_eq!(label(&[(&FIX, 2), (&APPLY, 0)]), "Fix 2 Problems");
        assert_eq!(label(&[(&FIX, 0), (&APPLY, 1)]), "Apply 1 Suggestion");
        assert_eq!(label(&[(&FIX, 1), (&APPLY, 3)]), "Fix 1 and Apply 3");
        assert_eq!(label(&[(&FIX, 0), (&APPLY, 0)]), "Fix");
        // Two kinds with one verb: counted together.
        let urgent = Kind { one: "Critical Problem", many: "Critical Problems", ..FIX };
        assert_eq!(label(&[(&urgent, 1), (&FIX, 2)]), "Fix 3");
    }

    /// Checked by its kind until the person says otherwise, and kept so;
    /// Choose sets every one; what's gone is forgotten.
    #[test]
    fn chosen_goes_by_kind_until_changed() {
        let mut c = Chosen::default();
        assert!(c.is("a", &FIX) && !c.is("s", &APPLY));
        c.set("a", &FIX, false);
        c.set("s", &APPLY, true);
        assert!(!c.is("a", &FIX) && c.is("s", &APPLY));
        let items = [("a", &FIX), ("b", &FIX), ("s", &APPLY)];
        c.choose(items, |k| *k == APPLY);
        assert!(!c.is("a", &FIX) && !c.is("b", &FIX) && c.is("s", &APPLY));
        c.choose(items, |_| true);
        assert!(items.iter().all(|(id, k)| c.is(id, k)));
        c.set("s", &APPLY, true);
        c.keep(["a"]);
        assert_eq!(c, Chosen::default(), "s gone, a as its kind has it");
        assert_eq!(Choice::of_command("choose-all"), Some(Choice::All));
        assert_eq!(Choice::of_command("choose-kind-1"), Some(Choice::Kind(1)));
        assert!(Choice::Kind(1).picks(1) && !Choice::Kind(1).picks(0) && !Choice::None.picks(0));
    }
}

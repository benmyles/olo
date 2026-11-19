//! The History section: every transaction (when, who, what it did, the
//! command or comment), the selected one's packages by what was done to
//! each (installed, updated from which version, removed, …), and Undo:
//! the opposite of what it did, as one change to review (what it
//! installed removed, what it removed installed again, what it updated
//! put back to the version before, if the repositories still have it).

use std::rc::Rc;

use olo_kit::{
    ClickEvent, Cx, Element, IntoElement as _, ParentElement as _, ScrollHandle, Styled as _, div, metrics as m, px, theme,
    widgets::{Button, Column, ColumnListView, ListModel, ScrollView, StringView},
};
use olo_system::dnf::{Change2, Transaction};

use crate::{
    OsUpdates,
    review::{self, Op, Plan},
    updates::{field, heading, text},
    words,
};

const DETAILS: f32 = 300.0;

#[derive(Clone)]
pub enum Msg {
    Selected,
    Undo,
}

pub struct State {
    list: ListModel,
    pub selected: Option<i64>,
    scroll: ScrollHandle,
}

impl Default for State {
    fn default() -> State {
        State { list: ListModel::new(false), selected: None, scroll: ScrollHandle::new() }
    }
}

impl State {
    pub fn took(&mut self, history: &[Transaction]) {
        let row = self.selected.and_then(|id| history.iter().position(|t| t.id == id));
        let mut st = self.list.state_mut();
        st.selected.clear();
        if let Some(r) = row {
            st.select(r);
        }
    }

    pub fn selected<'a>(&self, history: &'a [Transaction]) -> Option<&'a Transaction> {
        self.selected.and_then(|id| history.iter().find(|t| t.id == id))
    }
}

/// Whether a transaction can be undone: it changed something.
pub fn undoable(t: &Transaction) -> bool {
    !(t.installed.is_empty() && t.removed.is_empty() && t.upgraded.is_empty() && t.downgraded.is_empty())
}

/// What a transaction did, in a few words: "Updated 14, installed 2".
pub fn what(t: &Transaction) -> String {
    let mut parts = Vec::new();
    for (list, verb) in [(&t.upgraded, "Updated"), (&t.installed, "Installed"), (&t.removed, "Removed"), (&t.downgraded, "Downgraded"), (&t.reinstalled, "Reinstalled")] {
        if !list.is_empty() {
            parts.push(format!("{} {}", if parts.is_empty() { verb.to_owned() } else { verb.to_lowercase() }, list.len()));
        }
    }
    parts.join(", ")
}

/// A package by name, version and arch, as dnf takes it.
fn spec(c: &Change2, evr: &str) -> String {
    format!("{}-{evr}.{}", c.name, c.arch)
}

/// Undoing `t`: the opposite of each thing it did.
pub fn undo_plan(t: &Transaction) -> Plan {
    let mut ops: Vec<(Op, Vec<String>)> = Vec::new();
    let installed: Vec<String> = t.installed.iter().map(|c| spec(c, &c.evr)).collect();
    let removed: Vec<String> = t.removed.iter().map(|c| spec(c, &c.evr)).collect();
    let back: Vec<String> = t.upgraded.iter().filter(|c| !c.original_evr.is_empty()).map(|c| spec(c, &c.original_evr)).collect();
    let forward: Vec<String> = t.downgraded.iter().filter(|c| !c.original_evr.is_empty()).map(|c| spec(c, &c.original_evr)).collect();
    for (op, list) in [(Op::Remove, installed), (Op::Install, removed), (Op::Downgrade, back), (Op::Upgrade, forward)] {
        if !list.is_empty() {
            ops.push((op, list));
        }
    }
    let title = olo_kit::tr!("Undo Transaction {}", t.id);
    Plan { title: title.clone(), ops, release: None, offline_only: false, comment: title }
}

pub fn message(app: &mut OsUpdates, msg: Msg, cx: &mut Cx<OsUpdates>) {
    match msg {
        Msg::Selected => app.h.selected = app.h.list.state().single().and_then(|r| app.history.get(r)).map(|t| t.id),
        Msg::Undo => {
            if let Some(t) = app.h.selected(&app.history).filter(|t| undoable(t)) {
                review::open(undo_plan(t), cx.messenger());
            }
        }
    }
}

pub fn command(app: &mut OsUpdates, command: &str, cx: &mut Cx<OsUpdates>) -> bool {
    match command {
        "undo-transaction" => message(app, Msg::Undo, cx),
        _ => return false,
    }
    true
}

pub fn body(app: &mut OsUpdates, cx: &mut Cx<OsUpdates>) -> Element {
    let p = theme::palette();
    let columns: Rc<[Column]> = Rc::from(vec![
        Column::new("Id", 44.0).right(),
        Column::new("When", 150.0),
        Column::new("What", 190.0),
        Column::new("By", 70.0),
        Column::new("Command or Comment", 220.0),
        Column::new("Result", 60.0),
    ]);
    let n = app.history.len();
    let list = ColumnListView::new(cx, "history-list", &app.h.list, columns, n, |v: &OsUpdates, cell, g| {
        let Some(t) = v.history.get(cell.row) else { return };
        let s = match cell.column {
            0 => t.id.to_string(),
            1 => words::date_time(t.end.max(t.start)),
            2 => what(t),
            3 => words::user(t.user_id),
            4 => {
                if t.comment.is_empty() {
                    t.description.clone()
                } else {
                    t.comment.clone()
                }
            }
            _ => t.status.clone(),
        };
        cell.text(g, s);
    })
    .empty_text("Nothing in the history yet.")
    .on_select(cx.listener(|v: &mut OsUpdates, _: &(), cx| {
        message(v, Msg::Selected, cx);
        cx.invalidate();
    }))
    .on_open(cx.listener(|v: &mut OsUpdates, _: &usize, cx| {
        message(v, Msg::Undo, cx);
    }));
    let details = details_view(app, cx);
    let can = app.h.selected(&app.history).is_some_and(undoable) && app.loading.is_none();
    let foot = div()
        .flex()
        .items_center()
        .gap(px(m::SPACING))
        .child(Button::new("Undo…").id("undo").enabled(can).on_click(cx.listener(|v: &mut OsUpdates, _: &ClickEvent, cx| message(v, Msg::Undo, cx))))
        .child(div().flex_1().min_w_0().child(app.status(cx)));
    div()
        .size_full()
        .flex()
        .flex_col()
        .gap(px(m::SPACING))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_row()
                .gap(px(m::SPACING))
                .child(div().flex_1().min_w_0().h_full().child(crate::well(cx, p.list_background, list)))
                .child(div().w(px(DETAILS)).flex_shrink_0().h_full().child(details)),
        )
        .child(foot)
        .into_element()
}

fn details_view(app: &mut OsUpdates, cx: &mut Cx<OsUpdates>) -> Element {
    let p = theme::palette();
    let content: Element = match app.h.selected(&app.history) {
        None => StringView::new("Select a transaction to see what it did to each package.").dim(true).wrap(true).into_element(),
        Some(t) => {
            let mut col = div().flex().flex_col().gap(px(m::TIGHT));
            col = col.child(StringView::new(format!("Transaction {}", t.id)).bold(true).size(olo_kit::style::TITLE_SIZE).height(olo_kit::style::line_row(olo_kit::style::TITLE_SIZE)));
            col = col.child(field("When", text(words::date_time(t.start))));
            col = col.child(field("By", text(words::user(t.user_id))));
            if !t.description.is_empty() {
                col = col.child(field("Command", text(t.description.clone())));
            }
            if !t.comment.is_empty() {
                col = col.child(field("Comment", text(t.comment.clone())));
            }
            if !t.releasever.is_empty() {
                col = col.child(field("Fedora", text(t.releasever.clone())));
            }
            col = col.child(field("Result", text(t.status.clone())));
            for (list, title) in [(&t.upgraded, "Updated"), (&t.installed, "Installed"), (&t.removed, "Removed"), (&t.downgraded, "Downgraded"), (&t.reinstalled, "Reinstalled")] {
                if list.is_empty() {
                    continue;
                }
                col = col.child(heading(&format!("{title} ({})", list.len())));
                for c in list {
                    let version = if c.original_evr.is_empty() { c.evr.clone() } else { format!("{} → {}", c.original_evr, c.evr) };
                    col = col.child(
                        div()
                            .flex()
                            .flex_col()
                            .child(StringView::new(format!("{}.{}", c.name, c.arch)).wrap(true))
                            .child(StringView::new(format!("{version}{}", if c.repo_id.is_empty() { String::new() } else { format!(" · {}", c.repo_id) })).dim(true).small(true).wrap(true)),
                    );
                }
            }
            col.into_element()
        }
    };
    let inner = div().w_full().p(px(m::SPACING)).child(content);
    crate::well(cx, p.document_background, ScrollView::new(&app.h.scroll).border(false).auto_hide(true).background(p.document_background).child(inner))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_is_the_opposite() {
        let c = |name: &str, evr: &str, orig: &str| Change2 { name: name.into(), arch: "x86_64".into(), evr: evr.into(), original_evr: orig.into(), ..Default::default() };
        let t = Transaction {
            id: 7,
            installed: vec![c("foo", "1.0-1.fc45", "")],
            upgraded: vec![c("expat", "2.9.0-1.fc45", "2.8.3-1.fc45")],
            removed: vec![c("bar", "2:3.1-2.fc45", "")],
            ..Default::default()
        };
        let p = undo_plan(&t);
        assert_eq!(
            p.ops,
            vec![(Op::Remove, vec!["foo-1.0-1.fc45.x86_64".to_owned()]), (Op::Install, vec!["bar-2:3.1-2.fc45.x86_64".to_owned()]), (Op::Downgrade, vec!["expat-2.8.3-1.fc45.x86_64".to_owned()])]
        );
        assert_eq!(what(&t), "Updated 1, installed 1, removed 1");
    }
}

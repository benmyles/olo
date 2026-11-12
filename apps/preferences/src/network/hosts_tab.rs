//! Network ▸ Hosts: /etc/hosts's entries (address, names, a comment), each
//! on or off; added, changed, turned off and removed here, then saved as
//! root in one go (`olo_system::hosts`: comments and the rest of the file
//! kept, and only if nothing else changed it meanwhile). The machine's
//! own entries (localhost) are shown, not edited.

use olo_kit::{
    Child, Cx, IntoElement, ParentElement as _, Styled as _, WindowCx, div, metrics as m, px,
    widgets::{Button, CheckBox, Column, ColumnListView, ListModel, TextControl, TextField},
};
use olo_system::hosts::{Host, HostsFile};

use super::{Extra, Request, State, check, field, state_of};
use crate::{PanelView, form::Ui};

// (On the 4-point grid, as Ethernet's.)
const DIVIDER: f32 = 92.0;

pub struct Edit {
    /// The entries as edited here, not yet saved.
    pending: Option<Vec<Host>>,
    list: ListModel,
    address: Child<TextField>,
    names: Child<TextField>,
    comment: Child<TextField>,
    /// The row the fields were filled from, and as what.
    filled: Option<(usize, Host)>,
}

impl Edit {
    pub fn new(cx: &mut Cx<PanelView>) -> Edit {
        Edit { pending: None, list: ListModel::new(false), address: field(cx, ""), names: field(cx, "Separated by spaces"), comment: field(cx, "Optional"), filled: None }
    }

    fn host_in_fields(&self, on: bool, cx: &WindowCx) -> Host {
        Host {
            on,
            address: self.address.read(cx).text().trim().to_owned(),
            names: self.names.read(cx).text().split([' ', ',']).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect(),
            comment: self.comment.read(cx).text().trim().to_owned(),
        }
    }
}

pub fn ui(ui: &mut Ui, st: &mut State, extra: &Extra, busy: bool) {
    let (applied, text) = match &extra.hosts {
        None => return ui.note("Reading /etc/hosts…"),
        Some(Err(e)) => return ui.note(format!("/etc/hosts can't be read: {e}")),
        Some(Ok(h)) => h.clone(),
    };
    let e = &mut st.hosts;
    let mut hosts = e.pending.clone().unwrap_or_else(|| applied.clone());
    let selected = e.list.state().single().filter(|&i| i < hosts.len());
    // The fields edit the entry chosen: filled as it's chosen, read back
    // into it as they're typed in.
    match selected {
        Some(i) if e.filled.as_ref().map(|f| f.0) != Some(i) => {
            let h = hosts[i].clone();
            let (a, n, c) = (h.address.clone(), h.names.join(" "), h.comment.clone());
            e.address.update(ui.cx, |f, cx| f.set_text(a, cx));
            e.names.update(ui.cx, |f, cx| f.set_text(n, cx));
            e.comment.update(ui.cx, |f, cx| f.set_text(c, cx));
            e.filled = Some((i, h));
        }
        Some(i) if !hosts[i].system() => {
            let now = e.host_in_fields(hosts[i].on, ui.cx);
            if now != hosts[i] {
                hosts[i] = now.clone();
                e.pending = Some(hosts.clone());
                e.filled = Some((i, now));
            }
        }
        Some(_) => {}
        None => e.filled = None,
    }
    let editable = selected.is_some_and(|i| !hosts[i].system());
    for f in [&e.address, &e.names, &e.comment] {
        if f.read(ui.cx).enabled() != editable {
            f.update(ui.cx, |f, cx| f.set_enabled(editable, cx));
        }
    }
    let rows = hosts.clone();
    let columns = vec![Column::new("On", 30.0), Column::new("Address", 150.0), Column::new("Names", 190.0), Column::new("Comment", 150.0)];
    let list = ColumnListView::new(ui.cx, "hosts", &e.list, columns, rows.len(), move |_: &PanelView, cell, g| {
        let h = &rows[cell.row];
        // The machine's own, not to be edited: dim.
        if h.system() {
            cell.text_color = cell.text_color.opacity(0.55);
        }
        let color = cell.text_color;
        match cell.column {
            0 if h.on => check(g, cell.bounds, color),
            1 => cell.text(g, h.address.clone()),
            2 => cell.text(g, h.names.join(" ")),
            3 => cell.text(g, h.comment.clone()),
            _ => {}
        }
    })
    .empty_text("No entries.")
    .on_select(ui.cx.listener(|_: &mut PanelView, _: &(), cx| cx.notify()));
    ui.add(div().flex_1().min_h(px(m::LIST_ROW * 6.0 + 2.0)).child(list));

    // The entry chosen.
    let on = selected.map(|i| hosts[i].on).unwrap_or(false);
    let toggle = ui.on_view(move |v, now: &bool, cx| {
        let Some(st) = state_of(v) else { return };
        let Some(i) = selected else { return };
        let mut h = st.hosts.pending.clone().unwrap_or_else(|| match &st.link.shared.lock().unwrap().extra.hosts {
            Some(Ok((h, _))) => h.clone(),
            _ => Vec::new(),
        });
        if let Some(x) = h.get_mut(i) {
            x.on = *now;
            st.hosts.filled = Some((i, x.clone()));
        }
        st.hosts.pending = Some(h);
        cx.notify();
    });
    ui.add(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::SPACING * 2.0))
            .child(div().flex_1().child(TextControl::new(ui.cx, &e.address).label("Address:").divider(DIVIDER)))
            .child(CheckBox::new("On", on).enabled(editable).on_toggle(toggle)),
    );
    ui.add(TextControl::new(ui.cx, &e.names).label("Names:").divider(DIVIDER));
    ui.add(TextControl::new(ui.cx, &e.comment).label("Comment:").divider(DIVIDER));
    let invalid = hosts.iter().find_map(|h| h.check().err());
    if let Some(why) = &invalid {
        ui.label(format!("{why}."));
    }
    let changed = hosts != applied;
    let add = ui.on_click_view(move |v, cx| {
        let Some(st) = state_of(v) else { return };
        let mut h = st.hosts.pending.clone().unwrap_or_else(|| applied_of(st));
        h.push(Host { on: true, ..Host::default() });
        let at = h.len() - 1;
        st.hosts.pending = Some(h);
        st.hosts.filled = None;
        st.hosts.list.state_mut().select(at);
        st.hosts.address.focus(cx);
        cx.notify();
    });
    let remove = ui.on_click_view(move |v, cx| {
        let Some(st) = state_of(v) else { return };
        let Some(i) = selected else { return };
        let mut h = st.hosts.pending.clone().unwrap_or_else(|| applied_of(st));
        if i < h.len() && !h[i].system() {
            h.remove(i);
        }
        st.hosts.pending = Some(h);
        st.hosts.filled = None;
        st.hosts.list.state_mut().selected.clear();
        cx.notify();
    });
    let revert = ui.on_click_view(|v, cx| {
        if let Some(st) = state_of(v) {
            st.hosts.pending = None;
            st.hosts.filled = None;
        }
        cx.notify();
    });
    let pending = hosts.clone();
    let save = ui.on_click_view(move |v, cx| {
        let Some(st) = state_of(v) else { return };
        let new = HostsFile::parse(&text).with(&pending);
        if HostsFile::check(&new).is_ok() {
            let _ = st.link.tx.send(Request::Hosts { text: new, was: text.clone() });
            st.hosts.pending = None;
            st.hosts.filled = None;
        }
        cx.notify();
    });
    ui.add(
        div()
            .flex()
            .flex_row()
            .gap(px(m::SPACING))
            .child(Button::new("Add").enabled(!busy).on_click(add))
            .child(Button::new("Remove").enabled(editable && !busy).on_click(remove))
            .child(div().flex_1())
            .child(Button::new("Revert").enabled(changed && !busy).on_click(revert))
            .child(Button::new("Save").default(changed).enabled(changed && invalid.is_none() && !busy).on_click(save))
            .into_element(),
    );
}

/// The entries as last read.
fn applied_of(st: &State) -> Vec<Host> {
    match &st.link.shared.lock().unwrap().extra.hosts {
        Some(Ok((h, _))) => h.clone(),
        _ => Vec::new(),
    }
}

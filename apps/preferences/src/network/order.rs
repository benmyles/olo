//! Network ▸ Service Order, as a Mac's Set Service Order: the kinds of
//! link in the order traffic prefers them (Ethernet, then Wi-Fi, unless
//! changed), the one in use checked. Applied as each kind's route metric
//! by default in NetworkManager's configuration (`olo_system::netconf`,
//! written as root), so a network joined later, by anything, falls in
//! line; links connected take it at once, without disconnecting.

use olo_system::{
    netconf::{NetConf, Service},
    network::Status,
};
use olo_ui::{
    gpui::{Context, Entity, IntoElement, ParentElement as _, Styled as _, div, px},
    metrics as m,
    widgets::{Button, Column, ColumnListView, ListModel},
};

use super::{Extra, Request, State, check, state_of};
use crate::{PanelView, form::Ui};

pub struct Edit {
    /// The order as moved here, not yet applied.
    pending: Option<Vec<Service>>,
    list: Entity<ListModel>,
}

impl Edit {
    pub fn new(cx: &mut Context<PanelView>) -> Edit {
        Edit { pending: None, list: ListModel::new(cx, false) }
    }
}

/// Each service's state: whether a link of its kind is connected (with a
/// router: one traffic can go through), and whether there is one.
fn service_state(s: Service, status: &Status) -> (bool, &'static str) {
    let of_kind: Vec<_> = status.devices.iter().filter(|d| d.kind == s.kind()).collect();
    let up = of_kind.iter().any(|d| d.connected() && d.gateway.is_some());
    let text = if of_kind.is_empty() {
        "No adapter"
    } else if up {
        "Connected"
    } else if of_kind.iter().any(|d| d.connected()) {
        "Connected, no router"
    } else {
        "Not connected"
    };
    (up, text)
}

pub fn ui(ui: &mut Ui, st: &mut State, status: &Status, extra: &Extra, busy: bool) {
    let applied = extra.conf.order.clone();
    let order = st.order.pending.clone().unwrap_or_else(|| applied.clone());
    // In use: the first, as applied, that traffic can go through.
    let in_use = applied.iter().find(|s| service_state(**s, status).0).copied();
    ui.note("Traffic goes through the first service in this list that's connected; the others stand by. Networks joined later fall in line.");
    let rows = order.clone();
    let states: Vec<&'static str> = order.iter().map(|s| service_state(*s, status).1).collect();
    let columns = vec![Column::new("", 22.0), Column::new("Service", 200.0), Column::new("Status", 160.0)];
    let list = ColumnListView::new("service-order", &st.order.list, columns, rows.len(), move |cell, window, cx| {
        let s = rows[cell.row];
        match cell.column {
            0 if Some(s) == in_use => check(window, cell.bounds, cell.text_color),
            1 => cell.text(window, cx, s.name()),
            2 => cell.text(window, cx, states[cell.row]),
            _ => {}
        }
    })
    .on_select(|_, cx| cx.refresh_windows());
    ui.add(div().h(px(m::LIST_ROW * 4.0 + 2.0)).child(list));
    let selected = st.order.list.read(ui.cx).state.single().filter(|&i| i < order.len());
    let moved = |by: isize| -> Option<(Vec<Service>, usize)> {
        let from = selected?;
        let to = from.checked_add_signed(by).filter(|&to| to < order.len())?;
        let mut o = order.clone();
        o.swap(from, to);
        Some((o, to))
    };
    let set = |next: Option<(Vec<Service>, usize)>| {
        ui.on_click_view(move |v, _, cx| {
            let Some(st) = state_of(v) else { return };
            if let Some((o, to)) = next.clone() {
                st.order.pending = Some(o);
                st.order.list.update(cx, |l, _| l.state.select(to));
            }
            cx.notify();
        })
    };
    let (up, down) = (moved(-1), moved(1));
    let (can_up, can_down) = (up.is_some(), down.is_some());
    let (up, down) = (set(up), set(down));
    let changed = order != applied;
    let default = order == Service::ALL;
    let to_default = ui.on_click_view(|v, _, cx| {
        if let Some(st) = state_of(v) {
            st.order.pending = Some(Service::ALL.to_vec());
        }
        cx.notify();
    });
    let revert = ui.on_click_view(|v, _, cx| {
        if let Some(st) = state_of(v) {
            st.order.pending = None;
        }
        cx.notify();
    });
    let dns = extra.conf.dns.clone();
    let pending = order.clone();
    let apply = ui.on_click_view(move |v, _, cx| {
        let Some(st) = state_of(v) else { return };
        let _ = st.link.tx.send(Request::Conf(NetConf { order: pending.clone(), dns: dns.clone() }));
        st.order.pending = None;
        cx.notify();
    });
    ui.add(
        div()
            .flex()
            .flex_row()
            .gap(px(m::SPACING))
            .child(Button::new("Move Up").enabled(can_up && !busy).on_click(up))
            .child(Button::new("Move Down").enabled(can_down && !busy).on_click(down))
            .child(div().flex_1())
            .child(Button::new("Use the Usual Order").enabled(!default && !busy).on_click(to_default))
            .into_any_element(),
    );
    ui.add(
        div()
            .flex()
            .flex_row()
            .gap(px(m::SPACING))
            .child(div().flex_1())
            .child(Button::new("Revert").enabled(changed && !busy).on_click(revert))
            .child(Button::new("Apply").default(changed).enabled(changed && !busy).on_click(apply))
            .into_any_element(),
    );
    ui.note("Applying asks for an administrator's password once while this window is open.");
}

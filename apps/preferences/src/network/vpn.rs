//! Network ▸ VPN: the VPNs saved (WireGuard, and NetworkManager's VPN
//! plugins': OpenVPN, IPsec…), each Connected or not, brought up and
//! down; a WireGuard configuration (`wg-quick`'s `.conf`) imported
//! natively (`nm::import_wireguard`), and a VPN removed.

use olo_system::network::Vpn;
use olo_ui::{
    file_panel::{self, FilePanel, PanelResult},
    gpui::{Context, Entity, IntoElement, ParentElement as _, Styled as _, div, px},
    metrics as m,
    widgets::{Button, Column, ColumnListView, ListModel},
};

use super::{Extra, Request, State, check, state_of};
use crate::{PanelView, form::Ui};

pub struct Edit {
    list: Entity<ListModel>,
    /// The VPN chosen (its saved connection; the list reorders).
    selected: Option<String>,
}

impl Edit {
    pub fn new(cx: &mut Context<PanelView>) -> Edit {
        Edit { list: ListModel::new(cx, false), selected: None }
    }
}

pub fn ui(ui: &mut Ui, st: &mut State, extra: &Extra, busy: bool) {
    let vpns: Vec<Vpn> = extra.vpns.clone();
    let at = st.vpn.selected.as_ref().and_then(|p| vpns.iter().position(|v| v.profile == *p));
    st.vpn.list.update(ui.cx, |l, _| {
        l.state.selected.clear();
        if let Some(i) = at {
            l.state.select(i);
        }
    });
    let rows = vpns.clone();
    let columns = vec![Column::new("", 22.0), Column::new("Name", 240.0), Column::new("Kind", 120.0), Column::new("Status", 140.0)];
    let profiles: Vec<String> = vpns.iter().map(|v| v.profile.clone()).collect();
    let model = st.vpn.list.clone();
    let pick = ui.on_view(move |v, _: (), _, cx| {
        let row = model.read(cx).state.single();
        if let Some(st) = state_of(v) {
            st.vpn.selected = row.and_then(|i| profiles.get(i).cloned());
        }
        cx.notify();
    });
    let list = ColumnListView::new("vpns", &st.vpn.list, columns, rows.len(), move |cell, window, cx| {
        let v = &rows[cell.row];
        match cell.column {
            0 if v.state == 2 => check(window, cell.bounds, cell.text_color),
            1 => cell.text(window, cx, v.name.clone()),
            2 => cell.text(window, cx, v.kind.clone()),
            3 => cell.text(window, cx, v.state_text()),
            _ => {}
        }
    })
    .empty_text("No VPNs. Import a WireGuard configuration, or add one with its maker's app.")
    .on_select(move |window, cx| pick((), window, cx));
    ui.add(div().flex_1().min_h(px(m::LIST_ROW * 6.0 + 2.0)).child(list));
    let chosen = at.and_then(|i| vpns.get(i)).cloned();
    let link = st.link;
    let toggle = chosen.clone().map(|v| {
        let label = if v.active.is_some() { "Disconnect" } else { "Connect" };
        Button::new(label).default(true).enabled(!busy).on_click(move |_, _| {
            let _ = link.tx.send(match &v.active {
                Some(a) => Request::Deactivate(a.clone()),
                None => Request::VpnUp(v.profile.clone()),
            });
        })
    });
    // (Asked first: its keys go with it.)
    let remove = chosen.clone().map(|v| {
        Button::new("Remove").enabled(!busy && v.active.is_none()).on_click(move |_, cx| {
            let profile = v.profile.clone();
            let message = format!("Remove the VPN “{}”? Its settings and keys go with it.", v.name);
            olo_ui::widgets::open_alert(cx, "Network", "warning", &message, &["Cancel", "Remove"], move |choice, _| {
                if choice == Some(1) {
                    let _ = link.tx.send(Request::Delete(profile.clone()));
                }
            });
        })
    });
    let import = ui.on_click_view(move |_, _, cx| {
        let dir = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        file_panel::show(cx, "Import a WireGuard Configuration", FilePanel::open(dir).types(&["conf"]), move |a, _| {
            if let PanelResult::Chosen(path) = a.result {
                let _ = link.tx.send(Request::ImportWireguard(path));
            }
        });
    });
    let mut row = div().flex().flex_row().gap(px(m::SPACING)).child(Button::new("Import WireGuard…").enabled(!busy).on_click(import)).child(div().flex_1());
    if let Some(r) = remove {
        row = row.child(r);
    }
    if let Some(t) = toggle {
        row = row.child(t);
    }
    ui.add(row.into_any_element());
    ui.note("A WireGuard file (from the server, or wg-quick's) is kept with its keys; its DNS servers are asked alone while it's connected.");
}

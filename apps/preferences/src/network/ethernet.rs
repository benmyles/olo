//! Network ▸ Ethernet: each wired link (an adapter menu when there are
//! several): whether a cable's in and it's connected, Connect and
//! Disconnect, and its connection's IPv4 settings, from DHCP or its own
//! (address, router), with DNS servers of its own, applied at once
//! without disconnecting (`nm::set_ipv4_in`, then `Reapply`).

use olo_system::network::{self as nm, Device, Kind, Status};
use olo_ui::{
    gpui::{Context, Entity, IntoElement, ParentElement as _, SharedString, Styled as _, div, px},
    metrics as m,
    widgets::{Button, MenuField, StringView, TextControl, TextField},
};

use super::{Extra, Request, State, field, state_of};
use crate::{PanelView, form::Ui};

const DIVIDER: f32 = 130.0;

pub struct Edit {
    device: usize,
    manual: bool,
    address: Entity<TextField>,
    gateway: Entity<TextField>,
    dns: Entity<TextField>,
    /// What the fields were filled from (the link, its connection's
    /// settings): filled again as that changes, not as it's typed in.
    loaded: Option<(String, nm::Ipv4)>,
}

impl Edit {
    pub fn new(cx: &mut Context<PanelView>) -> Edit {
        Edit { device: 0, manual: false, address: field(cx, "192.168.1.20/24"), gateway: field(cx, "192.168.1.1"), dns: field(cx, "From the network"), loaded: None }
    }

    /// The settings as the fields have them (from DHCP, the address and
    /// router shown are DHCP's, not settings).
    fn edited(&self, cx: &olo_ui::gpui::App) -> nm::Ipv4 {
        let dns = self.dns.read(cx).text().split([',', ' ']).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect();
        if !self.manual {
            return nm::Ipv4 { manual: false, dns, ..nm::Ipv4::default() };
        }
        let address = self.address.read(cx).text().trim().to_owned();
        let address = if address.is_empty() || address.contains('/') { address } else { format!("{address}/24") };
        nm::Ipv4 { manual: true, address, gateway: self.gateway.read(cx).text().trim().to_owned(), dns }
    }
}

/// A link's state in words, as the tab heads it.
fn state_text(d: &Device) -> String {
    match d.state {
        20 => "No cable".into(),
        100 => {
            let name = d.connection.as_ref().map(|c| format!(" with “{c}”")).unwrap_or_default();
            let speed = d.speed.map(|s| format!(", {}", nm::speed_text(s))).unwrap_or_default();
            format!("Connected{name}{speed}")
        }
        _ => d.state_text().to_owned(),
    }
}

pub fn ui(ui: &mut Ui, st: &mut State, status: &Status, extra: &Extra, busy: bool) {
    let wired: Vec<Device> = status.devices.iter().filter(|d| d.kind == Kind::Ethernet).cloned().collect();
    if wired.is_empty() {
        ui.note("There's no Ethernet adapter. A USB or Thunderbolt one shows here once it's plugged in.");
        return;
    }
    let link = st.link;
    let e = &mut st.eth;
    e.device = e.device.min(wired.len() - 1);
    if wired.len() > 1 {
        let names: Vec<SharedString> = wired.iter().map(|d| SharedString::from(d.interface.clone())).collect();
        let pick = ui.on_view(|v, i: usize, _, cx| {
            if let Some(st) = state_of(v) {
                st.eth.device = i;
            }
            cx.notify();
        });
        ui.add(MenuField::new("ethernet-adapter", names, e.device).label("Adapter:").divider(DIVIDER).on_change(pick));
    }
    let d = wired[e.device].clone();
    let path = d.path.clone();
    let action = if d.connected() || matches!(d.state, 40..=90) {
        Button::new("Disconnect").enabled(!busy).on_click(move |_, _| {
            let _ = link.tx.send(Request::Disconnect(path.clone()));
        })
    } else {
        Button::new("Connect").enabled(!busy && d.state != 20).on_click(move |_, _| {
            let _ = link.tx.send(Request::Connect(path.clone()));
        })
    };
    ui.add(super::labelled(DIVIDER, "Status:", div().flex().flex_row().items_center().gap(px(m::SPACING)).child(div().flex_1().child(StringView::new(state_text(&d)))).child(action)));

    // The connection's IPv4 settings.
    let (profile, saved) = extra.ipv4.iter().find(|(p, ..)| *p == d.path).map(|(_, p, v)| (p.clone(), v.clone())).unwrap_or((None, nm::Ipv4::default()));
    let key = (d.path.clone(), saved.clone());
    if e.loaded.as_ref() != Some(&key) {
        // From DHCP: what it gave, to see (and start from, made one's own).
        let (address, gateway) = if saved.manual { (saved.address.clone(), saved.gateway.clone()) } else { (d.addresses.first().cloned().unwrap_or_default(), d.gateway.clone().unwrap_or_default()) };
        let dns = saved.dns.join(", ");
        e.manual = saved.manual;
        for (f, text) in [(&e.address, address), (&e.gateway, gateway), (&e.dns, dns)] {
            f.update(ui.cx, |f, cx| f.set_text(text, cx));
        }
        e.loaded = Some(key);
    }
    let manual = e.manual;
    for f in [&e.address, &e.gateway] {
        if f.read(ui.cx).enabled() != manual {
            f.update(ui.cx, |f, cx| f.set_enabled(manual, cx));
        }
    }
    let edited = e.edited(ui.cx);
    let valid = edited.check();
    let changed = profile.is_some() && edited != saved;
    let (address, gateway, dns) = (e.address.clone(), e.gateway.clone(), e.dns.clone());
    ui.bbox("IPv4", |ui| {
        let how: Vec<SharedString> = ["Using DHCP", "Manually"].into_iter().map(SharedString::from).collect();
        let choose = ui.on_view(|v, i: usize, _, cx| {
            if let Some(st) = state_of(v) {
                st.eth.manual = i == 1;
            }
            cx.notify();
        });
        ui.add(MenuField::new("ethernet-ipv4", how, usize::from(manual)).label("Configure IPv4:").divider(DIVIDER).enabled(profile.is_some()).on_change(choose));
        ui.add(TextControl::new(address).label("IP address:").divider(DIVIDER));
        ui.add(TextControl::new(gateway).label("Router:").divider(DIVIDER));
        ui.add(TextControl::new(dns).label("DNS servers:").divider(DIVIDER));
        ui.note(if profile.is_none() {
            "Connect it once to have settings of its own."
        } else {
            "Leave the DNS servers empty to use the network's. DNS servers set for every link (the DNS tab) take the place of these."
        });
        if let (true, Err(why)) = (changed, &valid) {
            ui.label(why.clone());
        }
        let revert = ui.on_click_view(|v, _, cx| {
            if let Some(st) = state_of(v) {
                st.eth.loaded = None;
            }
            cx.notify();
        });
        let (profile, device) = (profile.clone(), d.path.clone());
        let apply = ui.on_click_view(move |v, _, cx| {
            let Some(st) = state_of(v) else { return };
            let ipv4 = st.eth.edited(cx);
            if let (Some(profile), Ok(())) = (profile.clone(), ipv4.check()) {
                let _ = st.link.tx.send(Request::Ipv4 { device: device.clone(), profile, ipv4 });
            }
            cx.notify();
        });
        ui.add(
            div()
                .flex()
                .flex_row()
                .gap(px(m::SPACING))
                .child(div().flex_1())
                .child(Button::new("Revert").enabled(changed && !busy).on_click(revert))
                .child(Button::new("Apply").default(changed).enabled(changed && valid.is_ok() && !busy).on_click(apply))
                .into_any_element(),
        );
    });
    let none = "—".to_owned();
    let traffic = nm::traffic(&d.interface).map(|(r, s)| format!("{} received, {} sent", olo_ui::files::format_size(r), olo_ui::files::format_size(s))).unwrap_or_else(|| none.clone());
    super::rows(ui, DIVIDER, &[("Hardware address:", if d.hw_address.is_empty() { none.clone() } else { d.hw_address.clone() }), ("Interface:", d.interface.clone()), ("Traffic:", traffic)]);
}

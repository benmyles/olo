//! Network ▸ Ethernet: each wired link (an adapter menu when there are
//! several): whether a cable's in and it's connected, Connect and
//! Disconnect, and its connection's IPv4 settings, from DHCP or its own
//! (address, router), with DNS servers of its own, applied at once
//! without disconnecting (`nm::set_addressing_in`, then `Reapply`).

use olo_kit::{
    Child, Cx, IntoElement, ParentElement as _, SharedString, Styled as _, WindowCx, div, metrics as m, px,
    widgets::{Button, MenuField, StringView, TextControl, TextField},
};
use olo_system::network::{self as nm, Device, Kind, Status};

use super::{Extra, Request, State, field, state_of};
use crate::{PanelView, form::Ui};

// (On the 4-point grid: whole device pixels at every scale, so a menu's
// text and a field's under it line up at 1.25× too.)
const DIVIDER: f32 = 132.0;

pub struct Edit {
    device: usize,
    manual: bool,
    address: Child<TextField>,
    gateway: Child<TextField>,
    dns: Child<TextField>,
    ipv6: nm::Ipv6,
    mtu: Child<TextField>,
    /// What the fields were filled from (the link, its connection's
    /// settings): filled again as that changes, not as it's typed in.
    loaded: Option<(String, nm::Addressing)>,
}

impl Edit {
    pub fn new(cx: &mut Cx<PanelView>) -> Edit {
        Edit {
            device: 0,
            manual: false,
            address: field(cx, "192.168.1.20/24"),
            gateway: field(cx, "192.168.1.1"),
            dns: field(cx, "From the network"),
            ipv6: nm::Ipv6::Auto,
            mtu: field(cx, "Automatic"),
            loaded: None,
        }
    }

    /// The settings as the fields have them (from DHCP, the address and
    /// router shown are DHCP's, not settings); Err if the MTU isn't a
    /// number.
    fn edited(&self, cx: &WindowCx) -> Result<nm::Addressing, String> {
        let dns = self.dns.read(cx).text().split([',', ' ']).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect();
        let mtu = match self.mtu.read(cx).text().trim() {
            "" => 0,
            t => t.parse::<u32>().map_err(|_| "The MTU is a number of bytes.".to_owned())?,
        };
        if !self.manual {
            return Ok(nm::Addressing { manual: false, dns, ipv6: self.ipv6, mtu, ..nm::Addressing::default() });
        }
        let address = self.address.read(cx).text().trim().to_owned();
        let address = if address.is_empty() || address.contains('/') { address } else { format!("{address}/24") };
        Ok(nm::Addressing { manual: true, address, gateway: self.gateway.read(cx).text().trim().to_owned(), dns, ipv6: self.ipv6, mtu })
    }
}

/// The IPv6 menu's choices (Manually only when it is, set elsewhere).
fn ipv6_choices(now: nm::Ipv6) -> Vec<(nm::Ipv6, &'static str)> {
    let mut c = vec![(nm::Ipv6::Auto, "Automatically"), (nm::Ipv6::LinkLocal, "Link-Local Only"), (nm::Ipv6::Off, "Off")];
    if now == nm::Ipv6::Manual {
        c.push((nm::Ipv6::Manual, "Manually (Set Elsewhere)"));
    }
    c
}

/// A link's state in words, as the tab heads it (`behind`: a captive
/// portal wants a sign-in first).
fn state_text(d: &Device, behind: bool) -> String {
    match d.state {
        20 => "No cable".into(),
        100 => {
            let name = d.connection.as_ref().map(|c| format!(" with “{c}”")).unwrap_or_default();
            let speed = d.speed.map(|s| format!(", {}", nm::speed_text(s))).unwrap_or_default();
            let portal = if behind { format!(": {}", super::SIGN_IN_NEEDED) } else { String::new() };
            format!("Connected{name}{speed}{portal}")
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
        let pick = ui.on_view(|v, i: &usize, cx| {
            if let Some(st) = state_of(v) {
                st.eth.device = *i;
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
    let portal = status.portal.as_ref().filter(|p| p.device == d.path);
    ui.add(super::labelled(
        DIVIDER,
        "Status:",
        div().flex().flex_row().items_center().gap(px(m::SPACING)).child(div().flex_1().child(StringView::new(state_text(&d, portal.is_some())))).child(action),
    ));
    if let Some(p) = portal {
        ui.add(super::labelled(DIVIDER, "", super::portal_line(p)));
    }

    // The connection's IPv4 settings.
    let (profile, saved) = extra.ipv4.iter().find(|(p, ..)| *p == d.path).map(|(_, p, v)| (p.clone(), v.clone())).unwrap_or((None, nm::Addressing::default()));
    let key = (d.path.clone(), saved.clone());
    if e.loaded.as_ref() != Some(&key) {
        // From DHCP: what it gave, to see (and start from, made one's own).
        let (address, gateway) = if saved.manual { (saved.address.clone(), saved.gateway.clone()) } else { (d.addresses.first().cloned().unwrap_or_default(), d.gateway.clone().unwrap_or_default()) };
        let dns = saved.dns.join(", ");
        let mtu = if saved.mtu == 0 { String::new() } else { saved.mtu.to_string() };
        e.manual = saved.manual;
        e.ipv6 = saved.ipv6;
        for (f, text) in [(&e.address, address), (&e.gateway, gateway), (&e.dns, dns), (&e.mtu, mtu)] {
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
    let valid = edited.as_ref().map_err(String::clone).and_then(|a| a.check().map_err(|e| format!("{}{}.", e[..1].to_uppercase(), &e[1..])));
    let changed = (profile.is_some() && edited.as_ref().is_ok_and(|a| *a != saved)) || edited.is_err();
    let (address, gateway, dns, mtu, ipv6) = (&e.address, &e.gateway, &e.dns, &e.mtu, e.ipv6);
    ui.bbox("TCP/IP", |ui| {
        let how: Vec<SharedString> = ["Using DHCP", "Manually"].into_iter().map(SharedString::from).collect();
        let choose = ui.on_view(|v, i: &usize, cx| {
            if let Some(st) = state_of(v) {
                st.eth.manual = *i == 1;
            }
            cx.notify();
        });
        ui.add(MenuField::new("ethernet-ipv4", how, usize::from(manual)).label("Configure IPv4:").divider(DIVIDER).enabled(profile.is_some()).on_change(choose));
        ui.add(TextControl::new(ui.cx, address).label("IP address:").divider(DIVIDER));
        ui.add(TextControl::new(ui.cx, gateway).label("Router:").divider(DIVIDER));
        ui.add(TextControl::new(ui.cx, dns).label("DNS servers:").divider(DIVIDER));
        let choices = ipv6_choices(ipv6);
        let names: Vec<SharedString> = choices.iter().map(|(_, n)| SharedString::from(*n)).collect();
        let at = choices.iter().position(|(c, _)| *c == ipv6).unwrap_or(0);
        let pick6 = ui.on_view(move |v, i: &usize, cx| {
            if let (Some(st), Some((c, _))) = (state_of(v), choices.get(*i)) {
                st.eth.ipv6 = *c;
            }
            cx.notify();
        });
        ui.add(MenuField::new("ethernet-ipv6", names, at).label("Configure IPv6:").divider(DIVIDER).enabled(profile.is_some()).on_change(pick6));
        ui.add(TextControl::new(ui.cx, mtu).label("MTU:").divider(DIVIDER));
        ui.note(if profile.is_none() {
            "Connect it once to have settings of its own."
        } else {
            "Leave the DNS servers empty to use the network's. DNS servers set for every link (the DNS tab) take the place of these."
        });
        if let (true, Err(why)) = (changed, &valid) {
            ui.label(why.clone());
        }
        let revert = ui.on_click_view(|v, cx| {
            if let Some(st) = state_of(v) {
                st.eth.loaded = None;
            }
            cx.notify();
        });
        let (profile, device) = (profile.clone(), d.path.clone());
        let apply = ui.on_click_view(move |v, cx| {
            let Some(st) = state_of(v) else { return };
            if let (Some(profile), Ok(ipv4)) = (profile.clone(), st.eth.edited(cx))
                && ipv4.check().is_ok()
            {
                let _ = st.link.tx.send(Request::Addressing { device: device.clone(), profile, ipv4 });
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
                .into_element(),
        );
    });
    let none = "—".to_owned();
    let traffic = nm::traffic(&d.interface).map(|(r, s)| format!("{} received, {} sent", olo_kit::files::format_size(r), olo_kit::files::format_size(s))).unwrap_or_else(|| none.clone());
    super::rows(ui, DIVIDER, &[("Hardware address:", if d.hw_address.is_empty() { none.clone() } else { d.hw_address.clone() }), ("Interface:", d.interface.clone()), ("Traffic:", traffic)]);
}

//! Network ▸ DNS: DNS servers and search domains of one's own for every
//! link, in place of what each network hands out (NetworkManager's global
//! DNS, in `olo_system::netconf`'s file, written as root); a menu of the
//! well-known public resolvers to start from; what each link asks now.

use olo_system::{
    netconf::{Dns, NetConf, valid_domain, valid_server},
    network::Status,
};
use olo_ui::{
    gpui::{App, Context, Entity, IntoElement, ParentElement as _, SharedString, Styled as _, div, px},
    metrics as m,
    widgets::{Button, MenuField, TextControl, TextField},
};

use super::{Extra, Request, State, field, state_of};
use crate::{PanelView, form::Ui};

const DIVIDER: f32 = 120.0;

/// Public resolvers, each with its IPv4 and IPv6 addresses.
const RESOLVERS: &[(&str, &str)] = &[
    ("Cloudflare (1.1.1.1)", "1.1.1.1, 1.0.0.1, 2606:4700:4700::1111, 2606:4700:4700::1001"),
    ("Google (8.8.8.8)", "8.8.8.8, 8.8.4.4, 2001:4860:4860::8888, 2001:4860:4860::8844"),
    ("Quad9 (9.9.9.9)", "9.9.9.9, 149.112.112.112, 2620:fe::fe, 2620:fe::9"),
];

pub struct Edit {
    servers: Entity<TextField>,
    searches: Entity<TextField>,
    /// What the fields were filled from: filled again as it changes.
    loaded: Option<Dns>,
}

fn words(f: &Entity<TextField>, cx: &App) -> Vec<String> {
    f.read(cx).text().split([',', ' ', ';']).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect()
}

impl Edit {
    pub fn new(cx: &mut Context<PanelView>) -> Edit {
        Edit { servers: field(cx, "Each network's own"), searches: field(cx, "None, or corp.example"), loaded: None }
    }

    fn edited(&self, cx: &App) -> Dns {
        Dns { servers: words(&self.servers, cx), searches: words(&self.searches, cx) }
    }
}

pub fn ui(ui: &mut Ui, st: &mut State, status: &Status, extra: &Extra, busy: bool) {
    let applied = extra.conf.dns.clone();
    let e = &mut st.dns;
    if e.loaded.as_ref() != Some(&applied) {
        let (servers, searches) = (applied.servers.join(", "), applied.searches.join(", "));
        e.servers.update(ui.cx, |f, cx| f.set_text(servers, cx));
        e.searches.update(ui.cx, |f, cx| f.set_text(searches, cx));
        e.loaded = Some(applied.clone());
    }
    let edited = e.edited(ui.cx);
    // The resolver the servers are, if they're one.
    let mut names: Vec<SharedString> = vec!["Each Network's Own".into()];
    names.extend(RESOLVERS.iter().map(|(n, _)| SharedString::from(*n)));
    names.push("Servers of Your Own".into());
    let chosen = if edited.servers.is_empty() { 0 } else { RESOLVERS.iter().position(|(_, a)| words_of(a) == edited.servers).map_or(names.len() - 1, |i| i + 1) };
    let servers = e.servers.clone();
    let pick = move |i: usize, window: &mut olo_ui::gpui::Window, cx: &mut App| {
        let text = match i {
            0 => String::new(),
            i if i <= RESOLVERS.len() => RESOLVERS[i - 1].1.to_owned(),
            _ => {
                return servers.update(cx, |f, cx| {
                    f.focus(window);
                    f.select_all_text(cx);
                });
            }
        };
        servers.update(cx, |f, cx| f.set_text(text, cx));
    };
    ui.bbox("For Every Link", |ui| {
        ui.add(MenuField::new("dns-resolver", names, chosen).label("Use:").divider(DIVIDER).on_change(pick));
        ui.add(TextControl::new(st.dns.servers.clone()).label("DNS servers:").divider(DIVIDER));
        ui.add(TextControl::new(st.dns.searches.clone()).label("Search domains:").divider(DIVIDER));
        ui.note("Servers of your own are asked in place of what each network hands out, on every link. Search domains are tried after a short name: nas, then nas.corp.example.");
    });
    let bad = edited
        .servers
        .iter()
        .find(|s| !valid_server(s))
        .map(|s| format!("“{s}” isn't an IP address."))
        .or_else(|| edited.searches.iter().find(|d| !valid_domain(d)).map(|d| format!("“{d}” isn't a domain.")));
    if let Some(why) = &bad {
        ui.label(why.clone());
    }
    // What each link asks now.
    let asked: Vec<(String, String)> = status
        .devices
        .iter()
        .filter(|d| d.connected())
        .map(|d| (format!("{}:", d.interface), if applied.servers.is_empty() { if d.dns.is_empty() { "—".to_owned() } else { d.dns.join(", ") } } else { applied.servers.join(", ") }))
        .collect();
    if !asked.is_empty() {
        ui.label("Asked now:");
        let rows: Vec<(&str, String)> = asked.iter().map(|(a, b)| (a.as_str(), b.clone())).collect();
        super::rows(ui, DIVIDER, &rows);
    }
    let changed = edited != applied;
    let revert = ui.on_click_view(|v, _, cx| {
        if let Some(st) = state_of(v) {
            st.dns.loaded = None;
        }
        cx.notify();
    });
    let order = extra.conf.order.clone();
    let apply = ui.on_click_view(move |v, _, cx| {
        let Some(st) = state_of(v) else { return };
        let conf = NetConf { order: order.clone(), dns: st.dns.edited(cx) };
        if conf.check().is_ok() {
            let _ = st.link.tx.send(Request::Conf(conf));
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
            .child(Button::new("Apply").default(changed).enabled(changed && bad.is_none() && !busy).on_click(apply))
            .into_any_element(),
    );
}

fn words_of(s: &str) -> Vec<String> {
    s.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect()
}

//! Rex's agent notifications and the hosts whose sessions may ask this
//! computer to do things. The local server is always the first row.

use std::{collections::HashSet, rc::Rc};

use olo_kit::{
    ParentElement as _, SharedString, Styled as _, div, metrics as m, px, style, theme,
    widgets::{Button, Column, ColumnListView, ListModel, StringView},
    words,
};
use olo_protocol::{RexGrants, RexHost, RexSettings};
use serde::{Deserialize, Serialize};

use crate::{PanelView, form::Ui, panel, prefs};

#[derive(Clone, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Selection {
    #[default]
    #[serde(alias = "None")]
    Local,
    Host(String),
}

impl Selection {
    fn at(hosts: &[RexHost], row: usize) -> Self {
        if row == 0 { Self::Local } else { hosts.get(row - 1).map_or(Self::Local, |h| Self::Host(h.id.clone())) }
    }

    fn row(&self, hosts: &[RexHost]) -> Option<usize> {
        match self {
            Self::Local => Some(0),
            Self::Host(id) => hosts.iter().position(|h| h.id == *id).map(|i| i + 1),
        }
    }

    fn host_mut<'a>(&self, hosts: &'a mut [RexHost]) -> Option<&'a mut RexHost> {
        let Self::Host(id) = self else { return None };
        hosts.iter_mut().find(|h| h.id == *id)
    }

    fn grants_mut<'a>(&self, rex: &'a mut RexSettings) -> Option<&'a mut RexGrants> {
        match self {
            Self::Local => Some(&mut rex.local),
            Self::Host(id) => rex.hosts.iter_mut().find(|h| h.id == *id).map(|h| &mut h.grants),
        }
    }

    /// Keep the row in the same place chosen, or the one before it if
    /// this was the last. This computer stays even with no other hosts.
    fn remove(&mut self, hosts: &mut Vec<RexHost>) {
        if let Self::Host(id) = self
            && let Some(i) = hosts.iter().position(|h| h.id == *id)
        {
            hosts.remove(i);
            *self = Self::at(hosts, (i + 1).min(hosts.len()));
        }
    }
}

pub struct State {
    list: ListModel,
    pub selected: Selection,
    editing: Option<String>,
    focus_name: bool,
    edited_addresses: HashSet<String>,
}

impl Default for State {
    fn default() -> Self {
        Self { list: ListModel::new(false), selected: Selection::Local, editing: None, focus_name: false, edited_addresses: HashSet::new() }
    }
}

fn add_host(hosts: &mut Vec<RexHost>) -> Selection {
    let id = loop {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).expect("random host id");
        let candidate = format!("{:032x}", u128::from_be_bytes(bytes));
        if !hosts.iter().any(|h| h.id == candidate) {
            break candidate;
        }
    };
    hosts.push(RexHost { id: id.clone(), label: words::tr("New host").to_owned(), ..Default::default() });
    Selection::Host(id)
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    notifications(ui);
    hosts(ui, st);
}

fn notifications(ui: &mut Ui) {
    ui.bbox("Notifications", |ui| {
        ui.add(StringView::new("Tell me when an agent whose pane isn’t in front:").wrap(true));
        let checks = ui.collect(|ui| {
            ui.check("Is waiting for me", ui.s.rex.notify_blocked, |s, v| s.rex.notify_blocked = v);
            ui.check("Has finished", ui.s.rex.notify_done, |s, v| s.rex.notify_done = v);
            ui.check("Has failed", ui.s.rex.notify_failed, |s, v| s.rex.notify_failed = v);
        });
        ui.add(div().flex().flex_row().flex_wrap().items_center().gap_x(px(2.0 * m::STACK)).gap_y(px(m::TIGHT)).children(checks));
        ui.add(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(m::SPACING))
                .child(div().flex_1().min_w_0().child(StringView::new("Choose how they look and sound in Notifications.").dim(true).wrap(true)))
                .child(Button::new("Open Notifications").on_click(|_, _| prefs().open("notifications"))),
        );
    });
}

fn hosts(ui: &mut Ui, st: &mut State) {
    let rex = &ui.s.rex;
    // Follow the ID through reorders. If a selected host goes away
    // (Undo, Defaults or an edit elsewhere), select This computer.
    let chosen = st.selected.row(&rex.hosts).unwrap_or(0);
    if chosen == 0 {
        st.selected = Selection::Local;
    }
    if st.list.state().single() != Some(chosen) {
        st.list.state_mut().select(chosen);
        st.list.reveal(ui.cx, chosen);
    }
    let host = chosen.checked_sub(1).and_then(|i| rex.hosts.get(i));
    let changed_host = st.editing.as_deref() != host.map(|h| h.id.as_str());
    if changed_host {
        ui.clear_fields();
        st.editing = host.map(|h| h.id.clone());
    }

    let wide = f32::from(ui.cx.viewport_size().width) >= 720.0;
    ui.bbox("Hosts", |ui| {
        let picker = ui.collect(|ui| host_picker(ui, st, host.is_some(), wide));
        let details = ui.collect(|ui| {
            fields(ui, host, st, changed_host);
            grants(ui, &st.selected, host.map(|h| if h.label.is_empty() { h.endpoint.as_str() } else { h.label.as_str() }), host.map_or(rex.local, |h| h.grants));
        });
        let picker = div().flex().flex_col().gap(px(m::STACK)).children(picker);
        let details = div().flex().flex_col().gap(px(m::STACK)).children(details);
        if wide {
            ui.add(div().flex().flex_row().items_start().gap(px(m::SECTION)).child(picker.w(px(240.0)).flex_shrink_0()).child(details.flex_1().min_w_0()));
        } else {
            ui.add(picker);
            ui.add(details);
        }
    });
}

fn host_picker(ui: &mut Ui, st: &State, removable: bool, wide: bool) {
    host_list(ui, st, wide);
    let add = ui.on_click_view(|v, cx| {
        if let panel::State::Rex(st) = &mut v.state {
            prefs().edit_as("", words::tr("Add Host"), |s| st.selected = add_host(&mut s.rex.hosts));
            st.focus_name = true;
            cx.notify();
        }
    });
    let remove = ui.on_click_view(|v, cx| {
        if let panel::State::Rex(st) = &mut v.state {
            prefs().edit_as("", words::tr("Remove Host"), |s| st.selected.remove(&mut s.rex.hosts));
            cx.notify();
        }
    });
    ui.row(|ui| {
        ui.button("Add", add);
        ui.enabled_if(removable, |ui| ui.button("Remove", remove));
    });
}

fn host_list(ui: &mut Ui, st: &State, wide: bool) {
    let hosts = &ui.s.rex.hosts;
    // (A host by its name alone: its address is in the field beside the
    // list, whole. In a second column here every address was cut short,
    // "ssh://dev@build.…", which read as broken: both design reviews.)
    let rows: Rc<Vec<(SharedString, SharedString)>> = Rc::new(
        std::iter::once((words::of_text("This computer"), "".into()))
            .chain(hosts.iter().map(|h| {
                let name = if !h.label.is_empty() {
                    h.label.clone().into()
                } else if !h.endpoint.is_empty() {
                    h.endpoint.clone().into()
                } else {
                    words::of_text("New host")
                };
                (name, h.endpoint.clone().into())
            }))
            .collect(),
    );
    let ids: Vec<_> = hosts.iter().map(|h| h.id.clone()).collect();
    let (cells, names) = (rows.clone(), rows.clone());
    let list = ColumnListView::new(ui.cx, "rex-hosts", &st.list, vec![Column::new("Name", if wide { 198.0 } else { 216.0 })], rows.len(), move |_: &PanelView, cell, g| {
        cell.text(g, cells[cell.row].0.clone());
    })
    .no_header()
    .type_select(move |_, i| names[i].0.clone())
    .on_select(ui.on_view(move |v, _: &(), cx| {
        if let panel::State::Rex(st) = &mut v.state {
            st.selected = match st.list.state().single() {
                Some(0) => Selection::Local,
                Some(i) => ids.get(i - 1).map_or(Selection::Local, |id| Selection::Host(id.clone())),
                None => Selection::Local,
            };
            cx.notify();
        }
    }));
    // Whole rows inside the two-line rim, including at fractional scales.
    let lines = if wide { 16.0 } else { 4.0 };
    ui.add(div().h(px(m::LIST_ROW * lines + 2.0 * list_rim(ui))).child(list));
}

/// The inset from the host list’s outside edge to its first row.
fn list_rim(ui: &Ui) -> f32 {
    let scale = ui.cx.scale_factor();
    2.0 * style::line_pixels(scale) / scale
}

fn fields(ui: &mut Ui, host: Option<&RexHost>, st: &mut State, changed_host: bool) {
    const STARTUP: &str = "Connect when Rex starts";
    // The labels' own column, so that each field begins beside its
    // label. (It was as wide as the startup choice, to have the address's
    // note beside that choice begin under the address: the fields then
    // began 170 points from "Name:". The note has a row of its own now.)
    let divider = crate::form::divider_for(&["Name:", "Address:"]);
    let remote = host.is_some();
    let record = host.map_or("local", |h| h.id.as_str());
    let name_key = format!("rex-name-{record}");
    let address_key = format!("rex-address-{record}");
    let name_value = host.map_or_else(|| words::tr("This computer"), |h| h.label.as_str());
    // (This computer's isn't a missing value: said, in the disabled field.)
    let address = host.map_or_else(|| words::tr("On this computer"), |h| h.endpoint.as_str());
    let show_hint = host.is_some_and(|h| h.endpoint.contains(char::is_whitespace) || h.endpoint.is_empty() && st.edited_addresses.contains(&h.id));

    let fields = ui.collect(|ui| {
        ui.enabled_if(remote, |ui| {
            let selected = st.selected.clone();
            let name = ui.field(
                &name_key,
                name_value,
                |f| f.name("Name"),
                move |s, text| {
                    if let Some(h) = selected.host_mut(&mut s.rex.hosts) {
                        h.label = text;
                    }
                },
            );
            ui.add(ui.control_of(&name_key).label("Name:").divider(divider));
            if std::mem::take(&mut st.focus_name) {
                ui.cx.on_next_frame(move |cx| {
                    name.focus(cx);
                    name.update(cx, |f, cx| f.select_all_text(cx));
                });
            }
            let selected = st.selected.clone();
            let address_row = ui.collect(|ui| {
                ui.text(
                    &address_key,
                    address,
                    |f| if remote { f.name("Address").hint("ssh://you@host") } else { f.name("Address") },
                    |c| c.label("Address:").divider(divider),
                    move |s, text| {
                        if let Some(h) = selected.host_mut(&mut s.rex.hosts) {
                            h.endpoint = text;
                        }
                    },
                );
            });
            // The address's note, right under its field and beginning
            // where the field does; its row is always there, so that
            // nothing below moves when it shows.
            let hint = div().pl(px(divider)).h(px(m::LABEL_ROW)).child(StringView::new("Rex needs an address to connect.").dim(true).cut(true));
            ui.add(div().flex().flex_col().gap(px(m::TIGHT / 2.0)).children(address_row).child(if show_hint { hint } else { hint.invisible() }));
            if changed_host
                && let Some(h) = host
                && !st.edited_addresses.contains(&h.id)
            {
                let id = h.id.clone();
                ui.on_first_field_edit(&address_key, move |v| {
                    if let panel::State::Rex(st) = &mut v.state {
                        st.edited_addresses.insert(id);
                    }
                });
            }
            let startup = ui.collect(|ui| {
                let selected = st.selected.clone();
                ui.check(STARTUP, host.is_none_or(|h| h.connect_at_start), move |s, v| {
                    if let Some(h) = selected.host_mut(&mut s.rex.hosts) {
                        h.connect_at_start = v;
                    }
                });
            });
            ui.add(div().children(startup));
        });
    });
    ui.add(div().flex().flex_col().gap(px(m::TIGHT)).children(fields));
}

/// The most the caution under a host's permissions is wide: its
/// longest sentence in two lines of about a length.
const CAUTION_WIDTH: f32 = 320.0;

fn grants(ui: &mut Ui, selected: &Selection, name: Option<&str>, grants: RexGrants) {
    let local = *selected == Selection::Local;
    // (The heading names whose sessions: "Its sessions may:" pointed at
    // nothing in particular, and a note under the choices said again
    // what the heading and the ticks say. Both design reviews.)
    let heading: SharedString = match name.filter(|n| !n.trim().is_empty()) {
        _ if local => words::of_text("Sessions on this computer may:"),
        Some(name) => olo_kit::tr!("Sessions on {} may:", name).into(),
        None => words::of_text("Its sessions may:"),
    };
    ui.add(div().mt(px(m::TIGHT)).child(StringView::new(heading).height(m::LABEL_ROW)));
    let checks = ui.collect(|ui| {
        let mut check = |label, on, field: fn(&mut RexGrants) -> &mut bool| {
            let selected = selected.clone();
            ui.check(label, on, move |s, v| {
                if let Some(g) = selected.grants_mut(&mut s.rex) {
                    *field(g) = v;
                }
            });
        };
        check("Open links on this computer", grants.open_links, |g| &mut g.open_links);
        check(if local { "Reach their local servers" } else { "Reach its local servers" }, grants.forward_ports, |g| &mut g.forward_ports);
        check("Use Agent Screens", grants.agent_screens, |g| &mut g.agent_screens);
        check("Get items from Passwords", grants.passwords, |g| &mut g.passwords);
        // (Not this computer's own: they have Passwords' agent as every
        // program here has. Its room is kept there, after the others
        // and unseen, so that choosing a host moves nothing.)
        // ("to other hosts": under the host's own address, "Sign in
        // with your SSH keys" read as how Rex signs in to it.)
        if !local {
            check("Sign in to other hosts with your SSH keys", grants.ssh_keys, |g| &mut g.ssh_keys);
        }
        check("Read and set the clipboard", grants.clipboard, |g| &mut g.clipboard);
        if local {
            check("Sign in to other hosts with your SSH keys", false, |g| &mut g.ssh_keys);
        }
    });
    let rows = checks.len();
    let checks = checks.into_iter().enumerate().map(|(i, row)| if local && i + 1 == rows { div().invisible().child(row) } else { div().child(row) });
    ui.add(div().mt(px(m::TIGHT - m::STACK)).flex().flex_col().gap(px(m::STACK)).children(checks));
    // Reserve the same wrapped line(s) in every state. Hidden messages
    // take their room without being painted or appearing in the tree.
    let p = theme::palette();
    let ink = olo_theme::readable(p.warning, p.panel_background, 4.5);
    // (One determiner, plain nouns, in the rows' order: Opus's review of
    // "Give Passwords, your SSH keys and the clipboard".)
    // (Each in two lines of about a length, broken here: wrapped where
    // the width ran out, one left "this computer." alone under a long
    // line. Both reviews' third look.)
    let words = match (grants.passwords, grants.ssh_keys, grants.clipboard) {
        (true, false, false) => "Give your passwords only to a host\nyou trust as much as this computer.",
        (false, true, false) => "Give your SSH keys only to a host\nyou trust as much as this computer.",
        (false, false, true) => "Give your clipboard only to a host\nyou trust as much as this computer.",
        (true, true, false) => "Give your passwords and SSH keys only to\na host you trust as much as this computer.",
        (false, true, true) => "Give your SSH keys and clipboard only to\na host you trust as much as this computer.",
        (true, false, true) => "Give your passwords and clipboard only to\na host you trust as much as this computer.",
        _ => "Give your passwords, SSH keys and clipboard\nonly to a host you trust as much as this computer.",
    };
    // A note for the ticks as a group: from their own left edge, and
    // further under the last of them than they are from each other
    // (begun at the labels' edge and close under the last row it read
    // as that row's own, which with three things it may be about is
    // often the wrong one: both design reviews, 2026-11-23). Two lines'
    // room always, so that a tick never changes the window's height.
    // (No wider than the ticks' own words, in two lines of about a
    // length: at the fields' width it was a long strip with two words
    // under it. And the list beside is a row longer, so that Add and
    // Remove stand at the group's foot as the button of the group
    // above does. Both reviews' second look.)
    let caution = div().mt(px(m::STACK)).min_h(px(2.0 * m::LABEL_ROW)).max_w(px(CAUTION_WIDTH)).child(StringView::new(words).color(style::c(ink)).wrap(true));
    ui.add(if !local && (grants.passwords || grants.clipboard || grants.ssh_keys) { caution } else { caution.invisible() });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_host_has_its_own_id_even_after_removal() {
        let mut hosts = Vec::new();
        let first = add_host(&mut hosts);
        let second = add_host(&mut hosts);
        assert_ne!(first, second);
        assert_eq!(second.row(&hosts), Some(2));
        assert_eq!(hosts[1], RexHost { id: hosts[1].id.clone(), label: words::tr("New host").to_owned(), ..Default::default() });
        hosts.clear();
        let third = add_host(&mut hosts);
        assert_ne!(first, third);
        assert_ne!(second, third);
    }

    #[test]
    fn remove_selects_the_next_host_then_the_previous_then_this_computer() {
        let mut hosts = Vec::new();
        let first = add_host(&mut hosts);
        let mut chosen = add_host(&mut hosts);
        let last = add_host(&mut hosts);
        chosen.remove(&mut hosts);
        assert_eq!(chosen, last);
        chosen.remove(&mut hosts);
        assert_eq!(chosen, first);
        chosen.remove(&mut hosts);
        assert_eq!(chosen, Selection::Local);
        assert!(hosts.is_empty());
        let saved = add_host(&mut hosts);
        chosen.remove(&mut hosts);
        assert_eq!(serde_json::from_str::<Selection>(r#""None""#).unwrap(), Selection::Local);
        assert_eq!(saved.row(&hosts), Some(1));
        assert_eq!(chosen, Selection::Local);
    }

    #[test]
    fn grants_follow_the_host_id_and_a_missing_host_cannot_edit_local_grants() {
        let mut rex = RexSettings::default();
        let first = add_host(&mut rex.hosts);
        let second = add_host(&mut rex.hosts);
        rex.hosts.swap(0, 1);
        assert_eq!(first.row(&rex.hosts), Some(2));
        first.grants_mut(&mut rex).unwrap().clipboard = true;
        assert!(!second.grants_mut(&mut rex).unwrap().clipboard);
        rex.hosts.clear();
        assert!(first.grants_mut(&mut rex).is_none());
        assert_eq!(rex.local, RexGrants::all());
    }
}

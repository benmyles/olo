//! Deskbar: its live widgets (`olo_protocol::deskbar`; Ben, 2026-11-07:
//! "add a preferences app for the deskbar and in it support
//! configuring/adding live performance widgets into it e.g. cpu, gpu,
//! disk, network monitors etc. -- it should all be very customizable in
//! terms of where they appear on the deskbar, how they look, how often
//! they update, exactly what to show and so on"). The widgets in their
//! order beside the chosen one's settings: what it measures and which
//! (a GPU, a disk, an interface, a sensor), its own name, where, its
//! look and size, its words, how often it's read, how far back its graph
//! looks, its colour; an app's widget (Weather's: `olo_protocol::glance`),
//! its kind's options, where and how big. Each change shows in the
//! Deskbar at once.

use std::sync::{Arc, Mutex};

use olo_kit::{
    Cx, ParentElement as _, SharedString, Styled as _, WindowCx, div, metrics as m,
    palette::Hue,
    px,
    widgets::{BBox, Button, Column, ColumnListView, ListModel, MenuField},
};
use olo_protocol::{
    deskbar::{Look, Measure, Place, PoolView, Size, Widget},
    pool,
};
use olo_system::meters::{self, Sources};

use crate::{PanelView, form::Ui, panel};

pub struct State {
    list: ListModel,
    /// What there is to measure, read off the UI thread as the panel
    /// opens.
    sources: Arc<Mutex<Option<Sources>>>,
    /// The kind Add adds.
    add: usize,
}

impl State {
    pub fn new(_cx: &mut Cx<PanelView>) -> State {
        let sources = Arc::new(Mutex::new(None));
        let (found, proxy) = (sources.clone(), olo_kit::proxy());
        std::thread::spawn(move || {
            *found.lock().unwrap_or_else(|e| e.into_inner()) = Some(meters::sources());
            proxy.send(crate::Msg::Redraw);
        });
        State { list: ListModel::new(false), sources, add: 0 }
    }
}

/// For the lint: what there is to measure made up (`sources`, as long as
/// real machines' and pools' come), and widget `chosen` chosen, its
/// editor showing.
#[cfg(test)]
pub fn for_lint(st: &mut State, sources: Sources, chosen: usize) {
    // (A new one: the real machine's, read as the panel opened, go to the
    // old.)
    st.sources = Arc::new(Mutex::new(Some(sources)));
    st.list.state_mut().select(chosen);
}

fn state_of(v: &mut PanelView) -> Option<&mut State> {
    match &mut v.state {
        panel::State::Deskbar(st) => Some(st),
        _ => None,
    }
}

/// The choices a widget's interval and span are made from.
const EVERY: [(u32, &str); 8] =
    [(250, "¼ second"), (500, "½ second"), (1000, "1 second"), (2000, "2 seconds"), (5000, "5 seconds"), (10_000, "10 seconds"), (30_000, "30 seconds"), (60_000, "1 minute")];
const SPANS: [(u32, &str); 7] = [(30, "30 seconds"), (60, "1 minute"), (120, "2 minutes"), (300, "5 minutes"), (600, "10 minutes"), (1800, "30 minutes"), (3600, "1 hour")];

/// What a widget is, said in its list row: "CPU, a graph under the tray";
/// an app's, "Weather: Next Hours, the next 6 hours, chance of rain, under
/// the tray".
fn summary(w: &Widget, shares: bool, sources: &Sources) -> String {
    let app = matches!(w.measure, Measure::App { .. });
    let look = match w.look {
        _ if app => "",
        Look::Graph => "a graph",
        Look::Bars => "bars",
        Look::Meter => "a meter",
        Look::Number => "a number",
        Look::Heatmap => "a heatmap",
    };
    let place = match w.place {
        Place::Tray => "in the tray",
        Place::Top => "under the tray",
        Place::Foot => "at the bottom",
    };
    let beside = if shares { ", beside the one before" } else { "" };
    // (Its own name first: "Build box: CPU, a graph under the tray".)
    let named = w.own_name().map(|n| format!("{n}: ")).unwrap_or_default();
    // ("Divider: an etched line under the tray"; named, "Tools: a heading
    // under the tray". Short: how far it reaches is in its settings.)
    if let Measure::Divider { line, .. } = &w.measure {
        if !named.is_empty() {
            return format!("{named}a heading {place}");
        }
        let words = line.name();
        return format!("Divider: {}{} {place}", words[..1].to_lowercase(), &words[1..]);
    }
    if app {
        return format!("{named}{}, {place}{beside}", meters::describe(&w.measure, sources));
    }
    format!("{named}{}, {look} {place}{beside}", meters::describe(&w.measure, sources))
}

/// Whether the widget at `i` shares the row of the one before it: it
/// asks to, out of the tray, and one before it is in its place.
fn shares(widgets: &[Widget], i: usize) -> bool {
    let w = &widgets[i];
    w.beside && w.place != Place::Tray && widgets[..i].iter().any(|o| o.place == w.place)
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let widgets = ui.s.deskbar.widgets.clone();
    let sources = st.sources.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default();
    let selected = st.list.state().single().filter(|i| *i < widgets.len());
    let list_model = st.list.clone();
    let left = ui.collect(|ui| {
        let rows: Vec<String> = widgets.iter().enumerate().map(|(i, w)| summary(w, shares(&widgets, i), &sources)).collect();
        let n = rows.len();
        let list = ColumnListView::new(ui.cx, "deskbar-widgets", &list_model, vec![Column::new("Widget", 290.0)], n, move |_: &PanelView, cell, g| cell.text(g, rows[cell.row].clone()))
            .no_header()
            .empty_text("No widgets: add one below.")
            .on_select(ui.cx.listener(|_: &mut PanelView, _: &(), cx| cx.notify()));
        // Add: one of the chosen kind, after the chosen widget (else at
        // the end), chosen.
        let kinds: Vec<SharedString> = Measure::kinds().iter().map(|k| SharedString::from(k.kind_name())).collect();
        let pick = ui.on_view(|v, i: &usize, cx| {
            if let Some(st) = state_of(v) {
                st.add = *i;
            }
            cx.notify();
        });
        let add_kind = st.add;
        let (lm2, lm3, lm4) = (list_model.clone(), list_model.clone(), list_model.clone());
        let add = ui.edit_click(move |s| {
            let kind = Measure::kinds().into_iter().nth(add_kind).unwrap_or(Measure::Cpu { cores: false });
            let w = Widget::new(s.deskbar.next_id(), kind);
            let at = selected.map_or(s.deskbar.widgets.len(), |i| (i + 1).min(s.deskbar.widgets.len()));
            s.deskbar.widgets.insert(at, w);
        });
        let select_after_add = {
            let n = widgets.len();
            move |_cx: &mut WindowCx| {
                let at = selected.map_or(n, |i| i + 1);
                lm2.state_mut().select(at);
            }
        };
        let remove = ui.edit_click(move |s| {
            if let Some(i) = selected.filter(|i| *i < s.deskbar.widgets.len()) {
                s.deskbar.widgets.remove(i);
            }
        });
        let up = ui.edit_click(move |s| {
            if let Some(i) = selected.filter(|i| *i > 0 && *i < s.deskbar.widgets.len()) {
                s.deskbar.widgets.swap(i, i - 1);
            }
        });
        let down = ui.edit_click(move |s| {
            if let Some(i) = selected.filter(|i| i + 1 < s.deskbar.widgets.len()) {
                s.deskbar.widgets.swap(i, i + 1);
            }
        });
        let count = widgets.len();
        ui.add(
            BBox::new("Widgets")
                .child(div().h(px(220.0)).child(list))
                .child(div().flex().flex_row().gap(px(m::SPACING)).child(MenuField::new("deskbar-add-kind", kinds, add_kind).width(140.0).on_change(pick)).child(Button::new("Add").on_click(
                    move |window, cx| {
                        add(window, cx);
                        select_after_add(cx);
                    },
                )))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(m::SPACING))
                        .child(Button::new("Remove").enabled(selected.is_some()).on_click(move |window, cx| {
                            remove(window, cx);
                            lm3.state_mut().selected.clear();
                        }))
                        .child(Button::new("Move Up").enabled(selected.is_some_and(|i| i > 0)).on_click(move |window, cx| {
                            up(window, cx);
                            if let Some(i) = selected.filter(|i| *i > 0) {
                                lm4.state_mut().select(i - 1);
                            }
                        }))
                        .child(Button::new("Move Down").enabled(selected.is_some_and(|i| i + 1 < count)).on_click({
                            let lm = list_model.clone();
                            move |window, cx| {
                                down(window, cx);
                                if let Some(i) = selected.filter(|i| i + 1 < count) {
                                    lm.state_mut().select(i + 1);
                                }
                            }
                        })),
                ),
        );
        ui.note("Each shows in the Deskbar as it changes. Click one there for Activity Monitor (a Token Pool one, Token Pool; an app's, its app).");
        // Beside each application's name, one dot: blue for its
        // notifications (it drains as they fade); red in its place
        // while the app itself says something is unread or waiting.
        let on = ui.s.deskbar.unread_dots;
        ui.bbox("Applications", |ui| {
            ui.check("A red dot for an app with something unread in it", on, |s, v| s.deskbar.unread_dots = v);
            ui.note("In place of the blue dot of its notifications, for as long as the app says something is unread or waiting in it.");
        });
    });
    let right = ui.collect(|ui| match selected.and_then(|i| widgets.get(i)) {
        Some(w) => editor(ui, w, selected.is_some_and(|i| widgets[..i].iter().any(|o| o.place == w.place)), &sources),
        None => ui.bbox("Widget", |ui| ui.note("Choose a widget to change it, or add one.")),
    });
    ui.add_columns(left, right);
}

/// The chosen widget's settings, each changing it (by id) at once
/// (`after` one in its place: it can share that one's row).
fn editor(ui: &mut Ui, w: &Widget, after: bool, sources: &Sources) {
    let id = w.id;
    // A change to the widget `id`, whatever its place in the list now.
    fn change(id: u32, f: impl Fn(&mut Widget) + 'static) -> impl Fn(&mut olo_protocol::Settings) + 'static {
        move |s| {
            if let Some(w) = s.deskbar.get_mut(id) {
                f(w);
            }
        }
    }
    const DIVIDER: f32 = 104.0;
    ui.bbox("Widget", |ui| {
        // What it measures: its kind, and which of it.
        let kinds: Vec<(usize, &str)> = Measure::kinds().iter().enumerate().map(|(i, k)| (i, k.kind_name())).collect();
        let kind_now = Measure::kinds().iter().position(|k| k.same_kind(&w.measure)).unwrap_or(0);
        ui.choose(
            "widget-kind",
            &kinds,
            &kind_now,
            |f| f.label("Shows:").divider(DIVIDER),
            move |s, i| {
                if let Some(w) = s.deskbar.get_mut(id)
                    && let Some(k) = Measure::kinds().into_iter().nth(i)
                    && !k.same_kind(&w.measure)
                {
                    // (A divider in the look's own line, unless given a hue.)
                    w.hue = (!k.is_divider()).then(|| k.hue());
                    w.measure = k;
                }
            },
        );
        measure_options(ui, w, sources, DIVIDER);
        // Its own name (Ben, 2026-11-14: "let me provide a custom name for
        // the widget if i want to override/customize the name"), its name
        // by what it measures as the hint; emptied, that again.
        let field_id = format!("widget-name-{id}");
        let field = ui.field(
            &field_id,
            w.name.as_deref().unwrap_or(""),
            |f| f.name("Widget name"),
            move |s, v| {
                let v = v.trim().to_owned();
                change(id, move |w| w.name = (!v.is_empty()).then(|| v.clone()))(s)
            },
        );
        let hint = w.default_label();
        field.update(ui.cx, move |f, cx| f.set_hint(hint, cx));
        let name = ui.control_of(&field_id).label("Name:").divider(DIVIDER);
        ui.add(name);
        ui.choose("widget-place", &Place::ALL.map(|p| (p, p.name())), &w.place, |f| f.label("Where:").divider(DIVIDER), move |s, v| change(id, move |w| w.place = v)(s));
        // A divider: where, how much room, and its colour.
        if w.measure.is_divider() {
            ui.choose(
                "widget-size",
                &[(Size::Small, "A little room"), (Size::Medium, "Some room"), (Size::Large, "A lot of room")],
                &w.size,
                |f| f.label("Room:").divider(DIVIDER),
                move |s, v| change(id, move |w| w.size = v)(s),
            );
            let mut hues: Vec<(Option<Hue>, &str)> = vec![(None, "The look's own")];
            hues.extend(Hue::ALL.iter().map(|h| (Some(*h), h.name())));
            ui.choose("widget-hue", &hues, &w.hue, |f| f.label("Colour:").divider(DIVIDER), move |s, v| change(id, move |w| w.hue = v)(s));
            ui.note(if w.place == Place::Tray {
                "A divider shows among the widgets, not in the tray: choose where."
            } else {
                "Named, it's a heading: its name at the left, the line going on from it."
            });
            return;
        }
        // (An app's widget looks as its app made it: where and how big.)
        let app = matches!(w.measure, Measure::App { .. });
        if !app {
            ui.choose("widget-look", &Look::ALL.map(|l| (l, l.name())), &w.look, |f| f.label("Look:").divider(DIVIDER), move |s, v| change(id, move |w| w.look = v)(s));
        }
        let in_tray = w.place == Place::Tray;
        ui.enabled_if(!in_tray, |ui| {
            ui.choose("widget-size", &Size::ALL.map(|z| (z, z.name())), &w.size, |f| f.label("Size:").divider(DIVIDER), move |s, v| change(id, move |w| w.size = v)(s));
            if !app {
                ui.check("Show its name", w.label, move |s, v| change(id, move |w| w.label = v)(s));
                ui.check("Show its value", w.value, move |s, v| change(id, move |w| w.value = v)(s));
            }
        });
        // (Side by side, each small: its name over its drawing.)
        ui.enabled_if(!in_tray && after, |ui| {
            ui.check("Beside the widget before it", w.beside, move |s, v| change(id, move |w| w.beside = v)(s));
        });
        if in_tray {
            ui.note("In the tray it's an icon's size; its name and value are in its tip.");
        }
        if app {
            ui.note(if in_tray { "Its tip says the rest." } else { "Small: one line. Medium and Large: all it shows, Large's sky bigger." });
            return;
        }
        ui.choose("widget-every", &EVERY, &w.every_ms, |f| f.label("Update every:").divider(DIVIDER), move |s, v| change(id, move |w| w.every_ms = v)(s));
        ui.enabled_if(w.look == Look::Graph, |ui| {
            ui.choose("widget-span", &SPANS, &w.span_s, |f| f.label("Graph shows:").divider(DIVIDER), move |s, v| change(id, move |w| w.span_s = v)(s));
        });
        let mut hues: Vec<(Option<Hue>, &str)> = vec![(None, "Accent")];
        hues.extend(Hue::ALL.iter().map(|h| (Some(*h), h.name())));
        // (A Token Pool heatmap's cells are coloured by how used each
        // quota is, green to red: no colour of its own.)
        let by_use = w.look == Look::Heatmap && matches!(w.measure, Measure::Pool { .. });
        ui.enabled_if(!by_use, |ui| {
            ui.choose("widget-hue", &hues, &w.hue, |f| f.label("Colour:").divider(DIVIDER), move |s, v| change(id, move |w| w.hue = v)(s));
        });
        if by_use {
            ui.note("Coloured by use: green unused, red used up.");
        }
    });
}

/// The options of what `w` measures: which core, GPU, disk, volume,
/// interface or sensor, and which of its series.
fn measure_options(ui: &mut Ui, w: &Widget, sources: &Sources, divider: f32) {
    let id = w.id;
    let set = move |f: Box<dyn Fn(&mut Measure)>| {
        move |s: &mut olo_protocol::Settings| {
            if let Some(w) = s.deskbar.get_mut(id) {
                f(&mut w.measure);
            }
        }
    };
    // A menu of `list`'s (key, name), with `none` first as None.
    let pick = |ui: &mut Ui, mid: &str, label: &'static str, none: Option<&str>, list: &[(String, String)], now: &Option<String>, f: fn(&mut Measure, Option<String>)| {
        let mut choices: Vec<(Option<String>, &str)> = Vec::new();
        if let Some(n) = none {
            choices.push((None, n));
        }
        choices.extend(list.iter().map(|(k, n)| (Some(k.clone()), n.as_str())));
        ui.choose(
            mid,
            &choices,
            now,
            |m| m.label(label).divider(divider),
            move |s, v| {
                if let Some(w) = s.deskbar.get_mut(id) {
                    f(&mut w.measure, v);
                }
            },
        );
    };
    match &w.measure {
        Measure::Cpu { cores } => ui.check(format!("Each core ({})", sources.cores), *cores, move |s, v| set(Box::new(move |m| *m = Measure::Cpu { cores: v }))(s)),
        Measure::Memory { swap } => ui.check("Swap too", *swap, move |s, v| set(Box::new(move |m| *m = Measure::Memory { swap: v }))(s)),
        Measure::Gpu { pci } => pick(ui, "widget-gpu", "GPU:", Some("The one it started on"), &sources.gpus, pci, |m, v| *m = Measure::Gpu { pci: v }),
        Measure::Disk { device, read, write } => {
            pick(ui, "widget-disk", "Disk:", Some("Every disk"), &sources.disks, device, |m, v| {
                if let Measure::Disk { device, .. } = m {
                    *device = v;
                }
            });
            let (r, wr) = (*read, *write);
            ui.check("Reads", r, move |s, v| {
                set(Box::new(move |m| {
                    if let Measure::Disk { read, .. } = m {
                        *read = v
                    }
                }))(s)
            });
            ui.check("Writes", wr, move |s, v| {
                set(Box::new(move |m| {
                    if let Measure::Disk { write, .. } = m {
                        *write = v
                    }
                }))(s)
            });
        }
        Measure::Space { mount } => {
            let now = Some(mount.clone());
            pick(ui, "widget-volume", "Volume:", None, &sources.volumes, &now, |m, v| *m = Measure::Space { mount: v.unwrap_or_else(|| "/".into()) });
        }
        Measure::Network { interface, down, up } => {
            pick(ui, "widget-interface", "Interface:", Some("Every interface"), &sources.interfaces, interface, |m, v| {
                if let Measure::Network { interface, .. } = m {
                    *interface = v;
                }
            });
            let (d, u) = (*down, *up);
            ui.check("Down (received)", d, move |s, v| {
                set(Box::new(move |m| {
                    if let Measure::Network { down, .. } = m {
                        *down = v
                    }
                }))(s)
            });
            ui.check("Up (sent)", u, move |s, v| {
                set(Box::new(move |m| {
                    if let Measure::Network { up, .. } = m {
                        *up = v
                    }
                }))(s)
            });
        }
        Measure::Temperature { sensor } => pick(ui, "widget-sensor", "Sensor:", Some("The processor"), &sources.sensors, sensor, |m, v| *m = Measure::Temperature { sensor: v }),
        Measure::Power => ui.note(if sources.battery { "What it draws from the battery; on power, it says so." } else { "This computer has no battery." }),
        Measure::Pool { provider, quotas, view, used, count_off, warn, notify } => {
            pool_options(ui, id, provider, quotas, *view, [*used, *count_off, *warn, *notify], w.look == Look::Heatmap, sources, divider)
        }
        Measure::App { app, glance, options } => app_options(ui, id, app, glance, options, divider),
        Measure::Divider { line, reach } => {
            use olo_protocol::deskbar::{DividerLine, DividerReach};
            ui.choose("widget-line", &DividerLine::ALL.map(|l| (l, l.name())), line, |f| f.label("Line:").divider(divider), {
                let set = set.clone();
                move |s, v| {
                    set(Box::new(move |m| {
                        if let Measure::Divider { line, .. } = m {
                            *line = v
                        }
                    }))(s)
                }
            });
            ui.enabled_if(*line != DividerLine::None, |ui| {
                ui.choose(
                    "widget-reach",
                    &DividerReach::ALL.map(|r| (r, r.name())),
                    reach,
                    |f| f.label("Reaches:").divider(divider),
                    move |s, v| {
                        set(Box::new(move |m| {
                            if let Measure::Divider { reach, .. } = m {
                                *reach = v
                            }
                        }))(s)
                    },
                );
            });
        }
    }
}

/// An app widget's options, as its kind offers them (`olo_protocol::glance`):
/// a menu each.
fn app_options(ui: &mut Ui, id: u32, app: &str, glance: &str, options: &std::collections::BTreeMap<String, String>, divider: f32) {
    let Some(kind) = olo_protocol::glance::kind(app, glance) else { return };
    ui.note(kind.about);
    for opt in kind.options {
        let values: Vec<(String, &str)> = opt.values.iter().map(|(v, name)| ((*v).to_owned(), *name)).collect();
        let now = kind.value(options, opt.id).to_owned();
        let oid = opt.id;
        ui.choose(
            &format!("widget-{app}-{oid}"),
            &values,
            &now,
            |f| f.label(opt.label).divider(divider),
            move |s, v: String| {
                if let Some(w) = s.deskbar.get_mut(id)
                    && let Measure::App { options, .. } = &mut w.measure
                {
                    options.insert(oid.to_owned(), v);
                }
            },
        );
    }
}

/// A Token Pool widget's options: whose quotas, which (any of them:
/// Ben, 2026-11-13, "make the quota type a multi-select"), how they're
/// shown, and whether it warns and tells.
#[allow(clippy::too_many_arguments)]
fn pool_options(ui: &mut Ui, id: u32, provider: &str, chosen: &[String], view: PoolView, [used, count_off, warn, notify]: [bool; 4], by_use: bool, sources: &Sources, divider: f32) {
    // A change to the widget's Pool measure.
    fn set(id: u32, f: impl Fn(&mut Vec<String>, &mut String, &mut PoolView, &mut [&mut bool; 4]) + 'static) -> impl Fn(&mut olo_protocol::Settings) + 'static {
        move |s| {
            if let Some(w) = s.deskbar.get_mut(id)
                && let Measure::Pool { provider, quotas, view, used, count_off, warn, notify } = &mut w.measure
            {
                f(quotas, provider, view, &mut [used, count_off, warn, notify]);
            }
        }
    }
    let names: Vec<(String, String)> = pool::PROVIDERS.iter().map(|p| (p.to_string(), pool::provider_name(p))).collect();
    let providers: Vec<(String, &str)> = names.iter().map(|(p, n)| (p.clone(), n.as_str())).collect();
    ui.choose(
        "widget-pool-provider",
        &providers,
        &provider.to_owned(),
        |f| f.label("Provider:").divider(divider),
        move |s, v: String| {
            set(id, move |quotas, provider, _, _| {
                if *provider != v {
                    // (Another provider's quotas are its own.)
                    *provider = v.clone();
                    quotas.clear();
                }
            })(s)
        },
    );
    // Its quotas as the pool last said them, and those chosen even if the
    // pool doesn't say them now (not running, or no account has one): a
    // pop-up field of several, each checked or not.
    let mut offered: Vec<String> = sources.pool.iter().find(|(p, _)| p == provider).map(|(_, q)| q.clone()).unwrap_or_default();
    for q in chosen {
        if !offered.contains(q) {
            offered.push(q.clone());
        }
    }
    if !offered.is_empty() {
        // (One field however many: as many and as long as the pool says.)
        ui.choose_many(
            "widget-pool-quotas",
            &offered,
            chosen,
            "Whichever holds each back",
            |f| f.label("Quotas:").divider(divider),
            move |s, now| set(id, move |quotas, _, _, _| *quotas = now.clone())(s),
        );
    }
    ui.choose("widget-pool-view", &PoolView::ALL.map(|v| (v, v.name())), &view, |f| f.label("Accounts:").divider(divider), move |s, v| set(id, move |_, _, view, _| *view = v)(s));
    ui.check("Show what's used, not what's left", used, move |s, v| set(id, move |_, _, _, b| *b[0] = v)(s));
    ui.enabled_if(view == PoolView::Together, |ui| {
        ui.check("Count accounts turned off or signed out", count_off, move |s, v| set(id, move |_, _, _, b| *b[1] = v)(s));
    });
    // (A heatmap's cells are coloured by use already: `editor`.)
    ui.enabled_if(!by_use, |ui| {
        ui.check("Orange, then red, as it runs low", warn, move |s, v| set(id, move |_, _, _, b| *b[2] = v)(s));
    });
    ui.check("Tell me when it runs out, and when it's back", notify, move |s, v| set(id, move |_, _, _, b| *b[3] = v)(s));
    if sources.pool.is_empty() {
        ui.note("Token Pool isn't running: its quotas are listed once it is.");
    }
}

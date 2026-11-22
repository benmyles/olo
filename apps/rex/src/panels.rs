//! Native Olo secondary windows for Rex:
//! - Settings (`⌘,`, left category sidebar + aligned grid + Defaults/Revert/Close, `fits_content`)
//! - Hosts & Port Forwards (`⇧⌘H`, uniform controls, aligned columns, `fits_content`)
//! - Pair Device with QR Code (pixel-exact vector QR canvas)
//! - Rex Diagnostics (`⇧⌘,`, clean two-column facts grid, `fits_content`)
//! - Native Olo SSH Askpass & Host Key Trust dialog (`SshAskpass` impl)

use std::sync::Arc;

use olo_kit::{
    Bounds, Child, CursorStyle, Element, FontWeight, InteractiveElement as _, IntoElement as _, MouseButton, ParentElement as _, PathBuilder, Pixels, Point, SharedString, Styled as _, WindowCx,
    agent, canvas, div, metrics as m, point, px, size, style, theme,
    widgets::{BBox, Banner, Button, CheckBox, ColorControl, ColorWell, MenuField, Slider, StringView, TextControl, TextField, Tone, divider_for},
};
use olo_rex_client::{AgentStatus, SshAskpass};
use olo_rex_vt::Rgb;
use olo_sync::{Condvar, Mutex};
use zeroize::Zeroizing;

use crate::{
    hub::{HostConnState, RexHub},
    keymap::KeymapEngine,
    settings::{AnimationStyle, BarBehavior, CursorShapeSetting, CursorTrail, Density, HostBridgePolicy, OptionAsAlt, SchemeChoice, Settings, ToolbarLayout},
    themes::{self, hsla},
};

pub const SETTINGS_TABS: &[&str] = &["Appearance", "Themes", "Typography", "Cursor", "Keybindings", "Notifications", "Olo Bridge", "Server"];

pub const SETTINGS_WIDTH: f32 = 540.0;
const SIDEBAR_CAT_W: f32 = 144.0;
const POPUP_W: f32 = 148.0;

pub fn settings_window_min_width() -> f32 {
    SETTINGS_WIDTH
}

#[derive(Clone)]
pub struct SettingsPanelState {
    pub tab: usize,
    pub selected_bridge_host: String,
    pub recording_shortcut_for: Option<String>,
    pub ssh_agent_status: AgentStatus,
    pub opened_snapshot: Option<Settings>,
}

impl Default for SettingsPanelState {
    fn default() -> Self {
        Self { tab: 0, selected_bridge_host: "local".to_owned(), recording_shortcut_for: None, ssh_agent_status: AgentStatus::default(), opened_snapshot: None }
    }
}

pub type SettingsMutator = Arc<dyn Fn(Box<dyn FnOnce(&mut Settings) + Send>) + Send + Sync>;

fn update_host_policy(mutate: &SettingsMutator, host_key: String, is_local: bool, f: impl FnOnce(&mut HostBridgePolicy) + Send + 'static) {
    mutate(Box::new(move |s| {
        let mut p = s.policy_for_host(&host_key, is_local);
        f(&mut p);
        s.host_policies.insert(host_key, p);
    }));
}

fn paint_dot(g: &mut olo_kit::Painter, center: Point<Pixels>, radius: f32, color: olo_kit::Hsla) {
    let mut pb = PathBuilder::fill();
    let cx = f32::from(center.x);
    let cy = f32::from(center.y);
    let steps = 16;
    for i in 0..steps {
        let a = (i as f32 / steps as f32) * std::f32::consts::TAU;
        let pt = point(px(cx + radius * a.cos()), px(cy + radius * a.sin()));
        if i == 0 { pb.move_to(pt) } else { pb.line_to(pt) }
    }
    pb.close();
    if let Ok(path) = pb.build() {
        g.paint_path(path, color);
    }
}

/// Grid-aligned slider row: right-aligned `label` in the same label column (`div_x`)
/// as `MenuField`, track starting at `div_x` with uniform `track_w` and optional integer
/// tick marks beneath, and `value_text` cleanly to the right.
#[allow(clippy::too_many_arguments)]
fn slider_row(
    id: &'static str,
    label: &'static str,
    div_x: f32,
    value: f32,
    range: std::ops::RangeInclusive<f32>,
    step: f32,
    ticks: Option<usize>,
    value_text: String,
    track_w: f32,
    on_change: impl Fn(&f32, &mut WindowCx) + 'static,
) -> Element {
    let slider_col = if let Some(tick_count) = ticks.filter(|&n| n >= 2) {
        let tick_bar = canvas(move |g, b| {
            let p = theme::palette();
            let tick_ink = style::secondary_text(&p);
            let left = f32::from(b.left()) + 5.5;
            let span = (f32::from(b.size.width) - 11.0).max(1.0);
            for i in 0..tick_count {
                let x = left + span * (i as f32 / (tick_count - 1) as f32);
                style::vline(g, px(x), b.top(), b.top() + px(4.0), tick_ink);
            }
        })
        .paint_key(style::key(("slider-ticks", tick_count)))
        .w(px(track_w))
        .h(px(5.0));
        div().flex().flex_col().child(Slider::new(id, value, range).step(step).width(track_w).on_change(on_change)).child(tick_bar)
    } else {
        div().child(Slider::new(id, value, range).step(step).width(track_w).on_change(on_change))
    };

    div()
        .flex()
        .flex_row()
        .items_start()
        .w_full()
        .child(div().w(px(div_x)).h(px(20.0)).pr(px(m::TIGHT)).flex().flex_row().items_center().justify_start().flex_shrink_0().child(StringView::new(label)))
        .child(slider_col)
        .child(div().h(px(20.0)).pl(px(m::SPACING)).flex().flex_row().items_center().flex_shrink_0().child(StringView::new(value_text).dim(true)))
}

fn category_row(label: &'static str, selected: bool, on_click: impl Fn(&(), &mut WindowCx) + 'static) -> Element {
    let key = style::key((label, selected));
    canvas(move |g, b| {
        let p = theme::palette();
        if selected {
            style::fill_rect(g, b, style::c(p.list_selected_background));
        }
        let ink = style::c(if selected { p.list_selected_item_text } else { p.panel_text });
        style::label_fit(
            g,
            b.left() + px(10.0),
            b.center().y,
            style::Align::Left,
            label,
            style::PLAIN_SIZE,
            style::ui_font(if selected { FontWeight::BOLD } else { FontWeight::NORMAL }),
            ink,
            (b.size.width - px(16.0)).max(px(24.0)),
        );
        g.agent(agent::Node { role: "Tab", name: Some(label.into()), bounds: b, selected: Some(selected), enabled: true, clickable: true, ..Default::default() });
    })
    .paint_key(key)
    .w_full()
    .h(px(24.0))
    .cursor(CursorStyle::PointingHand)
    .on_mouse_down(MouseButton::Left, move |_, cx| {
        on_click(&(), cx);
        cx.stop_propagation();
    })
}

#[allow(clippy::too_many_arguments)]
pub fn render_settings_panel(
    cx: &mut WindowCx,
    settings: &Settings,
    state: &SettingsPanelState,
    hub: &Arc<RexHub>,
    keymap: &KeymapEngine,
    mut feedback: Option<Element>,
    qr: Option<Arc<qrcode::QrCode>>,
    on_tab: impl Fn(&usize, &mut WindowCx) + 'static,
    mutate: SettingsMutator,
    on_select_bridge_host: impl Fn(&String, &mut WindowCx) + 'static,
    on_record_shortcut: impl Fn(&Option<String>, &mut WindowCx) + 'static,
    on_reload_config: impl Fn(&(), &mut WindowCx) + 'static,
    on_import_theme: impl Fn(&(), &mut WindowCx) + 'static,
    on_install_shim: impl Fn(&(), &mut WindowCx) + 'static,
    on_install_skill: impl Fn(&(), &mut WindowCx) + 'static,
    on_toggle_tailscale: impl Fn(&crate::tailscale::Action, &mut WindowCx) + 'static,
    on_defaults: impl Fn(&(), &mut WindowCx) + 'static,
    on_revert: impl Fn(&(), &mut WindowCx) + 'static,
    on_close: impl Fn(&(), &mut WindowCx) + 'static,
) -> Element {
    let page: Element = match state.tab {
        0 => render_appearance_tab(settings, mutate),
        1 => render_themes_tab(cx, settings, mutate, on_import_theme),
        2 => render_typography_tab(settings, mutate),
        3 => render_cursor_tab(settings, mutate),
        4 => render_keybindings_tab(state, keymap, on_record_shortcut, on_reload_config),
        5 => render_notifications_tab(settings, mutate),
        6 => render_bridge_tab(settings, state, hub, mutate, on_select_bridge_host, on_install_shim, on_install_skill),
        _ => render_server_tab(settings, state, hub, mutate, on_toggle_tailscale, feedback.take(), qr),
    };

    let page = if let Some(feedback) = feedback { div().w_full().flex().flex_col().gap(px(m::SPACING)).child(page).child(feedback) } else { page };
    let on_tab_rc = std::rc::Rc::new(on_tab);
    let mut cat_list = div().w(px(SIDEBAR_CAT_W)).flex_shrink_0().flex().flex_col().gap(px(2.0)).p(px(4.0));
    for (idx, &name) in SETTINGS_TABS.iter().enumerate() {
        let cb = on_tab_rc.clone();
        cat_list = cat_list.child(category_row(name, state.tab == idx, move |_, cx| {
            cb(&idx, cx);
        }));
    }

    let cat_frame = div()
        .relative()
        .mt(px(7.0))
        .w(px(SIDEBAR_CAT_W))
        .flex_shrink_0()
        .child(
            canvas(|g, b| {
                let p = theme::palette();
                style::fill_rect(g, b, style::c(p.list_background));
                style::sunken(g, b, &p, false);
            })
            .paint_key(310)
            .absolute()
            .inset_0(),
        )
        .child(cat_list);

    let defaults_enabled = serde_json::to_string(settings).ok() != serde_json::to_string(&settings.preference_defaults()).ok();
    let revert_enabled = state.opened_snapshot.as_ref().is_some_and(|orig| serde_json::to_string(settings).ok() != serde_json::to_string(orig).ok());

    let footer_rule = canvas(|g, b| {
        let p = theme::palette();
        style::hline(g, b.left(), b.right(), b.top(), style::shade(&p, p.panel_background, 1.3));
    })
    .paint_key(311)
    .w_full()
    .h(px(1.0));

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(m::WINDOW_INSET))
        .p(px(m::WINDOW_INSET))
        .child(div().w_full().flex().flex_row().gap(px(m::WINDOW_INSET)).child(cat_frame).child(div().flex_1().min_w_0().child(page)))
        .child(footer_rule)
        .child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(m::SPACING))
                        .child(Button::new("Defaults").min_width(m::BUTTON_MIN_WIDTH).enabled(defaults_enabled).on_click(move |_, cx| on_defaults(&(), cx)))
                        .child(Button::new("Revert").min_width(m::BUTTON_MIN_WIDTH).enabled(revert_enabled).on_click(move |_, cx| on_revert(&(), cx))),
                )
                .child(Button::new("Close").min_width(m::BUTTON_MIN_WIDTH).default(true).on_click(move |_, cx| on_close(&(), cx))),
        )
}

fn render_appearance_tab(settings: &Settings, mutate: SettingsMutator) -> Element {
    let div_x = divider_for(&["Tab bar position:", "Tab bar visibility:", "Pane headers:", "Density:", "Animation style:", "Status icons per tab:", "Horizontal padding:"]);

    let layout_idx = match settings.toolbar_layout {
        ToolbarLayout::Above => 0,
        ToolbarLayout::Below => 1,
        ToolbarLayout::LeftSidebar => 2,
        ToolbarLayout::RightSidebar => 3,
    };
    let bar_idx = |b: BarBehavior| match b {
        BarBehavior::Always => 0,
        BarBehavior::Hover => 1,
        BarBehavior::Hidden => 2,
    };
    let bar_from = |i: usize| match i {
        1 => BarBehavior::Hover,
        2 => BarBehavior::Hidden,
        _ => BarBehavior::Always,
    };
    let density_idx = if settings.density == Density::Compact { 1 } else { 0 };
    let anim_idx = match settings.animation_style {
        AnimationStyle::Snappy => 0,
        AnimationStyle::Natural => 1,
        AnimationStyle::Off => 2,
    };

    let m1 = mutate.clone();
    let m2 = mutate.clone();
    let m3 = mutate.clone();
    let m4 = mutate.clone();
    let m5 = mutate.clone();
    let m6 = mutate.clone();
    let m7 = mutate.clone();
    let m8 = mutate.clone();
    let m9 = mutate.clone();
    let m10 = mutate.clone();
    let m11 = mutate.clone();
    let m12 = mutate.clone();
    let m13 = mutate;

    div()
        .flex()
        .flex_col()
        .gap(px(m::SECTION))
        .w_full()
        .child(
            BBox::new("Chrome & Tabs").child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(m::SPACING))
                    .child(
                        MenuField::new("toolbar-layout", vec!["Above".into(), "Below".into(), "Left Sidebar".into(), "Right Sidebar".into()], layout_idx)
                            .label("Tab bar position:")
                            .divider(div_x)
                            .width(div_x + POPUP_W)
                            .on_change(move |&i, _| {
                                m1(Box::new(move |s| {
                                    s.toolbar_layout = match i {
                                        1 => ToolbarLayout::Below,
                                        2 => ToolbarLayout::LeftSidebar,
                                        3 => ToolbarLayout::RightSidebar,
                                        _ => ToolbarLayout::Above,
                                    };
                                }));
                            }),
                    )
                    .child(
                        MenuField::new("toolbar-vis", vec!["Always".into(), "Show on Hover".into(), "Hidden".into()], bar_idx(settings.toolbar_behavior))
                            .label("Tab bar visibility:")
                            .divider(div_x)
                            .width(div_x + POPUP_W)
                            .on_change(move |&i, _| {
                                m2(Box::new(move |s| s.toolbar_behavior = bar_from(i)));
                            }),
                    )
                    .child(
                        MenuField::new("pane-headers", vec!["Always".into(), "Show on Hover".into(), "Hidden".into()], bar_idx(settings.pane_header_behavior))
                            .label("Pane headers:")
                            .divider(div_x)
                            .width(div_x + POPUP_W)
                            .on_change(move |&i, _| {
                                m3(Box::new(move |s| s.pane_header_behavior = bar_from(i)));
                            }),
                    )
                    .child(MenuField::new("density", vec!["Comfortable".into(), "Compact".into()], density_idx).label("Density:").divider(div_x).width(div_x + POPUP_W).on_change(move |&i, _| {
                        m4(Box::new(move |s| {
                            s.density = if i == 1 { Density::Compact } else { Density::Comfortable };
                        }));
                    }))
                    .child(MenuField::new("anim-style", vec!["Snappy".into(), "Natural".into(), "Off".into()], anim_idx).label("Animation style:").divider(div_x).width(div_x + POPUP_W).on_change(
                        move |&i, _| {
                            m5(Box::new(move |s| {
                                s.animation_style = match i {
                                    1 => AnimationStyle::Natural,
                                    2 => AnimationStyle::Off,
                                    _ => AnimationStyle::Snappy,
                                };
                            }));
                        },
                    ))
                    .child(div().pt(px(2.0)).child(slider_row(
                        "tab-icons",
                        "Status icons per tab:",
                        div_x,
                        settings.tab_icon_count as f32,
                        0.0..=5.0,
                        1.0,
                        Some(6),
                        format!("{}", settings.tab_icon_count),
                        POPUP_W,
                        move |&v, _| {
                            m6(Box::new(move |s| s.tab_icon_count = v.round().clamp(0.0, 5.0) as u8));
                        },
                    )))
                    .child(CheckBox::new("Expand tabs to fill available width", settings.tabs_fill_space).on_toggle(move |&on, _| m7(Box::new(move |s| s.tabs_fill_space = on)))),
            ),
        )
        .child(
            BBox::new("Panes & Focus").child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(m::TIGHT))
                    .child(CheckBox::new("Highlight active pane border", settings.active_pane_highlight).on_toggle(move |&on, _| m8(Box::new(move |s| s.active_pane_highlight = on))))
                    .child(CheckBox::new("Dim inactive panes", settings.fade_inactive_panes).on_toggle(move |&on, _| m9(Box::new(move |s| s.fade_inactive_panes = on))))
                    .child(CheckBox::new("Dim panes when window is inactive", settings.fade_inactive_windows).on_toggle(move |&on, _| m10(Box::new(move |s| s.fade_inactive_windows = on))))
                    .child(CheckBox::new("Focus follows mouse across split panes", settings.focus_follows_mouse).on_toggle(move |&on, _| m11(Box::new(move |s| s.focus_follows_mouse = on))))
                    .child(CheckBox::new("Automatically balance splits on open or close", settings.autobalance_splits).on_toggle(move |&on, _| m12(Box::new(move |s| s.autobalance_splits = on))))
                    .child(div().pt(px(2.0)).child(slider_row(
                        "pad-x",
                        "Horizontal padding:",
                        div_x,
                        settings.padding_x,
                        0.0..=28.0,
                        1.0,
                        None,
                        format!("{:.0} px", settings.padding_x),
                        POPUP_W,
                        move |&v, _| m13(Box::new(move |s| s.padding_x = v.round())),
                    ))),
            ),
        )
}

fn render_themes_tab(cx: &mut WindowCx, settings: &Settings, mutate: SettingsMutator, on_import_theme: impl Fn(&(), &mut WindowCx) + 'static) -> Element {
    let div_x = divider_for(&["Color scheme:", "Light theme:", "Dark theme:", "Minimum contrast:"]);
    let builtins = themes::builtin_themes();
    let mut names: Vec<SharedString> = vec!["Match Olo Look".into()];
    let mut ids: Vec<Option<String>> = vec![None];
    for (id, t) in &builtins {
        names.push(t.name.clone().into());
        ids.push(Some(id.clone()));
    }
    for custom in &settings.custom_themes {
        names.push(format!("{} (Imported)", custom.name).into());
        ids.push(Some(custom.name.clone()));
    }
    let light_idx = ids.iter().position(|id| id.as_deref() == settings.light_theme_id.as_deref()).unwrap_or(0);
    let dark_idx = ids.iter().position(|id| id.as_deref() == settings.dark_theme_id.as_deref()).unwrap_or(0);
    let scheme_idx = match settings.scheme_choice {
        SchemeChoice::Auto => 0,
        SchemeChoice::Light => 1,
        SchemeChoice::Dark => 2,
    };

    let active = themes::resolve_theme(settings, &theme::palette());
    let swatches_pal = active.palette.clone();
    let swatch_bg = active.background;
    let swatch_fg = active.foreground;
    let accent_rgb = settings.active_pane_highlight_color.unwrap_or(Rgb::new(56, 132, 240));

    let ids_l = ids.clone();
    let ids_d = ids;
    let m1 = mutate.clone();
    let m2 = mutate.clone();
    let m3 = mutate.clone();
    let m4 = mutate.clone();
    let m5 = mutate.clone();
    let m6 = mutate.clone();
    let m7 = mutate;

    div()
        .flex()
        .flex_col()
        .gap(px(m::SECTION))
        .w_full()
        .child(
            BBox::new("Terminal Theme").child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(m::SPACING))
                    .child(
                        MenuField::new("scheme-choice", vec!["Automatic (Follow Olo)".into(), "Light".into(), "Dark".into()], scheme_idx)
                            .label("Color scheme:")
                            .divider(div_x)
                            .width(div_x + 188.0)
                            .on_change(move |&i, _| {
                                m1(Box::new(move |s| {
                                    s.scheme_choice = match i {
                                        1 => SchemeChoice::Light,
                                        2 => SchemeChoice::Dark,
                                        _ => SchemeChoice::Auto,
                                    };
                                }));
                            }),
                    )
                    .child(MenuField::new("light-theme", names.clone(), light_idx).label("Light theme:").divider(div_x).width(div_x + 188.0).on_change(move |&i, _| {
                        let chosen = ids_l.get(i).cloned().flatten();
                        m2(Box::new(move |s| s.light_theme_id = chosen));
                    }))
                    .child(MenuField::new("dark-theme", names, dark_idx).label("Dark theme:").divider(div_x).width(div_x + 188.0).on_change(move |&i, _| {
                        let chosen = ids_d.get(i).cloned().flatten();
                        m3(Box::new(move |s| s.dark_theme_id = chosen));
                    }))
                    .child(
                        canvas(move |g, b| {
                            let p = theme::palette();
                            style::fill_rect(g, b, hsla(swatch_bg));
                            style::sunken(g, b, &p, false);
                            let inner = style::inset(g, b, 6.0);
                            style::label(
                                g,
                                inner.left(),
                                inner.top() + px(8.0),
                                style::Align::Left,
                                "ben@olo ~/rex % cargo test --release",
                                11.0,
                                style::mono_font(FontWeight::NORMAL),
                                hsla(swatch_fg),
                            );
                            let sw = 18.0f32;
                            let gap = 4.0f32;
                            for (i, c) in swatches_pal.iter().take(16).enumerate() {
                                let col = i % 8;
                                let row = i / 8;
                                let x = f32::from(inner.left()) + col as f32 * (sw + gap);
                                let y = f32::from(inner.top()) + 22.0 + row as f32 * (sw + gap);
                                style::fill_rect(g, Bounds::new(point(px(x), px(y)), size(px(sw), px(sw))), hsla(*c));
                            }
                        })
                        .paint_key(style::key((swatch_bg.r, swatch_bg.g, swatch_bg.b, swatch_fg.r)))
                        .w_full()
                        .h(px(74.0)),
                    )
                    .child(slider_row(
                        "min-contrast",
                        "Minimum contrast:",
                        div_x,
                        settings.minimum_contrast,
                        1.0..=7.0,
                        0.5,
                        None,
                        format!("{:.1}:1", settings.minimum_contrast),
                        POPUP_W,
                        move |&v, _| m4(Box::new(move |s| s.minimum_contrast = v)),
                    ))
                    .child(CheckBox::new("Generate perceptual CIELAB 256-color palette", settings.extended_palette).on_toggle(move |&on, _| m5(Box::new(move |s| s.extended_palette = on))))
                    .child(CheckBox::new("Lock theme colors (ignore program OSC 10/11)", settings.force_theme_palette).on_toggle(move |&on, _| m6(Box::new(move |s| s.force_theme_palette = on))))
                    .child(div().pt(px(2.0)).child(Button::new("Import Ghostty / .itermcolors…").on_click(move |_, cx| on_import_theme(&(), cx)))),
            ),
        )
        .child(
            BBox::new("Active Pane Accent Color").child(
                div().flex().flex_row().items_center().gap(px(m::SECTION)).child(ColorWell::new(hsla(accent_rgb), 28.0)).child(
                    ColorControl::new("rex-accent", [accent_rgb.r, accent_rgb.g, accent_rgb.b])
                        .ramp_width(172.0)
                        .on_change(move |&rgb, _| {
                            m7(Box::new(move |s| {
                                s.active_pane_highlight_color = Some(Rgb::new(rgb[0], rgb[1], rgb[2]));
                            }));
                        })
                        .build(cx),
                ),
            ),
        )
}

fn render_typography_tab(settings: &Settings, mutate: SettingsMutator) -> Element {
    let div_x = divider_for(&["Font size:", "Line height:", "Character spacing:", "Option / Alt key:"]);
    let opt_idx = match settings.option_as_alt {
        OptionAsAlt::Both => 0,
        OptionAsAlt::Left => 1,
        OptionAsAlt::Right => 2,
        OptionAsAlt::Off => 3,
    };
    let m1 = mutate.clone();
    let m2 = mutate.clone();
    let m3 = mutate.clone();
    let m4 = mutate.clone();
    let m5 = mutate.clone();
    let m6 = mutate.clone();
    let m7 = mutate.clone();
    let m8 = mutate.clone();
    let m9 = mutate;

    div()
        .flex()
        .flex_col()
        .gap(px(m::SECTION))
        .w_full()
        .child(
            BBox::new("Font & Metrics").child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(m::SPACING))
                    .child(slider_row("font-size", "Font size:", div_x, settings.font_size, 8.0..=28.0, 1.0, None, format!("{} pt", settings.font_size), POPUP_W, move |&v, _| {
                        m1(Box::new(move |s| s.font_size = v.round()))
                    }))
                    .child(slider_row("line-height", "Line height:", div_x, settings.line_height, 0.9..=1.8, 0.05, None, format!("{:.2}×", settings.line_height), POPUP_W, move |&v, _| {
                        m2(Box::new(move |s| s.line_height = v))
                    }))
                    .child(slider_row("char-spacing", "Character spacing:", div_x, settings.char_spacing, -1.0..=4.0, 0.5, None, format!("{:+.1} px", settings.char_spacing), POPUP_W, move |&v, _| {
                        m3(Box::new(move |s| s.char_spacing = v))
                    }))
                    .child(CheckBox::new("Enable programming ligatures", settings.ligatures).on_toggle(move |&on, _| m4(Box::new(move |s| s.ligatures = on))))
                    .child(CheckBox::new("Draw bold ANSI 0–7 text using bright palette", settings.bold_is_bright).on_toggle(move |&on, _| m5(Box::new(move |s| s.bold_is_bright = on)))),
            ),
        )
        .child(
            BBox::new("Input & Selection").child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(m::SPACING))
                    .child(CheckBox::new("Automatically copy text on selection", settings.copy_on_selection).on_toggle(move |&on, _| m6(Box::new(move |s| s.copy_on_selection = on))))
                    .child(CheckBox::new("Play subtle shine highlight when copying", settings.shine_on_copy).on_toggle(move |&on, _| m7(Box::new(move |s| s.shine_on_copy = on))))
                    .child(CheckBox::new("Keep viewport anchored to bottom during resize", settings.anchor_bottom).on_toggle(move |&on, _| m8(Box::new(move |s| s.anchor_bottom = on))))
                    .child(
                        MenuField::new("opt-alt", vec!["Both as Alt".into(), "Left as Alt".into(), "Right as Alt".into(), "Off".into()], opt_idx)
                            .label("Option / Alt key:")
                            .divider(div_x)
                            .width(div_x + POPUP_W)
                            .on_change(move |&i, _| {
                                m9(Box::new(move |s| {
                                    s.option_as_alt = match i {
                                        1 => OptionAsAlt::Left,
                                        2 => OptionAsAlt::Right,
                                        3 => OptionAsAlt::Off,
                                        _ => OptionAsAlt::Both,
                                    };
                                }));
                            }),
                    ),
            ),
        )
}

fn render_cursor_tab(settings: &Settings, mutate: SettingsMutator) -> Element {
    let div_x = divider_for(&["Cursor shape:", "Cursor trail:"]);
    let shape_idx = match settings.cursor_shape {
        CursorShapeSetting::Block => 0,
        CursorShapeSetting::Bar => 1,
        CursorShapeSetting::Underline => 2,
        CursorShapeSetting::BlockHollow => 3,
    };
    let trail_idx = match settings.cursor_trail {
        CursorTrail::Off => 0,
        CursorTrail::Warp => 1,
        CursorTrail::Comet => 2,
        CursorTrail::Blaze => 3,
        CursorTrail::Sweep => 4,
        CursorTrail::Tapered => 5,
        CursorTrail::Rainbow => 6,
        CursorTrail::Frozen => 7,
    };
    let m1 = mutate.clone();
    let m2 = mutate.clone();
    let m3 = mutate.clone();
    let m4 = mutate;

    BBox::new("Cursor Appearance")
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(m::SPACING))
                .child(
                    MenuField::new("cursor-shape", vec!["Block".into(), "Vertical Bar".into(), "Underline".into(), "Hollow Block".into()], shape_idx)
                        .label("Cursor shape:")
                        .divider(div_x)
                        .width(div_x + POPUP_W)
                        .on_change(move |&i, _| {
                            m1(Box::new(move |s| {
                                s.cursor_shape = match i {
                                    1 => CursorShapeSetting::Bar,
                                    2 => CursorShapeSetting::Underline,
                                    3 => CursorShapeSetting::BlockHollow,
                                    _ => CursorShapeSetting::Block,
                                };
                            }));
                        }),
                )
                .child(
                    MenuField::new("cursor-trail", vec!["Off".into(), "Warp".into(), "Comet".into(), "Blaze".into(), "Sweep".into(), "Tapered".into(), "Rainbow".into(), "Frozen".into()], trail_idx)
                        .label("Cursor trail:")
                        .divider(div_x)
                        .width(div_x + POPUP_W)
                        .on_change(move |&i, _| {
                            m4(Box::new(move |s| {
                                s.cursor_trail = match i {
                                    1 => CursorTrail::Warp,
                                    2 => CursorTrail::Comet,
                                    3 => CursorTrail::Blaze,
                                    4 => CursorTrail::Sweep,
                                    5 => CursorTrail::Tapered,
                                    6 => CursorTrail::Rainbow,
                                    7 => CursorTrail::Frozen,
                                    _ => CursorTrail::Off,
                                };
                            }));
                        }),
                )
                .child(CheckBox::new("Override program DECSCUSR cursor shapes", settings.force_cursor_style).on_toggle(move |&on, _| m2(Box::new(move |s| s.force_cursor_style = on))))
                .child(CheckBox::new("Blink cursor when pane is focused", settings.cursor_blink).on_toggle(move |&on, _| m3(Box::new(move |s| s.cursor_blink = on)))),
        )
        .into_element()
}

fn render_keybindings_tab(
    state: &SettingsPanelState,
    keymap: &KeymapEngine,
    on_record_shortcut: impl Fn(&Option<String>, &mut WindowCx) + 'static,
    on_reload_config: impl Fn(&(), &mut WindowCx) + 'static,
) -> Element {
    let on_record = std::rc::Rc::new(on_record_shortcut);
    let mut col = div().flex().flex_col().gap(px(m::SPACING)).w_full();

    if !keymap.diagnostics.is_empty() {
        let msg = keymap.diagnostics.iter().take(3).map(|d| format!("[{}] {}", d.severity, d.message)).collect::<Vec<_>>().join(" • ");
        col = col.child(Banner::new(Tone::Failure, "Keymap Configuration Issue", msg));
    }

    if let Some(act) = &state.recording_shortcut_for {
        col = col.child(StringView::new(format!("Press the new key combination for `{act}` (Esc to cancel)…")).bold(true).wrap(true));
    }

    let bindings = keymap.all_bindings();
    let mut rows = div().flex().flex_col().gap(px(4.0)).w_full();
    for b in bindings.iter().take(10) {
        let act_id = b.action.clone();
        let label = b.description.clone().unwrap_or_else(|| b.action.clone());
        let sc = b.sequence.format_human();
        let rec = on_record.clone();
        let is_rec = state.recording_shortcut_for.as_deref() == Some(act_id.as_str());
        rows = rows.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(m::SPACING))
                .child(div().flex_1().min_w_0().child(StringView::new(label).cut(true)))
                .child(StringView::new(sc).dim(true))
                .child(Button::new(if is_rec { "Listening…" } else { "Change…" }).small().on_click(move |_, cx| {
                    rec(&Some(act_id.clone()), cx);
                })),
        );
    }

    col.child(BBox::new("Active Shortcuts").child(rows)).child(
        div()
            .flex()
            .flex_row()
            .justify_between()
            .items_center()
            .child(StringView::new(format!("{} bindings synced with Rex server", bindings.len())).dim(true))
            .child(Button::new("Reload Config").on_click(move |_, cx| on_reload_config(&(), cx))),
    )
}

fn render_notifications_tab(settings: &Settings, mutate: SettingsMutator) -> Element {
    let m1 = mutate.clone();
    let m2 = mutate.clone();
    let m3 = mutate.clone();
    let m4 = mutate;

    div().flex().flex_col().gap(px(m::SECTION)).w_full().child(
        BBox::new("Agent & Status Alerts").child(
            div()
                .flex()
                .flex_col()
                .gap(px(m::SPACING))
                .child(
                    CheckBox::new("Notify when an agent is Blocked (awaiting approval)", settings.notifications.blocked.desktop_notification)
                        .on_toggle(move |&on, _| m1(Box::new(move |s| s.notifications.blocked.desktop_notification = on))),
                )
                .child(
                    CheckBox::new("Show red unread badge for Blocked agents", settings.notifications.blocked.badge).on_toggle(move |&on, _| m2(Box::new(move |s| s.notifications.blocked.badge = on))),
                )
                .child(
                    CheckBox::new("Notify when a command or agent reports an Error", settings.notifications.error.desktop_notification)
                        .on_toggle(move |&on, _| m3(Box::new(move |s| s.notifications.error.desktop_notification = on))),
                )
                .child(
                    CheckBox::new("Notify when a long-running task finishes (Done)", settings.notifications.done.desktop_notification)
                        .on_toggle(move |&on, _| m4(Box::new(move |s| s.notifications.done.desktop_notification = on))),
                ),
        ),
    )
}

fn render_bridge_tab(
    settings: &Settings,
    state: &SettingsPanelState,
    hub: &Arc<RexHub>,
    mutate: SettingsMutator,
    on_select_bridge_host: impl Fn(&String, &mut WindowCx) + 'static,
    on_install_shim: impl Fn(&(), &mut WindowCx) + 'static,
    on_install_skill: impl Fn(&(), &mut WindowCx) + 'static,
) -> Element {
    let hosts: Vec<(String, String, bool)> = {
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        st.host_order.iter().filter_map(|k| st.hosts.get(k).map(|h| (h.key.clone(), h.label.clone(), h.is_local))).collect()
    };
    let sel_idx = hosts.iter().position(|(k, _, _)| k == &state.selected_bridge_host).unwrap_or(0);
    let (host_key, _, is_local) = hosts.get(sel_idx).cloned().unwrap_or_else(|| ("local".to_owned(), "Local".to_owned(), true));
    let policy = settings.policy_for_host(&host_key, is_local);
    let host_names: Vec<SharedString> = hosts.iter().map(|(_, l, _)| l.clone().into()).collect();
    let host_keys: Vec<String> = hosts.iter().map(|(k, _, _)| k.clone()).collect();

    let (m1, hk1) = (mutate.clone(), host_key.clone());
    let (m2, hk2) = (mutate.clone(), host_key.clone());
    let (m3, hk3) = (mutate.clone(), host_key.clone());
    let (m4, hk4) = (mutate.clone(), host_key.clone());
    let (m5, hk5) = (mutate, host_key);

    div()
        .flex()
        .flex_col()
        .gap(px(m::SECTION))
        .w_full()
        .child(StringView::new("Zero-trust Olo Desktop Bridge: remote hosts default to OFF until approved per host.").dim(true).wrap(true))
        .child(MenuField::new("bridge-host", host_names, sel_idx).label("Host policy:").divider(divider_for(&["Host policy:"])).width(divider_for(&["Host policy:"]) + 180.0).on_change(
            move |&i, cx| {
                if let Some(k) = host_keys.get(i) {
                    on_select_bridge_host(k, cx);
                }
            },
        ))
        .child(
            BBox::new("Allowed Capabilities").child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(m::SPACING))
                    .child(
                        CheckBox::new("Allow xdg-open / olo.open to open URLs in Olo", policy.allow_open_url)
                            .on_toggle(move |&on, _| update_host_policy(&m1, hk1.clone(), is_local, move |p| p.allow_open_url = on)),
                    )
                    .child(
                        CheckBox::new("Auto-tunnel remote localhost URLs via Rex -L", policy.allow_localhost_autoforward)
                            .on_toggle(move |&on, _| update_host_policy(&m2, hk2.clone(), is_local, move |p| p.allow_localhost_autoforward = on)),
                    )
                    .child(
                        CheckBox::new("Allow remote agents to drive Olo Agent Screens", policy.allow_agent_screens)
                            .on_toggle(move |&on, _| update_host_policy(&m3, hk3.clone(), is_local, move |p| p.allow_agent_screens = on)),
                    )
                    .child(
                        CheckBox::new("Allow remote `op` to prompt local Olo Passwords", policy.allow_passwords_op)
                            .on_toggle(move |&on, _| update_host_policy(&m4, hk4.clone(), is_local, move |p| p.allow_passwords_op = on)),
                    )
                    .child(
                        CheckBox::new("Allow SSH Agent forwarding from Olo Passwords", policy.allow_ssh_agent)
                            .on_toggle(move |&on, _| update_host_policy(&m5, hk5.clone(), is_local, move |p| p.allow_ssh_agent = on)),
                    ),
            ),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .gap(px(m::SPACING))
                .child(Button::new("Install Bridge Shims").on_click(move |_, cx| on_install_shim(&(), cx)))
                .child(Button::new("Install Agent Skill").on_click(move |_, cx| on_install_skill(&(), cx))),
        )
}

fn render_server_tab(
    settings: &Settings,
    state: &SettingsPanelState,
    hub: &Arc<RexHub>,
    mutate: SettingsMutator,
    on_toggle_tailscale: impl Fn(&crate::tailscale::Action, &mut WindowCx) + 'static,
    feedback: Option<Element>,
    qr: Option<Arc<qrcode::QrCode>>,
) -> Element {
    let (ts_status, server_ver, connected, name) = {
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        let h = st.hosts.get("local");
        (
            h.and_then(|x| x.tailscale.clone()),
            h.and_then(|x| x.server_version.clone()),
            h.is_some_and(|x| x.conn_state == HostConnState::Connected),
            h.map_or_else(|| "Rex".into(), |x| x.label.clone()),
        )
    };
    let status = ts_status.as_ref();
    let ts_enabled = connected && status.is_some_and(|s| matches!(s.state.as_str(), "starting" | "needs_login" | "running" | "connected" | "needs_machine_auth" | "stopping"));
    let agent_line = if state.ssh_agent_status.reachable {
        format!("Connected ({} keys loaded) — {}", state.ssh_agent_status.key_count, state.ssh_agent_status.socket_path.as_ref().map(|p| p.display().to_string()).unwrap_or_default())
    } else {
        let msg = state.ssh_agent_status.error.as_deref().unwrap_or("No SSH agent socket found");
        if msg.ends_with('.') { msg.to_owned() } else { olo_kit::tr!("{}.", msg) }
    };
    let (version, api) = server_ver.map(|v| (olo_kit::tr!("Rex {}", v.version), Some(olo_kit::tr!("(API rev {})", v.api_revision)))).unwrap_or((name, None));
    let server = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(m::TIGHT))
        .child(StringView::new(olo_kit::tr!("Server: {}", version)).height(m::MARK_ROW))
        .children(api.map(|v| StringView::new(v).dim(true).height(m::MARK_ROW)));

    use crate::tailscale::Action;
    let on_action = std::rc::Rc::new(on_toggle_tailscale);
    let toggle = on_action.clone();
    let check =
        CheckBox::new("Reach this server over Tailscale", ts_enabled).enabled(connected && status.is_some()).on_toggle(move |on, cx| toggle(&if *on { Action::Enable } else { Action::Disable }, cx));
    let needs_login = connected && status.is_some_and(|s| s.state == "needs_login");
    let mut below = div().w_full().pl(px(m::MARK_SIZE + m::MARK_GAP)).flex().flex_col().items_start().gap(px(m::SPACING));
    if !connected || feedback.is_none() || needs_login {
        below = below.child(div().w_full().min_h(px(olo_kit::widgets::TextView::line_height())).child(StringView::new(crate::tailscale::state_line(status, connected)).wrap(true)));
    }
    let mut feedback = feedback.filter(|_| connected);
    if needs_login {
        if let Some(url) = status.and_then(crate::tailscale::login_link) {
            let open = on_action.clone();
            let page = url.to_owned();
            below = below
                .gap(px(m::BOX_INSET))
                .child(Button::new("Open Sign-In Page").on_click(move |_, cx| open(&Action::OpenLink(page.clone()), cx)))
                .children(feedback.take())
                .child(StringView::new("Or scan with your phone:").dim(true).height(m::LABEL_ROW))
                .children(qr.map(|qr| qr_code(url, qr, 144.0)));
        } else {
            below = below.child(Button::new("Sign In to Tailscale…").on_click(move |_, cx| on_action(&Action::Login, cx)));
        }
    }
    below = below.children(feedback);
    let group = div().w_full().flex().flex_col().items_start().gap(px(m::SPACING)).child(server).child(check).child(below);
    div()
        .flex()
        .flex_col()
        .gap(px(m::SECTION))
        .w_full()
        .child(BBox::new("Local Rex Server & Tailscale").child(group))
        .child(BBox::new("SSH Agent (Olo Passwords)").child(StringView::new(agent_line).dim(true)))
        .child(CheckBox::new("Enable Developer Mode (block IDs & protocol inspector)", settings.developer_mode).on_toggle(move |&on, _| mutate(Box::new(move |s| s.developer_mode = on))))
}

#[allow(clippy::too_many_arguments)]
pub fn render_hosts_panel(
    cx: &WindowCx,
    hub: &Arc<RexHub>,
    host_label_field: &Child<TextField>,
    host_addr_field: &Child<TextField>,
    fwd_port_field: &Child<TextField>,
    on_add_host: impl Fn(&(), &mut WindowCx) + 'static,
    on_reconnect_host: impl Fn(&String, &mut WindowCx) + 'static,
    on_remove_host: impl Fn(&String, &mut WindowCx) + 'static,
    on_forward_port: impl Fn(&(String, String, u16, bool), &mut WindowCx) + 'static,
    on_stop_forward: impl Fn(&String, &mut WindowCx) + 'static,
    on_add_manual_forward: impl Fn(&(), &mut WindowCx) + 'static,
    on_close: impl Fn(&(), &mut WindowCx) + 'static,
) -> Element {
    let on_recon = std::rc::Rc::new(on_reconnect_host);
    let on_rem = std::rc::Rc::new(on_remove_host);
    let on_fwd = std::rc::Rc::new(on_forward_port);
    let on_stop = std::rc::Rc::new(on_stop_forward);

    let can_add = !host_addr_field.read(cx).text().trim().is_empty();
    let can_fwd = fwd_port_field.read(cx).text().trim().parse::<u16>().is_ok_and(|p| p > 0);

    let (hosts, detected_ports, active_forwards) = {
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        let hosts: Vec<(String, String, String, bool, HostConnState)> = st
            .host_order
            .iter()
            .filter_map(|k| {
                let h = st.hosts.get(k)?;
                Some((h.key.clone(), h.label.clone(), h.endpoint_raw.clone(), h.is_local, h.conn_state.clone()))
            })
            .collect();
        let mut ports = Vec::new();
        for (bk, br) in &st.blocks {
            let g = br.lock().unwrap_or_else(|e| e.into_inner());
            let host_lbl = st.hosts.get(&bk.host_key).map(|h| h.label.clone()).unwrap_or_else(|| bk.host_key.clone());
            let proc_name = g.process.as_ref().and_then(|p| p.foreground.as_ref().or(p.child.as_ref())).map(|p| p.name.clone());
            for pe in &g.ports.ports {
                ports.push((bk.host_key.clone(), host_lbl.clone(), bk.session_id.clone(), pe.address.clone(), pe.port, proc_name.clone()));
            }
        }
        let forwards: Vec<_> = st
            .forwards
            .iter()
            .map(|f| {
                let lbl = st.hosts.get(&f.host_key).map(|h| h.label.clone()).unwrap_or_else(|| f.host_key.clone());
                (f.id.clone(), lbl, f.remote_host.clone(), f.remote_port, f.local_addr.port(), f.state.clone())
            })
            .collect();
        (hosts, ports, forwards)
    };

    let host_names_for_fwd: Vec<SharedString> = hosts.iter().map(|(_, lbl, _, _, _)| lbl.clone().into()).collect();

    let mut host_list = div().flex().flex_col().gap(px(m::SPACING)).w_full();
    for (key, label, ep, is_local, conn) in hosts {
        let conn_dot: [u8; 4] = match &conn {
            HostConnState::Connected => [56, 174, 92, 255],
            HostConnState::Connecting | HostConnState::Reconnecting { .. } => [232, 168, 36, 255],
            HostConnState::Disconnected | HostConnState::Error(_) => [210, 64, 64, 255],
        };
        let detail = match &conn {
            HostConnState::Connected => ep,
            HostConnState::Connecting => format!("{ep}  •  Connecting…"),
            HostConnState::Disconnected => format!("{ep}  •  Disconnected"),
            HostConnState::Reconnecting { attempt, .. } => format!("{ep}  •  Reconnecting (#{attempt})…"),
            HostConnState::Error(e) => format!("{ep}  •  {e}"),
        };
        let r_cb = on_recon.clone();
        let d_cb = on_rem.clone();
        let k1 = key.clone();
        let k2 = key;
        // Always place `Remove` to the left of `Reconnect`, with `Reconnect` pinned in the rightmost column
        // so `Local`'s `Reconnect` aligns directly above every remote host's `Reconnect` (never over `Remove`)!
        let mut row = div().flex().flex_row().items_center().gap(px(m::SPACING)).child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            canvas(move |g, b| {
                                paint_dot(g, b.center(), 3.8, style::c(conn_dot));
                            })
                            .paint_key(style::key(conn_dot))
                            .w(px(10.0))
                            .h(px(10.0))
                            .flex_shrink_0(),
                        )
                        .child(StringView::new(label).bold(true).cut(true)),
                )
                .child(div().pl(px(16.0)).child(StringView::new(detail).dim(true).cut(true))),
        );
        if !is_local {
            row = row.child(Button::new("Remove").on_click(move |_, cx| d_cb(&k2, cx)));
        }
        row = row.child(Button::new("Reconnect").on_click(move |_, cx| r_cb(&k1, cx)));
        host_list = host_list.child(row);
    }

    let mut port_list = div().flex().flex_col().gap(px(m::SPACING)).w_full();
    if detected_ports.is_empty() && active_forwards.is_empty() {
        port_list = port_list.child(StringView::new("No listening ports or active forwards.").dim(true));
    }
    for (hk, host_lbl, sid, addr, port, proc_name) in detected_ports.into_iter().take(8) {
        let cb = on_fwd.clone();
        let (hk_c, sid_c) = (hk.clone(), sid.clone());
        let endpoint_str = match proc_name {
            Some(p) if !p.is_empty() => format!("{addr}:{port} ({p})"),
            _ => format!("{addr}:{port}"),
        };
        port_list = port_list.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(m::SPACING))
                .child(div().flex_1().min_w_0().flex().flex_col().child(StringView::new(endpoint_str).bold(true).cut(true)).child(StringView::new(host_lbl).dim(true).cut(true)))
                .child(Button::new("Forward & Open").on_click(move |_, cx| {
                    cb(&(hk_c.clone(), sid_c.clone(), port, true), cx);
                })),
        );
    }
    for (fid, host_lbl, rhost, rport, lport, state) in active_forwards {
        let stop = on_stop.clone();
        let fid_c = fid.clone();
        // (A forward that couldn't be restored isn't running: its row says
        // so in its ink, and what is offered is to remove it, not to stop
        // it.)
        let failed = matches!(state, crate::hub::ForwardState::Failed(_));
        let status = match state {
            crate::hub::ForwardState::Active => None,
            crate::hub::ForwardState::Waiting => Some(StringView::new("Waiting to reconnect.").dim(true)),
            crate::hub::ForwardState::Failed(why) => Some(StringView::new(olo_kit::tr!("Couldn’t restore this forward: {}", why)).color(style::c(theme::palette().failure)).wrap(true)),
        };
        port_list = port_list.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(m::SPACING))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(StringView::new(format!("127.0.0.1:{lport}  →  {host_lbl} ({rhost}:{rport})")).bold(!failed).dim(failed).cut(true))
                        .children(status),
                )
                .child(Button::new(if failed { "Remove" } else { "Stop" }).on_click(move |_, cx| stop(&fid_c, cx))),
        );
    }

    div()
        .flex()
        .flex_col()
        .gap(px(m::SECTION))
        .p(px(m::WINDOW_INSET))
        .w_full()
        .child(BBox::new("Hosts").child(host_list))
        .child(
            BBox::new("Add Host").child(
                div()
                    .flex()
                    .flex_row()
                    .items_end()
                    .gap(px(m::SPACING))
                    .child(div().w(px(132.0)).flex_shrink_0().flex().flex_col().gap(px(3.0)).child(StringView::new("Label").dim(true)).child(TextControl::new(cx, host_label_field)))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(3.0)).child(StringView::new("Address").dim(true)).child(TextControl::new(cx, host_addr_field)))
                    .child(Button::new("Add & Connect").enabled(can_add).on_click(move |_, cx| on_add_host(&(), cx))),
            ),
        )
        .child(
            BBox::new("Ports").child(
                div().flex().flex_col().gap(px(m::SPACING)).child(port_list).child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(m::SPACING))
                        .child(div().w(px(84.0)).flex_shrink_0().child(TextControl::new(cx, fwd_port_field)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(MenuField::new("fwd-host", if host_names_for_fwd.is_empty() { vec!["Local".into()] } else { host_names_for_fwd }, 0).label("Host:").width(180.0)),
                        )
                        .child(Button::new("Forward Port").enabled(can_fwd).on_click(move |_, cx| on_add_manual_forward(&(), cx))),
                ),
            ),
        )
        .child(div().w_full().flex().flex_row().justify_end().child(Button::new("Close").min_width(m::BUTTON_MIN_WIDTH).default(true).on_click(move |_, cx| on_close(&(), cx))))
}

fn qr_code(url: &str, qr: Arc<qrcode::QrCode>, side: f32) -> Element {
    canvas(move |g, b| {
        let p = theme::palette();
        style::fill_rect(g, b, style::c([255, 255, 255, 255]));
        style::sunken(g, b, &p, false);

        let w = qr.width();
        let avail = f32::from(b.size.width).min(f32::from(b.size.height)) - 4.0;
        let scale = g.scale_factor();
        let cell = ((avail / (w + 8) as f32) * scale).floor().max(1.0) / scale;
        let total = cell * w as f32;
        let ox = f32::from(b.left()) + ((f32::from(b.size.width) - total) / 2.0 * scale).round() / scale;
        let oy = f32::from(b.top()) + ((f32::from(b.size.height) - total) / 2.0 * scale).round() / scale;
        let dark = style::c([0, 0, 0, 255]);
        for y in 0..w {
            for x in 0..w {
                if qr[(x, y)] == qrcode::Color::Dark {
                    g.paint_quad(olo_kit::fill(Bounds::new(point(px(ox + x as f32 * cell), px(oy + y as f32 * cell)), size(px(cell), px(cell))), dark));
                }
            }
        }
    })
    .paint_key(style::key(url))
    .w(px(side))
    .h(px(side))
}

pub fn render_qr_pairing_panel(
    url: Option<String>,
    qr: Option<Arc<qrcode::QrCode>>,
    on_copy_url: impl Fn(&String, &mut WindowCx) + 'static,
    on_close: impl Fn(&(), &mut WindowCx) + 'static,
) -> Element {
    let mut col = div().flex().flex_col().items_center().gap(px(m::SECTION)).p(px(m::WINDOW_INSET)).w_full();

    if let Some(u) = url {
        let u_copy = u.clone();
        col = col
            .child(StringView::new("Scan with Rex on iOS or another desktop to connect:").bold(true))
            .children(qr.map(|qr| qr_code(&u, qr, 220.0)))
            .child(StringView::new(u).dim(true).cut(true))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(m::SPACING))
                    .child(Button::new("Copy URL").on_click(move |_, cx| on_copy_url(&u_copy, cx)))
                    .child(Button::new("Done").default(true).on_click(move |_, cx| on_close(&(), cx))),
            );
    } else {
        col = col
            .child(StringView::new("Enable Tailscale in Rex Settings → Server to get a pairable HTTPS URL for mobile devices.").dim(true).wrap(true))
            .child(Button::new("Close").default(true).on_click(move |_, cx| on_close(&(), cx)));
    }
    col
}

fn build_info(version: &olo_rex_client::ServerVersion) -> String {
    let mut parts = Vec::new();
    if !version.platform.is_empty() {
        parts.push(version.platform.as_str());
    }
    if !version.build_date.is_empty() {
        parts.push(version.build_date.as_str());
    }
    if version.dirty {
        parts.push("modified source");
    }
    parts.join(" · ")
}

pub fn render_diagnostics_panel(
    hub: &Arc<RexHub>,
    on_copy_version: impl Fn(&String, &mut WindowCx) + 'static,
    on_copy_debug: impl Fn(&String, &mut WindowCx) + 'static,
    on_close: impl Fn(&(), &mut WindowCx) + 'static,
) -> Element {
    let (blocks_n, res_bytes, img_bytes) = hub.total_vt_memory();
    let (version_info, debug_info, rows): (String, String, Vec<(&'static str, String)>) = {
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        let local = st.hosts.get("local");
        let ver = local.and_then(|h| h.server_version.as_ref()).map(|v| format!("{} (commit {}, API rev {})", v.version, v.commit, v.api_revision)).unwrap_or_else(|| "Disconnected".to_owned());
        let status = local.and_then(|h| h.server_status.as_ref());
        let pid_val = status.map(|s| format!("PID {}", s.pid)).unwrap_or_else(|| "—".to_owned());
        let inst_val = status.map(|s| s.instance_id.clone()).unwrap_or_else(|| "—".to_owned());
        let sess_val = status.map(|s| format!("{} on this server", s.session_count)).unwrap_or_else(|| "0 on this server".to_owned());
        let hosts_val = format!("{} configured", st.hosts.len());
        let listeners = status.and_then(|s| s.listeners.as_ref()).map(|ls| ls.iter().map(|l| l.endpoint.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_else(|| "None".to_owned());
        let vt_val = format!("libghostty-vt ({blocks_n} live panes)");
        let mem_val = format!("{:.1} MiB of resident grid pages", res_bytes as f64 / (1024.0 * 1024.0));
        let img_val = if img_bytes == 0 { "none".to_owned() } else { format!("{:.1} MiB", img_bytes as f64 / (1024.0 * 1024.0)) };
        let mut ver_copy = format!("Olo Rex {}\nRex server {ver}", env!("CARGO_PKG_VERSION"));
        if let Some(version) = local.and_then(|h| h.server_version.as_ref()) {
            let build = build_info(version);
            if !build.is_empty() {
                ver_copy.push('\n');
                ver_copy.push_str(&build);
            }
        }
        let dbg_copy = format!(
            "{ver_copy}\nProcess: {pid_val}\nInstance: {inst_val}\nSessions: {sess_val}\nHosts: {hosts_val}\nListeners: {listeners}\nVT engine: {vt_val}\nMemory: {mem_val}\nKitty images: {img_val}"
        );
        (
            ver_copy,
            dbg_copy,
            vec![
                ("Server:", ver),
                ("Process:", pid_val),
                ("Instance:", inst_val),
                ("Sessions:", sess_val),
                ("Hosts:", hosts_val),
                ("Listeners:", listeners),
                ("VT engine:", vt_val),
                ("Memory:", mem_val),
                ("Kitty images:", img_val),
            ],
        )
    };

    let label_col_w = (divider_for(&["Server:", "Process:", "Instance:", "Sessions:", "Hosts:", "Listeners:", "VT engine:", "Memory:", "Kitty images:"]) - m::LABEL_GAP).max(64.0);
    let mut grid = div().flex().flex_col().gap(px(4.0)).w_full();
    for (label, value) in rows {
        grid = grid.child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(m::SPACING))
                .child(div().w(px(label_col_w)).flex_shrink_0().flex().flex_row().justify_end().child(StringView::new(label).dim(true)))
                .child(div().flex_1().min_w_0().child(StringView::new(value).cut(true))),
        );
    }

    div().flex().flex_col().gap(px(m::WINDOW_INSET)).p(px(m::WINDOW_INSET)).w_full().child(grid).child(
        div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(m::SPACING))
                    .child(Button::new("Copy Version Info").on_click(move |_, cx| on_copy_version(&version_info, cx)))
                    .child(Button::new("Copy Debug Info").on_click(move |_, cx| on_copy_debug(&debug_info, cx))),
            )
            .child(Button::new("Close").min_width(m::BUTTON_MIN_WIDTH).default(true).on_click(move |_, cx| on_close(&(), cx))),
    )
}

#[derive(Debug, Clone)]
pub enum AskpassRequest {
    ConfirmHostKey { host: String, port: u16, fingerprint: String, key_type: String, changed: bool },
    Secret { title: String, prompt: String, echo: bool },
}

#[derive(Debug, Clone)]
pub enum AskpassReply {
    Confirm(bool),
    Secret(Option<String>),
}

pub struct OloAskpass {
    pending: Mutex<Option<(AskpassRequest, Option<AskpassReply>)>>,
    cv: Condvar,
    on_request: Arc<dyn Fn() + Send + Sync>,
}

impl std::fmt::Debug for OloAskpass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OloAskpass").finish_non_exhaustive()
    }
}

impl OloAskpass {
    pub fn new(on_request: impl Fn() + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self { pending: Mutex::new(None), cv: Condvar::new(), on_request: Arc::new(on_request) })
    }

    pub fn current_request(&self) -> Option<AskpassRequest> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|(_, rep)| rep.is_none()).map(|(req, _)| req.clone())
    }

    pub fn answer(&self, reply: AskpassReply) {
        let mut g = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, slot)) = g.as_mut() {
            *slot = Some(reply);
            self.cv.notify_all();
        }
    }

    fn ask(&self, req: AskpassRequest) -> AskpassReply {
        let mut g = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        *g = Some((req, None));
        drop(g);
        (self.on_request)();
        let mut g = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some((_, Some(rep))) = g.as_ref() {
                let out = rep.clone();
                *g = None;
                return out;
            }
            g = self.cv.wait(g).unwrap_or_else(|e| e.into_inner());
        }
    }
}

impl SshAskpass for OloAskpass {
    fn confirm_host_key(&self, host: &str, port: u16, fingerprint: &str, key_type: &str, changed: bool) -> bool {
        match self.ask(AskpassRequest::ConfirmHostKey { host: host.to_owned(), port, fingerprint: fingerprint.to_owned(), key_type: key_type.to_owned(), changed }) {
            AskpassReply::Confirm(ok) => ok,
            _ => false,
        }
    }

    fn prompt_secret(&self, title: &str, prompt: &str, echo: bool) -> Option<Zeroizing<String>> {
        match self.ask(AskpassRequest::Secret { title: title.to_owned(), prompt: prompt.to_owned(), echo }) {
            AskpassReply::Secret(Some(s)) => Some(Zeroizing::new(s)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn diagnostics_use_the_real_build_fields() {
        let v = serde_json::from_value(serde_json::json!({"rex_version":"1.2","platform":"linux_amd64","built_at":"fixture-date","dirty":true})).unwrap();
        assert_eq!(super::build_info(&v), "linux_amd64 · fixture-date · modified source");
    }
}

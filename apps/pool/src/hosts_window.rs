//! The Hosts window (Pool ▸ Hosts): the Rex hosts that can have a Token
//! Pool of their own (those reached over SSH), and how each one's
//! stands. A pool there has this pool's accounts and goes on with this
//! computer off (`olo_token_pool::hosts`): Install puts it there, Sync
//! Now carries the accounts at once (they are carried by themselves
//! whenever they change), Remove takes it away, its sign-ins taken back
//! first. Each says whether it is in step, behind (since when, and
//! why), and what the pool there last said of itself: its accounts,
//! the day's requests, what it is answering now, its latest requests.
//! While the window is open each is asked again every half minute.
//! Nothing here waits on a host: what is asked runs on the gateway's
//! threads, and the window is told when a host stands differently.

use std::{sync::Arc, time::Duration};

use olo_kit::{
    ClickEvent, Cx, Element, IntoElement as _, ParentElement as _, ScrollHandle, Styled as _, View, div, metrics as m, px, style, theme,
    widgets::{Button, LIGHT, Light, ScrollView, StringView, busy_of, button_width, light},
};
use olo_token_pool::{
    gateway::Shared,
    hosts::{Hosts, Stands},
    model::now,
};
use serde_json::Value;

use crate::ui::{Msg, Which};

/// A line of detail under a host's name: its pitch, and how wide a
/// request's first column is (when; the others follow the buttons').
const DETAIL: f32 = 16.0;
const AT: f32 = 40.0;

/// How far under the well's top its words stand where there are no
/// hosts: two fifths down a well of the window's usual height.
const EMPTY_TOP: f32 = 130.0;

/// How often the hosts' pools are asked while the window is open.
const EVERY: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub enum HostsMsg {
    /// A host stands differently.
    Changed,
    /// Time passed ("2 minutes ago"), and the pools are asked again.
    Tick,
    /// A button: do this to that host.
    Do(String, &'static str),
    /// Remove… pressed for a host (None: its question answered no).
    Ask(Option<String>),
}

pub struct HostsWindow {
    shared: Arc<Shared>,
    hosts: Arc<Hosts>,
    /// The host whose removal is being asked.
    asking: Option<String>,
    /// What couldn't be begun, and for which host.
    refused: Option<(String, String)>,
    scroll: ScrollHandle,
}

/// "just now", "2 minutes ago", "3 hours ago", else the day and time.
fn ago(at: i64) -> String {
    match now() - at {
        s if s < 60 => olo_kit::words::tr("just now").to_string(),
        s if s < 120 => olo_kit::words::tr("a minute ago").to_string(),
        s if s < 3600 => olo_kit::tr!("{} minutes ago", s / 60).to_string(),
        s if s < 7200 => olo_kit::words::tr("an hour ago").to_string(),
        s if s < 86_400 => olo_kit::tr!("{} hours ago", s / 3600).to_string(),
        _ => crate::clock(at),
    }
}

/// How a host's pool stands, in a light and a few words; and whether
/// what it last couldn't do is said under it.
pub fn stands(s: &Stands) -> (Option<Light>, String) {
    if let Some(doing) = &s.doing {
        return (None, format!("{}…", olo_kit::words::of(doing.clone())));
    }
    match (s.wanted, s.in_step, &s.error, s.behind_since) {
        (false, _, Some(_), _) => (Some(Light::Down), olo_kit::words::tr("Couldn’t install").into()),
        (false, _, None, _) => (Some(Light::Off), olo_kit::words::tr("No pool").into()),
        (true, true, _, _) => (Some(Light::On), olo_kit::words::tr("Up to date").into()),
        (true, false, _, Some(since)) => (Some(Light::Waiting), olo_kit::tr!("Behind since {}", crate::clock(since)).into()),
        (true, false, _, None) => (Some(Light::Waiting), olo_kit::words::tr("Behind").into()),
    }
}

/// What the pool there last said of itself, in a line: its accounts,
/// the day's requests, what it is answering now; and when it said so,
/// where that is a while ago (it is asked every half minute while the
/// window is open: said only of one that hasn't answered lately).
pub fn said(status: &Value, heard_at: Option<i64>) -> String {
    let accounts = status["accounts"].as_array().map(Vec::as_slice).unwrap_or_default();
    let today: u64 = accounts.iter().map(|a| a["today"]["requests"].as_u64().unwrap_or(0)).sum();
    let open = status["open"].as_array().map_or(0, Vec::len);
    let mut parts = vec![match accounts.len() {
        1 => olo_kit::words::tr("1 account").to_string(),
        n => olo_kit::tr!("{} accounts", n).to_string(),
    }];
    parts.push(match today {
        1 => olo_kit::words::tr("1 request today").to_string(),
        n => olo_kit::tr!("{} requests today", n).to_string(),
    });
    if open > 0 {
        parts.push(match open {
            1 => olo_kit::words::tr("1 request running").to_string(),
            n => olo_kit::tr!("{} requests running", n).to_string(),
        });
    }
    if let Some(at) = heard_at.filter(|at| now() - at > 120) {
        parts.push(olo_kit::tr!("as of {}", ago(at)).to_string());
    }
    parts.join(" · ")
}

/// What the pool there is answering now, then its latest requests,
/// newest first, `most` in all: when ("now" for one running), the
/// model, the account, and the tokens in and out (or how it ended).
pub fn latest(status: &Value, most: usize) -> Vec<[String; 5]> {
    let list = |name: &str| status[name].as_array().map(Vec::as_slice).unwrap_or_default();
    let text = |v: &Value, name: &str| v[name].as_str().unwrap_or("?").to_owned();
    let running = list("open").iter().map(|o| [olo_kit::words::tr("now").to_string(), text(o, "model"), text(o, "account"), "…".to_owned(), String::new()]);
    let served = list("served").iter().rev().map(|s| {
        let (tokens_in, tokens_out) = match s["outcome"].as_str() {
            Some("ok") | None => (
                olo_kit::tr!("{} in", crate::tokens(s["usage"]["input_tokens"].as_u64().unwrap_or(0))).to_string(),
                olo_kit::tr!("{} out", crate::tokens(s["usage"]["output_tokens"].as_u64().unwrap_or(0))).to_string(),
            ),
            Some(other) => (other.replace('_', " "), String::new()),
        };
        [crate::clock(s["at"].as_i64().unwrap_or(0)), text(s, "model"), text(s, "account"), tokens_in, tokens_out]
    });
    running.chain(served).take(most).collect()
}

/// Why a host couldn't be reached, in a few words for a sentence: what
/// the error ends with ("ssh error: connect to nas:22 failed: No route
/// to host" is "no route to host"). The whole of it is what `olo-pool
/// --hosts` says.
pub fn brief(why: &str) -> String {
    let last = why.rsplit(": ").next().unwrap_or(why).trim().trim_end_matches('.');
    let mut chars = last.chars();
    match chars.next() {
        // (A word's capital, not an acronym's.)
        Some(first) if chars.clone().next().is_some_and(char::is_lowercase) => first.to_lowercase().chain(chars).collect(),
        _ => last.to_owned(),
    }
}

/// Made-up hosts, one in each state, for the window's pictures and
/// its lint (`OLO_POOL_DEMO_HOSTS=1`).
fn demo() -> Option<Vec<Stands>> {
    std::env::var_os("OLO_POOL_DEMO_HOSTS")?;
    let t = now();
    let request = |ago: i64, model: &str, account: &str, input: u64, output: u64| serde_json::json!({"at": t - ago, "model": model, "account": account, "outcome": "ok", "usage": {"requests": 1, "input_tokens": input, "output_tokens": output}});
    let status = serde_json::json!({
        "accounts": [{"today": {"requests": 212}}, {"today": {"requests": 38}}, {"today": {"requests": 0}}],
        "open": [{"model": "claude-fable-5-2", "account": "Work"}],
        "served": [request(260, "claude-opus-5-5", "Work", 48_200, 1_900), request(95, "gpt-6.2-astra", "Team", 12_400, 730), request(20, "claude-fable-5-2", "Work", 181_000, 4_250)],
    });
    let host = |id: &str, address: &str| Stands { id: id.into(), name: id.into(), address: address.into(), ..Stands::default() };
    Some(vec![
        Stands { wanted: true, in_step: true, carried_at: Some(t - 150), status: Some(status), heard_at: Some(t - 12), ..host("build", "you@build.example") },
        Stands {
            wanted: true,
            behind_since: Some(t - 2400),
            carried_at: Some(t - 9000),
            error: Some("ssh error: connect to nas.example:22 failed: No route to host".into()),
            ..host("nas", "you@nas.example")
        },
        Stands { doing: Some("Installing".into()), ..host("staging", "deploy@staging.example:2222") },
        Stands { error: Some("Olo has no Token Pool for FreeBSD amd64.".into()), hopeless: true, ..host("bsd", "you@bsd.example") },
        Stands { error: Some("The Token Pool couldn’t be put there: No space left on device".into()), ..host("ci", "you@ci.example") },
        host("dev-box", "you@dev-box.example"),
    ])
}

impl HostsWindow {
    pub fn new(shared: Arc<Shared>, hosts: Arc<Hosts>, cx: &mut Cx<Self>) -> HostsWindow {
        hosts.refresh();
        cx.after(EVERY, HostsMsg::Tick);
        HostsWindow { shared, hosts, asking: None, refused: None, scroll: ScrollHandle::new() }
    }

    /// One host: its light, name and how its pool stands, with what
    /// its state wants done beside (the one on the right edge; Remove…
    /// to its left where it has a pool); under them its address and
    /// what its pool said, or what went wrong, or the question before
    /// it is removed.
    fn host(&self, s: &Stands, wide: f32, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let (lamp, state) = stands(s);
        let lamp = lamp.map_or_else(|| busy_of(LIGHT), light);
        let button = |label: &'static str, id: &str, what: HostsMsg, cx: &mut Cx<Self>| {
            Button::new(label).id(format!("pool-host-{id}-{}", label.to_lowercase().replace(['…', ' '], ""))).min_width(wide).on_click(cx.msg::<ClickEvent>(what))
        };
        let asking = self.asking.as_deref() == Some(s.id.as_str());
        let mut actions = Vec::new();
        // (Nothing to press while something is being done, its ring
        // says so: a dimmed Install beside "Installing…" said it
        // twice. Nor where trying again can't mend it.)
        if s.doing.is_some() || (s.hopeless && !s.wanted) {
        } else if asking {
            // (Cancel where Remove… was pressed, under the pointer: a
            // second click there removes nothing.)
            actions.push(button("Cancel", &s.id, HostsMsg::Ask(None), cx));
            actions.push(button("Remove Pool", &s.id, HostsMsg::Do(s.id.clone(), "remove"), cx));
        } else if s.wanted {
            actions.push(button("Remove…", &s.id, HostsMsg::Ask(Some(s.id.clone())), cx));
            actions.push(button("Sync Now", &s.id, HostsMsg::Do(s.id.clone(), "sync"), cx));
        } else {
            actions.push(button(if s.error.is_some() { "Try Again" } else { "Install" }, &s.id, HostsMsg::Do(s.id.clone(), "install"), cx).enabled(self.hosts.carried()));
        }
        let line = div()
            .w_full()
            .h(px(m::CONTROL_HEIGHT))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::SPACING))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(m::TIGHT))
                    .child(lamp)
                    .child(StringView::new(s.name.clone()).bold(true))
                    .child(StringView::new("·"))
                    .child(div().flex_1().min_w_0().child(StringView::new(state).cut(true))),
            )
            .children(actions);
        // (Under its name, begun where the name begins; every line of
        // detail on one pitch.)
        let detail = |text: String| StringView::new(text).size(style::SMALL_SIZE).dim(true).cut(true).height(DETAIL);
        // (A figure, its end on its column's: not cut, as wide as it is.)
        let figure = |text: String, across: f32| div().w(px(across)).flex_shrink_0().flex().flex_row().justify_end().child(StringView::new(text).size(style::SMALL_SIZE).dim(true).height(DETAIL));
        let mut under = div().w_full().pl(px(LIGHT + m::TIGHT)).flex().flex_col();
        if asking {
            // (What it means for the agents there in the warning's
            // colour, as Rex's questions have what a thing costs; what
            // is done to lose nothing in the plain grey of a detail.)
            under = under
                .child(StringView::new(olo_kit::tr!("Remove the Token Pool from {}?", s.name)).size(style::SMALL_SIZE).cut(true).height(DETAIL))
                .child(StringView::new(olo_kit::words::tr("Claude\u{a0}Code and Codex there go back to their own sign-ins.").to_string()).size(style::SMALL_SIZE).color(style::c(p.warning)).wrap(true))
                .child(StringView::new(olo_kit::words::tr("The accounts’ latest sign-ins are copied back here first.").to_string()).size(style::SMALL_SIZE).dim(true).wrap(true));
        } else {
            under = under.child(detail(s.address.clone()));
            let refused = self.refused.as_ref().filter(|(id, _)| *id == s.id).map(|(_, why)| why.clone());
            // (What its pool says, or why not, a step under its address.)
            let mut says = div().w_full().pt(px(4.0)).flex().flex_col();
            match (refused.or_else(|| s.error.clone()), s.wanted) {
                // (Behind: its light is the waiting one, and so are its
                // words; red is for what went wrong and stays so.)
                (Some(why), true) => says = says.child(StringView::new(olo_kit::tr!("It couldn’t be reached: {}. Olo keeps trying.", brief(&why))).size(style::SMALL_SIZE).dim(true).wrap(true)),
                (Some(why), false) => says = says.child(StringView::new(why).size(style::SMALL_SIZE).color(style::c(p.failure)).wrap(true)),
                (None, _) => {
                    if let Some(status) = &s.status {
                        says = says.child(detail(said(status, s.heard_at)));
                        // (The account's column begins where Remove…
                        // does, and the tokens end where Sync Now does:
                        // every edge on one that is there already.)
                        for [at, model, account, tokens_in, tokens_out] in latest(status, 4) {
                            says = says.child(
                                div()
                                    .w_full()
                                    .flex()
                                    .flex_row()
                                    .child(div().w(px(AT)).flex_shrink_0().child(detail(at)))
                                    .child(div().flex_1().min_w_0().pr(px(m::SPACING)).child(detail(model)))
                                    .child(div().w(px(wide + m::SPACING)).flex_shrink_0().child(detail(account)))
                                    .child(figure(tokens_in, wide / 2.0))
                                    .child(figure(tokens_out, wide / 2.0)),
                            );
                        }
                    }
                }
            }
            under = under.child(says);
        }
        div().w_full().flex().flex_col().child(line).child(under).into_element()
    }
}

impl View for HostsWindow {
    type Message = HostsMsg;

    fn message(&mut self, message: HostsMsg, cx: &mut Cx<Self>) {
        match message {
            HostsMsg::Changed => {}
            HostsMsg::Tick => {
                self.hosts.refresh();
                cx.after(EVERY, HostsMsg::Tick);
            }
            HostsMsg::Ask(host) => self.asking = host,
            HostsMsg::Do(id, what) => {
                self.asking = None;
                self.refused = self.hosts.act(&id, what).err().map(|why| (id, why));
            }
        }
        cx.invalidate();
    }

    fn command(&mut self, name: &str, cx: &mut Cx<Self>) -> bool {
        crate::ui::command(&self.shared, name, cx)
    }

    fn window_closing(&mut self, _: &mut Cx<Self>) {
        olo_kit::proxy().send(Msg::Closed(Which::Hosts));
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let hosts = demo().unwrap_or_else(|| self.hosts.list());
        // (One width for every button of the window: their edges in two columns.)
        let wide = ["Install", "Try Again", "Sync Now", "Remove…", "Remove Pool", "Cancel"].iter().map(|l| button_width(l, false)).fold(m::BUTTON_MIN_WIDTH, f32::max);
        let intro = StringView::new(
            olo_kit::words::tr("Give a Rex host a Token Pool of its own, with the same accounts, so Claude\u{a0}Code and Codex there keep working while this computer is off.").to_string(),
        )
        .wrap(true)
        .dim(true);
        // The hosts in a well of their own, the list's ground, its
        // scroll bar inside it (on the panel's own colour beside a
        // loose bar they read as a page, not a list: Opus's review).
        let mut list = div().w_full().p(px(m::STACK)).flex().flex_col().gap(px(m::SPACING));
        if !self.hosts.carried() && demo().is_none() {
            list = list.child(StringView::new(olo_kit::words::tr("This Olo was built without the Token Pool for hosts, so none can be installed.").to_string()).wrap(true).color(style::c(p.warning)));
        }
        for host in &hosts {
            list = list.child(self.host(host, wide, cx));
        }
        if hosts.is_empty() {
            // (Said in the well, a little above its middle, as one
            // block: a heading, a line, the way to add one.)
            list = div()
                .w_full()
                .pt(px(EMPTY_TOP))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(4.0))
                .child(StringView::new(olo_kit::words::tr("No SSH hosts yet").to_string()).bold(true))
                .child(StringView::new(olo_kit::words::tr("Add one in Rex or in Preferences\u{a0}▸\u{a0}Rex.").to_string()).dim(true))
                .child(div().pt(px(m::TIGHT)).child(Button::new("Open Rex Preferences").id("pool-hosts-preferences").on_click(|_: &ClickEvent, _| olo_kit::app_menu::open_settings("rex"))));
        }
        let content = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .p(px(m::WINDOW_INSET))
            .child(intro)
            .child(div().flex_1().min_h_0().child(ScrollView::new(&self.scroll).background(p.list_background).child(list)));
        div().size_full().flex().flex_col().child(crate::ui::menu_bar(cx)).child(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How a host's pool stands, in its light and words, in every state
    /// it has; and what its pool said of itself, in a line.
    #[test]
    fn each_host_says_how_its_pool_stands() {
        let base = Stands { id: "h".into(), name: "build".into(), ..Stands::default() };
        let of = |s: Stands| stands(&s);
        assert_eq!(of(base.clone()), (Some(Light::Off), "No pool".into()));
        assert_eq!(of(Stands { doing: Some("Installing".into()), ..base.clone() }), (None, "Installing…".into()));
        assert_eq!(of(Stands { error: Some("no room".into()), ..base.clone() }), (Some(Light::Down), "Couldn’t install".into()));
        assert_eq!(of(Stands { wanted: true, in_step: true, carried_at: Some(now() - 5), ..base.clone() }), (Some(Light::On), "Up to date".into()));
        let (lamp, words) = of(Stands { wanted: true, behind_since: Some(now() - 600), error: Some("no route".into()), ..base.clone() });
        assert_eq!(lamp, Some(Light::Waiting));
        assert!(words.starts_with("Behind since "), "{words}");
        assert_eq!(of(Stands { wanted: true, ..base }), (Some(Light::Waiting), "Behind".into()));

        let status = serde_json::json!({"accounts": [{"today": {"requests": 3}}, {"today": {"requests": 4}}], "open": [{}, {}],
            "served": [{"at": 0, "model": "m1", "account": "Work", "outcome": "ok", "usage": {"input_tokens": 1200, "output_tokens": 30}}, {"at": 0, "model": "m2", "account": "Team", "outcome": "rate_limited"}]});
        assert_eq!(said(&status, None), "2 accounts · 7 requests today · 2 requests running");
        assert_eq!(said(&serde_json::json!({"accounts": [{"today": {"requests": 1}}]}), Some(now())), "1 account · 1 request today");
        assert_eq!(said(&serde_json::json!({"accounts": [{"today": {"requests": 1}}]}), Some(now() - 600)), "1 account · 1 request today · as of 10 minutes ago");
        let lines = latest(&status, 9);
        assert_eq!(lines.len(), 4);
        // (What is running first, then the latest, newest first.)
        assert_eq!(lines[0][0], "now");
        assert_eq!(lines[2][1..], ["m2".to_owned(), "Team".to_owned(), "rate limited".to_owned(), String::new()]);
        assert_eq!(lines[3][1..], ["m1".to_owned(), "Work".to_owned(), "1k in".to_owned(), "30 out".to_owned()]);
        assert_eq!(brief("ssh error: connect to nas.example:22 failed: No route to host"), "no route to host");
        assert_eq!(brief("The Token Pool there didn’t answer."), "the Token Pool there didn’t answer");
        assert_eq!(brief("SSH said no"), "SSH said no");
    }
}

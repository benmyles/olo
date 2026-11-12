//! Integrations: the services Olo works with, each connected here (and
//! only here). Telegram: its API ID and signing in (`telegram.rs`). GitHub: Terminal shows each tab's pull request and its
//! checks with it. The sign-in is `olo_system::github`'s: a token Olo keeps
//! in the login keyring, else `GH_TOKEN`, else the GitHub CLI's. Checking
//! and connecting talk to GitHub on a worker.

use olo_sync::Mutex;
use std::sync::Arc;

use olo_kit::{
    Child, ClickEvent, Cx, Element, IntoElement, ParentElement as _, Styled as _, WindowCx, canvas, div, icons, metrics as m, px,
    widgets::{BBox, Button, StringView, TextControl, TextField, TextFieldEvent},
};
use olo_system::github::{self, Source};

use crate::{Msg, PanelView, form::Ui, panel};

/// How big each service's icon is, beside its section.
const INTEGRATION_ICON: f32 = 48.0;

/// Where to make a token: a classic one with the `repo` scope, named.
const NEW_TOKEN: &str = "https://github.com/settings/tokens/new?scopes=repo&description=Olo";

#[derive(Clone, Debug, Default, PartialEq)]
enum GitHub {
    #[default]
    Checking,
    /// No token anywhere.
    None,
    Connected {
        login: String,
        source: Source,
    },
    /// GitHub didn't take the token (or couldn't be reached).
    Failed {
        source: Option<Source>,
        why: String,
    },
}

pub struct State {
    github: Arc<Mutex<GitHub>>,
    /// A token being typed, to connect with.
    token: Child<TextField>,
    adding: bool,
    proxy: olo_kit::Proxy,
    pub telegram: crate::telegram::State,
}

impl State {
    pub fn new(cx: &mut Cx<PanelView>) -> State {
        let token = crate::form::text_field(cx, |f| f.password(true).hint("ghp_… or github_pat_…"));
        cx.subscribe(&token, |v, e: &TextFieldEvent, cx| {
            if matches!(e, TextFieldEvent::Entered)
                && let Some(st) = state_of(v)
            {
                st.connect(cx);
                cx.notify();
            }
        });
        let mut st = State { github: Arc::default(), token, adding: false, proxy: olo_kit::proxy(), telegram: crate::telegram::State::new(cx) };
        // (Tests, the layout lint, never read the keyring.)
        if cfg!(test) {
            *st.github.lock().unwrap() = GitHub::None;
        } else {
            st.check();
        }
        st
    }
}

pub fn state_of(v: &mut PanelView) -> Option<&mut State> {
    match &mut v.state {
        panel::State::Integrations(st) => Some(st),
        _ => None,
    }
}

/// A row of buttons at the right, the first given rightmost (laid out
/// right to left), a little below what's above it (room for a
/// default button's ring).
pub fn buttons(items: Vec<Element>) -> impl IntoElement {
    div().flex().flex_row().justify_end().gap(px(m::SPACING + 2.0)).pt(px(m::TIGHT)).children(items.into_iter().rev())
}

/// A service's icon beside what it does and how it is connected.
pub fn service(icon: &'static str, about: &str, status: String) -> impl IntoElement {
    let art = canvas(move |g, b| {
        icons::paint(g, b, icon);
    })
    .w(px(INTEGRATION_ICON))
    .h(px(INTEGRATION_ICON))
    .flex_shrink_0();
    div()
        .flex()
        .flex_row()
        .gap(px(m::SPACING))
        .child(art)
        .child(div().flex_1().min_w_0().flex().flex_col().child(StringView::new(about.to_owned()).dim(true).wrap(true)).child(StringView::new(status).wrap(true)))
}

/// Who `token` signs in as, by asking GitHub.
fn login_of(token: &str) -> Result<String, String> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    let agent: ureq::Agent =
        ureq::Agent::config_builder().tls_config(tls).user_agent("Olo Preferences").timeout_global(Some(std::time::Duration::from_secs(15))).http_status_as_error(false).build().into();
    // (Asked again across a network change: `olo_net`.)
    let resp = olo_net::across_changes(std::time::Duration::from_secs(10), || {
        agent.get("https://api.github.com/user").header("Accept", "application/vnd.github+json").header("Authorization", &format!("Bearer {}", token.trim())).call()
    })
    .map_err(|e| format!("GitHub couldn’t be reached ({e})."))?;
    match resp.status().as_u16() {
        200 => {
            let body = resp.into_body().read_to_string().map_err(|e| e.to_string())?;
            let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
            Ok(v["login"].as_str().unwrap_or("?").to_owned())
        }
        401 => Err("GitHub didn’t take the token: it may have expired or been revoked.".into()),
        s => Err(format!("GitHub answered {s}.")),
    }
}

/// The token there is, and who it signs in as.
fn status() -> GitHub {
    match github::token() {
        None => GitHub::None,
        Some((t, source)) => match login_of(&t) {
            Ok(login) => GitHub::Connected { login, source },
            Err(why) => GitHub::Failed { source: Some(source), why },
        },
    }
}

impl State {
    /// Find the token and ask GitHub who it is, on a worker.
    fn check(&mut self) {
        *self.github.lock().unwrap() = GitHub::Checking;
        let (out, proxy) = (self.github.clone(), self.proxy.clone());
        std::thread::spawn(move || {
            *out.lock().unwrap() = status();
            proxy.send(Msg::Redraw);
        });
    }

    /// Try the token typed, and keep it if GitHub takes it.
    fn connect(&mut self, cx: &mut Cx<PanelView>) {
        let token = self.token.read(cx).text().to_owned();
        if token.trim().is_empty() {
            return;
        }
        self.token.update(cx, |f, cx| f.clear_secret(cx));
        self.adding = false;
        *self.github.lock().unwrap() = GitHub::Checking;
        let (out, proxy) = (self.github.clone(), self.proxy.clone());
        std::thread::spawn(move || {
            let now = match login_of(&token) {
                Ok(login) => match github::save(&token) {
                    Ok(()) => GitHub::Connected { login, source: Source::Olo },
                    Err(e) => GitHub::Failed { source: None, why: format!("The token couldn’t be kept in the keyring ({e}).") },
                },
                Err(why) => GitHub::Failed { source: None, why },
            };
            *out.lock().unwrap() = now;
            proxy.send(Msg::Redraw);
        });
    }

    /// Forget Olo's token; then whatever else gives one.
    fn disconnect(&mut self) {
        let (out, proxy) = (self.github.clone(), self.proxy.clone());
        *out.lock().unwrap() = GitHub::Checking;
        std::thread::spawn(move || {
            *out.lock().unwrap() = match github::forget() {
                Ok(()) => status(),
                Err(e) => GitHub::Failed { source: Some(Source::Olo), why: format!("The token couldn’t be taken out of the keyring ({e}).") },
            };
            proxy.send(Msg::Redraw);
        });
    }
}

/// Something done to the panel's state from a button.
fn act(ui: &Ui, f: impl Fn(&mut State, &mut Cx<PanelView>) + 'static) -> impl Fn(&ClickEvent, &mut WindowCx) + 'static {
    ui.on_click_view(move |v, cx| {
        if let Some(st) = state_of(v) {
            f(st, cx);
        }
        cx.notify();
    })
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let now = st.github.lock().unwrap().clone();
    let status = match &now {
        GitHub::Checking => "Checking…".to_owned(),
        GitHub::None => "Not connected: only public repositories’ pull requests show.".to_owned(),
        GitHub::Connected { login, source } => format!("Connected as {login}, through {}.", source.describe()),
        GitHub::Failed { source: Some(s), why } => format!("{why} (From {}.)", s.describe()),
        GitHub::Failed { source: None, why } => why.clone(),
    };
    let mut items: Vec<Element> = vec![service("github", "Terminal shows each tab’s pull request on GitHub and whether its checks pass.", status).into_element()];
    if st.adding {
        items.push(TextControl::new(ui.cx, &st.token).label("Token:").into_element());
        items.push(
            StringView::new("A classic token with the repo scope, or a fine-grained one that can read pull requests and checks. Olo keeps it in your login keyring.")
                .dim(true)
                .wrap(true)
                .into_element(),
        );
    }
    let row: Vec<Element> = if st.adding {
        let ready = !st.token.read(ui.cx).text().trim().is_empty();
        vec![
            Button::new("Connect").default(true).enabled(ready).on_click(act(ui, |st, cx| st.connect(cx))).into_element(),
            Button::new("Cancel")
                .on_click(act(ui, |st, cx| {
                    st.adding = false;
                    st.token.update(cx, |f, cx| f.clear_secret(cx));
                }))
                .into_element(),
            Button::new("Make a Token…")
                .on_click(|_, _| {
                    let _ = olo_xdg::open::open(&olo_xdg::open::Target::Url(NEW_TOKEN.into()));
                })
                .into_element(),
        ]
    } else {
        let checking = now == GitHub::Checking;
        let mut row = vec![Button::new("Check Again").enabled(!checking).on_click(act(ui, |st, _| st.check())).into_element()];
        if matches!(now, GitHub::Connected { source: Source::Olo, .. } | GitHub::Failed { source: Some(Source::Olo), .. }) {
            row.push(Button::new("Disconnect").on_click(act(ui, |st, _| st.disconnect())).into_element());
        }
        let label = if matches!(now, GitHub::Connected { .. }) { "Use Another Token…" } else { "Connect with a Token…" };
        let token = st.token.downgrade();
        row.push(
            Button::new(label)
                .enabled(!checking)
                .on_click(ui.on_click_view(move |v, cx| {
                    if let Some(st) = state_of(v) {
                        st.adding = true;
                    }
                    token.focus(cx);
                    cx.notify();
                }))
                .into_element(),
        );
        row
    };
    items.push(buttons(row).into_element());
    ui.add(BBox::new("GitHub").children(items));
    ui.space(m::SPACING);
    crate::telegram::ui(ui, &mut st.telegram);
}

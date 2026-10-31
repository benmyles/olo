//! Integrations: the services Olo works with, each connected here (and
//! only here). Telegram: its API ID and signing in (`telegram.rs`). GitHub: Terminal shows each tab's pull request and its
//! checks with it. The sign-in is `olo_system::github`'s: a token Olo keeps
//! in the login keyring, else `GH_TOKEN`, else the GitHub CLI's. Checking
//! and connecting talk to GitHub on a worker.

use std::sync::{Arc, Mutex};

use olo_kit::{
    egui::{Context, Sense, Ui, vec2},
    icons, metrics as m,
    widgets::{BBox, Button, StringView, TextControl},
};
use olo_system::github::{self, Source};

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
    Connected { login: String, source: Source },
    /// GitHub didn't take the token (or couldn't be reached).
    Failed { source: Option<Source>, why: String },
}

#[derive(Default)]
pub struct State {
    github: Arc<Mutex<GitHub>>,
    started: bool,
    /// A token being typed, to connect with.
    token: String,
    adding: bool,
    telegram: crate::telegram::State,
}

/// A row of buttons, right to left, as tall as a button (the kit's
/// `button_row` centres itself in all the height left, and this panel
/// has more boxes under each row).
pub fn buttons<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.allocate_ui_with_layout(vec2(ui.available_width(), m::CONTROL_HEIGHT), olo_kit::egui::Layout::right_to_left(olo_kit::egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = m::SPACING + 2.0;
        add(ui)
    })
    .inner
}

/// Who `token` signs in as, by asking GitHub.
fn login_of(token: &str) -> Result<String, String> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .user_agent("Olo Preferences")
        .timeout_global(Some(std::time::Duration::from_secs(15)))
        .http_status_as_error(false)
        .build()
        .into();
    let resp = agent
        .get("https://api.github.com/user")
        .header("Accept", "application/vnd.github+json")
        .header("Authorization", &format!("Bearer {}", token.trim()))
        .call()
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
    fn check(&mut self, ctx: &Context) {
        self.started = true;
        *self.github.lock().unwrap() = GitHub::Checking;
        let (out, ctx) = (self.github.clone(), ctx.clone());
        std::thread::spawn(move || {
            *out.lock().unwrap() = status();
            ctx.request_repaint();
        });
    }

    /// Try `token`, and keep it if GitHub takes it.
    fn connect(&mut self, ctx: &Context) {
        let token = std::mem::take(&mut self.token);
        self.adding = false;
        *self.github.lock().unwrap() = GitHub::Checking;
        let (out, ctx) = (self.github.clone(), ctx.clone());
        std::thread::spawn(move || {
            let now = match login_of(&token) {
                Ok(login) => match github::save(&token) {
                    Ok(()) => GitHub::Connected { login, source: Source::Olo },
                    Err(e) => GitHub::Failed { source: None, why: format!("The token couldn’t be kept in the keyring ({e}).") },
                },
                Err(why) => GitHub::Failed { source: None, why },
            };
            *out.lock().unwrap() = now;
            ctx.request_repaint();
        });
    }

    /// Forget Olo's token; then whatever else gives one.
    fn disconnect(&mut self, ctx: &Context) {
        let (out, ctx) = (self.github.clone(), ctx.clone());
        *out.lock().unwrap() = GitHub::Checking;
        std::thread::spawn(move || {
            *out.lock().unwrap() = match github::forget() {
                Ok(()) => status(),
                Err(e) => GitHub::Failed { source: Some(Source::Olo), why: format!("The token couldn’t be taken out of the keyring ({e}).") },
            };
            ctx.request_repaint();
        });
    }
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let ctx = ui.ctx().clone();
    if !st.started {
        st.check(&ctx);
    }
    let now = st.github.lock().unwrap().clone();
    BBox::new("GitHub").show(ui, |ui| {
        let status = match &now {
            GitHub::Checking => "Checking…".to_owned(),
            GitHub::None => "Not connected: only public repositories’ pull requests show.".to_owned(),
            GitHub::Connected { login, source } => format!("Connected as {login}, through {}.", source.describe()),
            GitHub::Failed { source: Some(s), why } => format!("{why} (From {}.)", s.describe()),
            GitHub::Failed { source: None, why } => why.clone(),
        };
        // Its icon beside what it does and how it is connected.
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = m::SPACING;
            let (r, _) = ui.allocate_exact_size(vec2(INTEGRATION_ICON, INTEGRATION_ICON), Sense::hover());
            icons::paint(ui.painter(), r, "github");
            ui.vertical(|ui| {
                ui.add(StringView::new("Terminal shows each tab’s pull request on GitHub and whether its checks pass.").dim(true).wrap(true));
                ui.add(StringView::new(&status).wrap(true));
            });
        });
        if st.adding {
            ui.add(TextControl::new(&mut st.token).label("Token:").password(true).hint("ghp_… or github_pat_…").autofocus(true));
            ui.add(StringView::new("A classic token with the repo scope, or a fine-grained one that can read pull requests and checks. Olo keeps it in your login keyring.").dim(true).wrap(true));
        }
        ui.add_space(m::TIGHT);
        buttons(ui, |ui| {
            if st.adding {
                let enter = ui.input(|i| i.key_pressed(olo_kit::egui::Key::Enter));
                let ok = ui.add_enabled(!st.token.trim().is_empty(), Button::new("Connect").default(true)).clicked();
                if ok || enter && !st.token.trim().is_empty() {
                    st.connect(&ctx);
                }
                if ui.add(Button::new("Cancel")).clicked() {
                    st.adding = false;
                    st.token.clear();
                }
                if ui.add(Button::new("Make a Token…")).clicked() {
                    let _ = olo_kit::open::open(&olo_kit::open::Target::Url(NEW_TOKEN.into()));
                }
            } else {
                let checking = now == GitHub::Checking;
                if ui.add_enabled(!checking, Button::new("Check Again")).clicked() {
                    st.check(&ctx);
                }
                if matches!(now, GitHub::Connected { source: Source::Olo, .. } | GitHub::Failed { source: Some(Source::Olo), .. })
                    && ui.add(Button::new("Disconnect")).clicked()
                {
                    st.disconnect(&ctx);
                }
                let label = if matches!(now, GitHub::Connected { .. }) { "Use Another Token…" } else { "Connect with a Token…" };
                if ui.add_enabled(!checking, Button::new(label)).clicked() {
                    st.adding = true;
                }
            }
        });
    });
    ui.add_space(m::SPACING);
    crate::telegram::ui(ui, &mut st.telegram);
}

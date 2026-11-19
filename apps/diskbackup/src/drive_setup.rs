//! Back Up to Google Drive (Back Up To ▸ Google Drive…): which of the
//! Google accounts connected to Olo DiskBackup backs up to. Preferences
//! ▸ Integrations ▸ Google connects them, each through an app of its
//! own, “Olo (its address)”, which Mail and Calendar sign in through too
//! (Ben, 2026-11-18: "we need to move that ai account setup thing from
//! diskbackup to be in the prefs -> integrations for each google account
//! that someone wants to connect, and then that app must be used"). The
//! backups go in a folder of their own there (“Olo DiskBackup”), which
//! only the account's Olo app sees (Google's `drive.file`).
//!
//! It follows the list of accounts (`olo_google::accounts::path`): one
//! connected while it's open shows at once. Backing up there is
//! `crate::set_up_drive_account` (on a worker), as the Google grid's
//! DiskBackup tick does it (`olo-diskbackup --set-up-drive-account`).

use olo_google::accounts::{self, Account, Service};
use olo_kit::{
    ClickEvent, Cx, Element, IntoElement as _, ParentElement as _, SharedString, Styled as _, View, div, metrics as m, px, style, theme,
    widgets::{Button, MenuField, StringView},
};

use crate::app::Msg;

/// The window's width, points.
const WIDTH: f32 = 460.0;

/// What a worker tells the window.
pub enum Done {
    /// The accounts connected to Olo.
    Accounts(Vec<Account>),
    /// Set up: whose Drive (or why not).
    SetUp(Result<String, String>),
}

pub struct DriveSetup {
    /// Those allowed Google Drive (None: not read yet), and how many
    /// more are connected but weren't allowed it.
    accounts: Option<Vec<Account>>,
    others: usize,
    chosen: usize,
    busy: bool,
    /// What to say (and whether it's an error).
    status: (String, bool),
    /// Set up: whose Drive.
    done: Option<String>,
    _watch: Option<olo_kit::watch::Watch>,
}

impl DriveSetup {
    pub fn new(cx: &mut Cx<Self>) -> DriveSetup {
        cx.work(accounts::accounts, Done::Accounts);
        // (An account connected meanwhile, in Preferences: shown at once.)
        let to = cx.messenger();
        let watch = olo_kit::watch::watch([accounts::path()], move |_| {
            to.send(Done::Accounts(accounts::accounts()));
        });
        DriveSetup { accounts: None, others: 0, chosen: 0, busy: false, status: (String::new(), false), done: None, _watch: watch }
    }

    /// Back up to the chosen account's Drive, on a worker.
    fn back_up_there(&mut self, cx: &mut Cx<Self>) {
        let Some(account) = self.accounts.as_ref().and_then(|l| l.get(self.chosen)).cloned() else { return };
        self.busy = true;
        self.status = (format!("Setting up the backups in {}’s Google Drive…", account.address), false);
        cx.invalidate();
        cx.work(move || crate::set_up_drive_account(&account.address), Done::SetUp);
    }
}

fn open_integrations() {
    olo_kit::app_menu::open_settings("integrations");
}

impl View for DriveSetup {
    type Message = Done;

    fn message(&mut self, d: Done, cx: &mut Cx<Self>) {
        match d {
            Done::Accounts(all) => {
                let backs_up = match olo_protocol::diskbackup::Settings::load().target {
                    olo_protocol::diskbackup::Target::GoogleDrive { account, .. } => Some(account),
                    _ => None,
                };
                let (allowed, others): (Vec<Account>, Vec<Account>) = all.into_iter().partition(|a| a.allowed(Service::DiskBackup));
                // (The one backed up to now first; else as listed.)
                self.chosen = backs_up.and_then(|b| allowed.iter().position(|a| a.address.eq_ignore_ascii_case(&b))).unwrap_or(0);
                self.others = others.len();
                self.accounts = Some(allowed);
            }
            Done::SetUp(r) => {
                self.busy = false;
                match r {
                    Ok(email) => {
                        // (The main window shows where backups go now.)
                        olo_kit::proxy().send(Msg::SetUp);
                        self.status = (format!("Backing up to Google Drive ({email}), in the folder “{}”.", olo_diskbackup::drive::FOLDER), false);
                        self.done = Some(email);
                    }
                    Err(e) => self.status = (format!("Not set up: {e}."), true),
                }
            }
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let dim = |text: String| StringView::new(SharedString::from(text)).wrap(true).dim(true);
        let mut body = div().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET)).w_full().min_w(px(WIDTH));
        let mut buttons = div().flex().flex_row().items_center().justify_end().gap(px(m::SPACING));
        let cancel = |label: &'static str, default: bool| Button::new(label).default(default).on_click(|_: &ClickEvent, cx| cx.close_window());
        match &self.accounts {
            None => body = body.child(dim("Reading the Google accounts connected to Olo…".into())),
            Some(l) if l.is_empty() => {
                body = body.child(dim(
                    "DiskBackup backs up to a Google account’s Drive through that account’s own app, which Olo makes once for each account you connect (Mail and Calendar sign in through it too). Connect yours in Preferences ▸ Integrations ▸ Google: it shows here as soon as it’s connected.".into(),
                ));
                if self.others > 0 {
                    let n = self.others;
                    body = body.child(dim(format!(
                        "{} connected {} allowed Google Drive: sign in again there, and allow it.",
                        if n == 1 { "One account" } else { "Some accounts" },
                        if n == 1 { "wasn’t" } else { "weren’t" }
                    )));
                }
                buttons = buttons.child(cancel("Cancel", false)).child(Button::new("Open Integrations…").default(true).on_click(|_: &ClickEvent, _| open_integrations()));
            }
            Some(l) => {
                let names: Vec<SharedString> = l.iter().map(|a| SharedString::from(a.address.clone())).collect();
                body = body.child(div().flex().child(
                    MenuField::new("drive-account", names, self.chosen.min(l.len() - 1)).label("Back up to the Drive of:").enabled(!self.busy && self.done.is_none()).on_change(cx.listener(
                        |w: &mut DriveSetup, i: &usize, cx| {
                            w.chosen = *i;
                            cx.invalidate();
                        },
                    )),
                ));
                body = body.child(dim(format!(
                    "DiskBackup keeps its backups in a folder “{}” in that account’s Drive, which only Olo’s app for the account sees: nothing else of your Drive. Backups there already (another computer’s) are added to, with their key from Passwords.",
                    olo_diskbackup::drive::FOLDER
                )));
                if self.done.is_some() {
                    buttons = buttons.child(cancel("Done", true));
                } else {
                    buttons = buttons
                        .child(Button::new("Connect Another Account…").enabled(!self.busy).on_click(|_: &ClickEvent, _| open_integrations()))
                        .child(cancel("Cancel", false))
                        .child(Button::new("Back Up There").default(!self.busy).enabled(!self.busy).on_click(cx.listener(|w: &mut DriveSetup, _: &ClickEvent, cx| w.back_up_there(cx))));
                }
            }
        }
        let (text, error) = self.status.clone();
        if !text.is_empty() {
            body = body.child(StringView::new(text).wrap(true).color(if error { style::c(p.failure) } else { style::disabled_text(&p) }));
        }
        body.child(buttons).into_element()
    }
}

/// The window's options.
pub fn options() -> olo_kit::WindowOptions {
    // (A form, as DiskBackup's window: it floats, and follows what it shows.)
    olo_kit::WindowOptions { fixed: true, ..olo_kit::WindowOptions::new("Back Up to Google Drive", WIDTH, 160.0).fits_content().floating() }
}

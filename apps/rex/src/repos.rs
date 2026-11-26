//! The Repositories window, and the form that adds one or changes one:
//! the person's repositories, cloned in their account on the hosts they
//! set up (Ben, 2026-11-22: "choosing what repos to clone and how to
//! set them up"). The list is the person's and the same for every host
//! (the desktop's settings, kept by `RexHub::set_repos`); how each
//! stands is said of the host the Account window is of, which is where
//! this window is opened from (its Repositories row: Choose…). What is
//! missing there is cloned by that window's run (`account::Item::Repos`:
//! one of what Set Up does), or from here (Clone).
//!
//! A window of its own, as every list of the person's choosing is to
//! be: the Account window has one row for all of them.

use std::rc::Rc;

use olo_kit::{
    Child, Element, ParentElement as _, Styled as _, WindowCx, div, metrics as m, px, style,
    widgets::{Button, Column, ColumnListView, ListModel, StringView, TextControl, TextField, button_row, button_width, divider_for},
};
use olo_rex_client::repos::{Repo, There};
use olo_sync::Mutex;

use crate::{
    hub::{HostConnState, HostStands},
    panels::{self, Light},
};

/// The window's width: the Hosts window's.
pub const WIDTH: f32 = 580.0;
/// The form's: an address is long.
pub const FORM_WIDTH: f32 = 480.0;

/// What the window's buttons do.
#[derive(Clone, Debug, PartialEq)]
pub enum ReposDo {
    /// The form, for a new one.
    Add,
    /// The form, for the one chosen.
    Edit(usize),
    Remove(usize),
    /// What is missing on the Account window's host, cloned.
    Clone,
}

/// Which repository the form is of (none: a new one).
static EDITING: Mutex<Option<usize>> = Mutex::new(None);

pub fn editing() -> Option<usize> {
    *EDITING.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn edit(which: Option<usize>) {
    *EDITING.lock().unwrap_or_else(|e| e.into_inner()) = which;
}

/// A row of the list: a repository, its folder, and how it stands on
/// the Account window's host.
#[derive(Clone, Debug, PartialEq)]
pub struct RepoRow {
    pub name: String,
    pub folder: String,
    /// Its light and state; none where no host is asked of (the
    /// Account window is closed, or its host isn't connected).
    pub stands: Option<(Light, String)>,
    /// It is being cloned now: a ring where its light would be (as
    /// the Account window's row has, for the same thing).
    pub ring: bool,
    /// What the line under the buttons says of it, chosen.
    pub line: String,
    /// The line is of something wrong (its light red).
    pub trouble: bool,
}

/// How a repository stands, in a word or two.
fn said(there: &There) -> (Light, &'static str) {
    match there {
        There::Cloned { set_up: true } => (Light::On, "Cloned"),
        There::Cloned { set_up: false } => (Light::Waiting, "Not set up"),
        There::Absent => (Light::Off, "Not cloned"),
        // ("Taken": something else is at its place. "In use" read as
        // held by a running program: Opus's review, 2026-11-23.)
        There::Another(_) | There::Taken => (Light::Down, "Folder taken"),
    }
}

/// What a run of the Account window is doing to the repositories now:
/// the one being cloned, by its name (none: not begun on one yet).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cloning(pub Option<String>);

/// The list's rows: `repos` against `host`, the Account window's.
/// `cloning`: a run is at the repositories. `failed`: the one a run
/// couldn't clone, by its name, and what the host said.
pub fn rows(repos: &[Repo], host: Option<&HostStands>, cloning: Option<&Cloning>, failed: Option<(&str, &str)>) -> Vec<RepoRow> {
    use olo_kit::words::tr;
    let reached = host.filter(|h| h.conn == HostConnState::Connected && h.ssh);
    let there = reached.and_then(|h| h.repos_there.as_ref()).filter(|there| there.len() == repos.len());
    let on = reached.map_or(String::new(), |h| h.name.clone());
    repos
        .iter()
        .enumerate()
        .map(|(i, repo)| {
            let name = repo.name();
            let on_it = cloning.is_some_and(|c| c.0.as_deref() == Some(name.as_str())) && there.is_some_and(|there| there[i].to_do());
            let could_not = failed.filter(|(of, _)| *of == name && there.is_none_or(|there| there[i].to_do()));
            let stands = match (reached.is_some(), there) {
                (false, _) => None,
                _ if could_not.is_some() => Some((Light::Down, tr("Couldn’t clone").to_string())),
                (true, None) => Some((Light::Off, tr("Checking…").to_string())),
                // (While a run is at them: the one it is on, and those behind it.)
                (true, Some(there)) if there[i].to_do() && cloning.is_some() => match cloning.and_then(|c| c.0.as_deref()) == Some(repo.name().as_str()) {
                    true => Some((Light::Waiting, tr("Cloning…").to_string())),
                    false => Some((Light::Off, tr("Waiting").to_string())),
                },
                (true, Some(there)) => {
                    let (light, word) = said(&there[i]);
                    Some((light, tr(word).to_string()))
                }
            };
            // (What is wrong first, and what to do about it; else where
            // it is from, and what sets it up.)
            let (line, trouble) = match (repo.wrong(), there.map(|there| &there[i])) {
                (Some(wrong), _) => (tr(wrong).to_string(), true),
                (None, _) if could_not.is_some() => (olo_kit::tr!("{} couldn’t be cloned: {}", name, could_not.map_or("", |(_, why)| why)).to_string(), true),
                (None, Some(There::Another(origin))) => (olo_kit::tr!("~/{} already holds another repository on {}: {}", repo.place(), on, origin).to_string(), true),
                (None, Some(There::Taken)) => (olo_kit::tr!("~/{} already holds something else on {}. Move it away, or Edit… the folder.", repo.place(), on).to_string(), true),
                (None, _) if repo.setup.trim().is_empty() => (olo_kit::tr!("{}  ·  Set up with nothing", repo.url.trim()).to_string(), false),
                (None, _) => (olo_kit::tr!("{}  ·  Set up with {}", repo.url.trim(), repo.setup.trim()).to_string(), false),
            };
            RepoRow { name, folder: format!("~/{}", repo.place()), stands, ring: on_it, line, trouble }
        })
        .collect()
}

/// The Repositories window: the list (its name, its folder, how it
/// stands on the Account window's host), Add…, Edit… and Remove under
/// it at the left, Clone at the right, and a line that says of the
/// chosen one where it is from and what sets it up, or what is in its
/// way. With no host to say it of (opened by itself), the list alone:
/// no standing, no Clone. `to_clone`: how many a run would do there;
/// `set_up_only`: none of them is to be cloned, only set up.
#[allow(clippy::too_many_arguments)]
pub fn render_panel(
    cx: &olo_kit::Cx<crate::ui::RexWindow>,
    list: &ListModel,
    rows: &Rc<Vec<RepoRow>>,
    chosen: Option<usize>,
    host: Option<&str>,
    to_clone: usize,
    set_up_only: bool,
    failed: Option<String>,
    on_chosen: impl Fn(&(), &mut WindowCx) + 'static,
    on_do: impl Fn(&ReposDo, &mut WindowCx) + 'static,
) -> Element {
    let on_do: Rc<dyn Fn(&ReposDo, &mut WindowCx)> = Rc::new(on_do);
    let tall = |n: usize| px(olo_kit::widgets::list_height(n, m::LIST_ROW, true));
    // (How each stands is of one host: named in the column's title
    // where the name fits it, else "On the host", which the line under
    // the buttons names. Cut, a long name lost what told it from its
    // neighbours: "On build-farm-eu-wes…", Astra's look.)
    let status: String = match host {
        Some(host) => {
            let named: String = olo_kit::tr!("On {}", host).into();
            let room = panels::STATUS - 2.0 * m::CELL_INSET;
            if f32::from(style::text_width(named.clone(), style::PLAIN_SIZE, style::ui_font(olo_kit::FontWeight::NORMAL))) <= room { named } else { olo_kit::words::tr("On the host").into() }
        }
        None => olo_kit::words::tr("Status").into(),
    };
    let view = ColumnListView::new(
        cx,
        "rex-repos",
        list,
        vec![Column::new("Repository", 180.0), Column::new("Folder", panels::STATUS_AT - 180.0), Column::new(status, panels::STATUS)],
        rows.len(),
        |v: &crate::ui::RexWindow, cell, g| {
            let Some(r) = v.repos_rows.get(cell.row) else { return };
            match cell.column {
                0 => cell.text(g, r.name.clone()),
                1 => cell.text(g, r.folder.clone()),
                // (With no host to say it of, nothing: a dash on every
                // row was noise.)
                _ => match &r.stands {
                    Some((_, state)) if r.ring => panels::ring_cell(g, cell, state),
                    Some((light, state)) => panels::lit_cell(g, cell, *light, state),
                    None => {}
                },
            }
        },
    )
    .row_key(|v: &crate::ui::RexWindow, row| v.repos_rows.get(row).map_or(0, |r| style::key((&r.name, &r.folder, r.ring, r.stands.as_ref().map(|(light, state)| (*light, state.clone()))))))
    .empty_text("No repositories yet.")
    .on_select(on_chosen);
    let view = div().w_full().h(tall(panels::list_rows(rows.len(), 5))).child(view);

    // (Clone's count measured as it says itself, in the person's language.)
    let clone = olo_kit::tr!("Clone {}", 9).to_string();
    let wide = ["Add…", "Edit…", "Remove", clone.as_str()].iter().map(|l| button_width(l, false)).fold(m::BUTTON_MIN_WIDTH, f32::max);
    let button = |label: String, id: &'static str, what: Option<ReposDo>| {
        let on_do = on_do.clone();
        let enabled = what.is_some();
        Button::new(label).id(id).min_width(wide).enabled(enabled).on_click(move |_, cx| {
            if let Some(what) = &what {
                on_do(what, cx);
            }
        })
    };
    let mut actions = div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(m::SPACING))
        .child(button(olo_kit::words::tr("Add…").into(), "repos-add", Some(ReposDo::Add)))
        .child(button(olo_kit::words::tr("Edit…").into(), "repos-edit", chosen.map(ReposDo::Edit)))
        .child(button(olo_kit::words::tr("Remove").into(), "repos-remove", chosen.map(ReposDo::Remove)))
        .child(div().flex_1());
    // (Clone is of a host: with none, there is no such button.)
    if host.is_some() {
        actions = actions.child(match to_clone {
            0 => button(olo_kit::words::tr("Clone").into(), "repos-clone", None),
            // (Named for the work: where every one left is cloned
            // already and only its set-up is to run, Set Up.)
            n if set_up_only => button(olo_kit::tr!("Set Up {}", n).into(), "repos-clone", Some(ReposDo::Clone)).default(true),
            n => button(olo_kit::tr!("Clone {}", n).into(), "repos-clone", Some(ReposDo::Clone)).default(true),
        });
    }

    // The chosen one's line, whole: a paragraph, so one too long for the
    // window's width goes on to a second line (a long address, what a
    // host said of a clone it refused). It was one row cut with "…",
    // and the sentence for no row chosen, which named the host, lost
    // its end on a host with a long name (Ben, 2026-11-23: "doesn't fit
    // the text in the window"): nothing here is cut, and a long name
    // makes the window a line taller (it fits its content).
    let row = style::line_row(style::PLAIN_SIZE);
    let wrong = |text: String| {
        div()
            .w_full()
            .flex()
            .flex_row()
            .items_start()
            .gap(px(m::TIGHT))
            .child(div().h(px(row)).flex().items_center().child(panels::light(Light::Down)))
            .child(div().flex_1().min_w_0().child(StringView::new(text).wrap(true)))
    };
    let quiet = |text: String| div().w_full().child(StringView::new(text).dim(true).wrap(true));
    let line = div().w_full().min_h(px(row)).child(match (chosen.and_then(|i| rows.get(i)), failed, host) {
        (Some(r), ..) if r.trouble => wrong(r.line.clone()),
        (Some(r), ..) => quiet(r.line.clone()),
        // (What the host said when a run couldn't clone.)
        (None, Some(failed), _) => wrong(failed),
        (None, None, Some(host)) => quiet(olo_kit::tr!("Each is cloned into its folder in your home on {}, then set up once with its command.", host).into()),
        (None, None, None) => quiet(olo_kit::words::tr("Open a host’s Account window to see how each stands there, and to clone.").into()),
    });

    div().w_full().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET)).child(view).child(div().w_full().flex().flex_col().gap(px(m::TIGHT)).child(actions).child(line))
}

/// The form's three fields as a repository.
pub fn typed(cx: &WindowCx, fields: &[Child<TextField>; 3]) -> Repo {
    let text = |i: usize| fields[i].read(cx).text().trim().to_owned();
    Repo { url: text(0), folder: text(1), setup: text(2) }
}

/// What is wrong with `repo` as typed, among `others` (the list
/// without the one being changed): its own ([`Repo::wrong`]), or a
/// folder another of them has.
pub fn wrong(repo: &Repo, others: &[Repo]) -> Option<String> {
    if let Some(wrong) = repo.wrong() {
        return Some(olo_kit::words::tr(wrong).into());
    }
    // (The folder by its name, which the person may not have typed: an
    // empty field comes to it. And what to do.)
    others.iter().find(|other| other.place() == repo.place()).map(|other| olo_kit::tr!("~/{} is already {}’s folder. Type another one.", repo.place(), other.name()).into())
}

/// The form: an address, a folder, a set-up command; under them what
/// is wrong with what is typed (after a red light), or, with nothing
/// wrong, what the command is for. A field's hint is what an empty
/// field comes to.
pub fn render_form(cx: &WindowCx, fields: &[Child<TextField>; 3], others: &[Repo], adding: bool, on_done: impl Fn(&bool, &mut WindowCx) + 'static) -> Element {
    let on_done = Rc::new(on_done);
    let (cancel, done) = (on_done.clone(), on_done);
    let divider = divider_for(&["Address:", "Folder:", "Set up with:"]);
    let width = FORM_WIDTH - 2.0 * m::WINDOW_INSET;
    let repo = typed(cx, fields);
    let good = wrong(&repo, others).is_none();
    // (Nothing typed yet isn't something wrong.)
    // (Both begin on the same line under the fields: the light is
    // centred in a line of the text's own height, beside a paragraph
    // that starts at its top. Centred against the whole paragraph, the
    // error stood lower than the note it took the place of: Gemini's
    // second look.)
    let line = style::line_row(style::SMALL_SIZE);
    let note = match wrong(&repo, others).filter(|_| !repo.url.is_empty()) {
        Some(wrong) => div()
            .w_full()
            .flex()
            .flex_row()
            .items_start()
            .gap(px(m::TIGHT))
            .child(div().h(px(line)).flex().items_center().child(panels::light(Light::Down)))
            .child(div().flex_1().min_w_0().child(StringView::new(wrong).size(style::SMALL_SIZE).wrap(true))),
        None => div().w_full().child(StringView::new(olo_kit::words::tr("The command runs once, in the new clone. Leave it empty for none.").to_string()).size(style::SMALL_SIZE).dim(true).wrap(true)),
    };
    // (The default button only where it has something to do: greyed,
    // it wore the default's ring. Gemini's look.)
    let confirm = Button::new(if adding { "Add" } else { "Save" }).default(good).enabled(good).on_click(move |_, cx| done(&true, cx));
    div()
        .flex()
        .flex_col()
        .gap(px(m::SPACING))
        .p(px(m::WINDOW_INSET))
        .w_full()
        .child(TextControl::new(cx, &fields[0]).label("Address:").divider(divider).width(width))
        .child(TextControl::new(cx, &fields[1]).label("Folder:").divider(divider).width(width))
        .child(TextControl::new(cx, &fields[2]).label("Set up with:").divider(divider).width(width))
        .child(div().pl(px(divider)).child(note))
        .child(button_row().pt(px(m::TIGHT)).child(Button::new("Cancel").on_click(move |_, cx| cancel(&false, cx))).child(confirm))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(there: Option<Vec<There>>, conn: HostConnState) -> HostStands {
        HostStands {
            kept: Vec::new(),
            name: "dev-box".into(),
            address: "you@dev-box.example".into(),
            conn,
            ssh: true,
            helper: crate::hub::HostHelper::None,
            helper_current: None,
            helper_sync_err: None,
            helper_kept_off: false,
            helper_setting: true,
            bridge: None,
            granted: Vec::new(),
            agent_keys: None,
            skill: None,
            skill_current: None,
            skill_sync_err: None,
            commands: None,
            commands_current: None,
            commands_sync_err: None,
            screens: None,
            screens_doing: None,
            this_olo: None,
            pool: None,
            version: None,
            server_newer: None,
            account: None,
            carried: None,
            sessions: None,
            sessions_window: Default::default(),
            repos: Vec::new(),
            repos_there: there,
        }
    }

    fn four() -> [Repo; 4] {
        [
            Repo { url: "git@github.com:example/olo.git".into(), folder: String::new(), setup: "cargo fetch".into() },
            Repo { url: "https://github.com/example/website".into(), folder: "sites/website".into(), setup: String::new() },
            Repo { url: "git@git.example.org:team/notes.git".into(), folder: String::new(), setup: String::new() },
            Repo { url: "git@git.example.org:team/x.git".into(), folder: String::new(), setup: "make".into() },
        ]
    }

    #[test]
    fn each_repository_says_how_it_stands_on_the_accounts_host() {
        let repos = four();
        let there = vec![There::Cloned { set_up: true }, There::Absent, There::Another("git@elsewhere.example:o/notes.git".into()), There::Cloned { set_up: false }];
        let h = host(Some(there), HostConnState::Connected);
        let rows_there = rows(&repos, Some(&h), None, None);
        assert_eq!((rows_there[0].name.as_str(), rows_there[0].folder.as_str()), ("olo", "~/src/olo"));
        assert_eq!(rows_there[0].stands, Some((Light::On, "Cloned".into())));
        assert_eq!(rows_there[0].line, "git@github.com:example/olo.git  ·  Set up with cargo fetch");
        assert_eq!((rows_there[1].folder.as_str(), rows_there[1].stands.clone(), rows_there[1].trouble), ("~/sites/website", Some((Light::Off, "Not cloned".into())), false));
        assert_eq!(rows_there[1].line, "https://github.com/example/website  ·  Set up with nothing");
        assert_eq!(rows_there[2].stands, Some((Light::Down, "Folder taken".into())));
        assert_eq!(rows_there[2].line, "~/src/notes already holds another repository on dev-box: git@elsewhere.example:o/notes.git");
        assert!(rows_there[2].trouble);
        assert_eq!(rows_there[3].stands, Some((Light::Waiting, "Not set up".into())));
        // While a run is at them: the one it is on, those behind it; what is cloned or in the way as it was.
        let running = rows(&repos, Some(&h), Some(&Cloning(Some("website".into()))), None);
        assert_eq!(running.iter().map(|r| r.stands.clone().unwrap().1).collect::<Vec<_>>(), ["Cloned", "Cloning…", "Folder taken", "Waiting"]);
        assert_eq!(running.iter().map(|r| r.ring).collect::<Vec<_>>(), [false, true, false, false], "a ring for the one it is on");
        // One a run couldn't clone: marked, and its line says which and why.
        let refused = rows(&repos, Some(&h), None, Some(("website", "Permission denied (publickey).")));
        assert_eq!(refused[1].stands, Some((Light::Down, "Couldn’t clone".into())));
        assert_eq!((refused[1].line.as_str(), refused[1].trouble), ("website couldn’t be cloned: Permission denied (publickey).", true));
        assert_eq!(refused[0].stands, Some((Light::On, "Cloned".into())));
        // No host to say it of (the Account window closed, or its host away): the list still, with no standing.
        for none in [rows(&repos, None, None, None), rows(&repos, Some(&host(None, HostConnState::Disconnected)), None, None)] {
            assert!(none.iter().all(|r| r.stands.is_none()) && none.len() == 4);
        }
        // Asked, not answered (or the list changed since).
        assert_eq!(rows(&repos, Some(&host(None, HostConnState::Connected)), None, None)[0].stands, Some((Light::Off, "Checking…".into())));
        assert_eq!(rows(&repos, Some(&host(Some(vec![There::Absent]), HostConnState::Connected)), None, None)[1].stands, Some((Light::Off, "Checking…".into())));
        // A folder that holds something else: what to do about it.
        let taken = rows(&repos[..1], Some(&host(Some(vec![There::Taken]), HostConnState::Connected)), None, None);
        assert_eq!(taken[0].line, "~/src/olo already holds something else on dev-box. Move it away, or Edit… the folder.");
    }

    #[test]
    fn what_is_typed_wrong_is_said_and_a_folder_is_one_repositorys() {
        let repos = four();
        assert_eq!(wrong(&Repo { url: "not an address".into(), ..Repo::default() }, &repos), Some("This isn’t a repository’s address (git@host:owner/name.git, or https://…).".into()));
        // (Another of the list has its folder: by its place, however it is written.)
        let same = Repo { url: "git@github.com:other/olo.git".into(), ..Repo::default() };
        assert_eq!(wrong(&same, &repos), Some("~/src/olo is already olo’s folder. Type another one.".into()));
        assert_eq!(wrong(&Repo { folder: "~/src/olo2".into(), ..same.clone() }, &repos), None);
        // (The one being changed isn't among the others.)
        assert_eq!(wrong(&repos[0], &repos[1..]), None);
    }
}

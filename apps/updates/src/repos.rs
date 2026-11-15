//! The Repositories section: every repository the system knows (on ones
//! first), each with a check box to turn it on or off (the daemon writes
//! the configuration; polkit asks), its packages, when its metadata was
//! made and the file that says it; the selected one's every setting
//! beside the list: where it's fetched from, its keys and whether its
//! packages' signatures are checked, its priority and cost, what it
//! leaves out. Add Repository… takes a .repo file's address, a Copr
//! project or RPM Fusion; Remove takes away a file Olo or the person put
//! in /etc/yum.repos.d (one that came with a package goes with it).

use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use olo_kit::{
    Bounds, Child, ClickEvent, Cx, Element, IntoElement as _, Messenger, ParentElement as _, Pixels, Point, ScrollHandle, Styled as _, View, WindowOptions, agent, div, metrics as m, px, style, theme,
    widgets::{Button, CheckBox, Column, ColumnListView, ListModel, RadioButton, ScrollView, StringView, TextControl, TextField, TextFieldEvent, button_row, check_box_look, open_alert, text_field},
};
use olo_system::dnf::{self, Opts, Repo};

use crate::{
    Msg as AppMsg, OsUpdates, helper, release,
    review::{self, Op, Plan},
    updates::{field, heading, link, text},
    words,
};

const DETAILS: f32 = 300.0;

#[derive(Clone)]
pub enum Msg {
    Selected,
    Toggle(usize),
    /// Turning one on or off is done (its id, or why not).
    Switched(Result<String, String>),
    /// Which package a repository's file came with.
    Owner(String, Option<String>),
    Remove,
    Removed(Result<String, String>),
    /// A .repo file to write (its name and text), from Add Repository.
    Write(String, String),
    Written(Result<String, String>),
}

pub struct State {
    list: ListModel,
    /// The repositories shown, as indices.
    rows: Rc<Vec<usize>>,
    checks: Rc<RefCell<HashMap<usize, Bounds<Pixels>>>>,
    pub selected: Option<String>,
    /// The package each file came with (None: none, or not known yet).
    owners: HashMap<String, Option<String>>,
    pub busy: Option<String>,
    scroll: ScrollHandle,
}

impl Default for State {
    fn default() -> State {
        State { list: ListModel::new(false), rows: Rc::default(), checks: Rc::default(), selected: None, owners: HashMap::new(), busy: None, scroll: ScrollHandle::new() }
    }
}

/// Whether a repository's file can be taken away: one in
/// /etc/yum.repos.d (Fedora's own are in /usr/share and stay).
pub fn removable(r: &Repo) -> bool {
    std::path::Path::new(&r.repofile).parent() == Some(helper::repos_dir().as_path()) && r.repofile.ends_with(".repo")
}

impl State {
    pub fn took(&mut self, repos: &[Repo]) {
        self.rows = Rc::new((0..repos.len()).collect());
        let row = self.selected.as_ref().and_then(|id| repos.iter().position(|r| r.id == *id));
        let mut st = self.list.state_mut();
        st.selected.clear();
        if let Some(r) = row {
            st.select(r);
        }
    }

    pub fn selected<'a>(&self, repos: &'a [Repo]) -> Option<&'a Repo> {
        self.selected.as_ref().and_then(|id| repos.iter().find(|r| r.id == *id))
    }
}

fn switch(app: &mut OsUpdates, id: String, on: bool, cx: &mut Cx<OsUpdates>) {
    let Some(session) = app.session.clone() else { return };
    app.r.busy = Some(format!("Turning {} {id}…", if on { "on" } else { "off" }));
    cx.work(move || session.enable(&[&id], on).map(|()| id).map_err(|e| dnf::error_text(&e)), |r| AppMsg::Repos(Msg::Switched(r)));
}

pub fn message(app: &mut OsUpdates, msg: Msg, cx: &mut Cx<OsUpdates>) {
    match msg {
        Msg::Selected => {
            let row = app.r.list.state().single();
            app.r.selected = row.and_then(|r| app.r.rows.get(r)).and_then(|i| app.repos.get(*i)).map(|r| r.id.clone());
            if let Some(r) = app.r.selected(&app.repos).cloned()
                && removable(&r)
                && !app.r.owners.contains_key(&r.repofile)
                && let Some(s) = app.session.clone()
            {
                let file = r.repofile.clone();
                app.r.owners.insert(file.clone(), None);
                cx.work(
                    move || {
                        let opts = Opts::new().strs("patterns", &[&file]).str("scope", "installed").bool("with_filenames", true).bool("with_provides", false).bool("with_nevra", false);
                        let owner = s.packages(opts, &["name"]).ok().and_then(|l| l.first().and_then(|a| a.get("name")).map(|v| dnf::json(v)).and_then(|v| v.as_str().map(str::to_owned)));
                        (file, owner)
                    },
                    |(f, o)| AppMsg::Repos(Msg::Owner(f, o)),
                );
            }
        }
        Msg::Toggle(row) => {
            let Some(r) = app.r.rows.get(row).and_then(|i| app.repos.get(*i)).cloned() else { return };
            switch(app, r.id, !r.enabled, cx);
        }
        Msg::Switched(r) => {
            app.r.busy = None;
            match r {
                Ok(id) => {
                    app.say(format!("{id} was turned {}.", if app.repos.iter().any(|r| r.id == id && r.enabled) { "off" } else { "on" }), false);
                    app.load(false, cx);
                }
                Err(e) => app.say(e, true),
            }
        }
        Msg::Owner(file, owner) => {
            app.r.owners.insert(file, owner);
        }
        Msg::Remove => remove(app, cx),
        Msg::Removed(r) | Msg::Written(r) => {
            app.r.busy = None;
            match r {
                Ok(said) => {
                    app.say(said, false);
                    app.load(false, cx);
                }
                Err(e) => app.say(e, true),
            }
        }
        Msg::Write(name, contents) => {
            app.r.busy = Some(format!("Adding {name}…"));
            cx.work(move || helper::write_repo(&name, &contents).map(|path| format!("Added {path}.")), |r| AppMsg::Repos(Msg::Written(r)));
        }
    }
}

/// Remove the selected repository's file (asked first), or, when it came
/// with a package, offer to remove that.
fn remove(app: &mut OsUpdates, cx: &mut Cx<OsUpdates>) {
    let Some(r) = app.r.selected(&app.repos).cloned() else { return };
    if !removable(&r) {
        app.say(format!("{} is Fedora’s own: it can be turned off, not removed.", r.id), true);
        return;
    }
    let others: Vec<String> = app.repos.iter().filter(|o| o.repofile == r.repofile && o.id != r.id).map(|o| o.id.clone()).collect();
    if let Some(Some(pkg)) = app.r.owners.get(&r.repofile).cloned() {
        let title = format!("Remove {pkg}");
        review::open(Plan { title: title.clone(), ops: vec![(Op::Remove, vec![pkg])], release: None, offline_only: false, comment: title }, cx.messenger());
        return;
    }
    let mut message = format!("Its file, {}, is removed", r.repofile);
    if !others.is_empty() {
        message.push_str(&format!(", and with it {} that it also holds ({})", words::count(others.len(), "repository", "repositories"), others.join(", ")));
    }
    message.push_str(". Packages installed from it stay installed, but get no more updates from it.");
    let path = r.repofile.clone();
    let to = cx.messenger();
    open_alert(cx, "Remove Repository", "warning", &format!("Remove the repository “{}”?\n\n{message}", r.name), &["Cancel", "Remove"], move |choice, _| {
        if *choice == Some(1) {
            let path = path.clone();
            let to = to.clone();
            std::thread::spawn(move || {
                let r = helper::remove_repo(&path).map(|()| format!("Removed {path}."));
                to.send(AppMsg::Repos(Msg::Removed(r)));
            });
        }
    });
}

pub fn command(app: &mut OsUpdates, command: &str, cx: &mut Cx<OsUpdates>) -> bool {
    match command {
        "add-repo" => AddRepo::open(cx.messenger()),
        "repo-on" | "repo-off" => {
            let Some(id) = app.r.selected.clone() else { return false };
            switch(app, id, command == "repo-on", cx);
        }
        "remove-repo" => remove(app, cx),
        "repo-file" => {
            if let Some(r) = app.r.selected(&app.repos)
                && let Err(e) = olo_kit::tracker::open(std::path::Path::new(&r.repofile))
            {
                tracing::warn!("open {} in Tracker: {e}", r.repofile);
            }
        }
        _ => return false,
    }
    cx.invalidate();
    true
}

pub fn body(app: &mut OsUpdates, cx: &mut Cx<OsUpdates>) -> Element {
    let p = theme::palette();
    let on = app.repos.iter().filter(|r| r.enabled).count();
    let head = StringView::new(if app.session.is_some() { format!("{} repositories, {on} on. Packages come from those that are on.", app.repos.len()) } else { String::new() }).dim(true);
    let list = list_view(app, cx);
    let list = crate::well(cx, p.list_background, list);
    let details = details_view(app, cx);
    let selected = app.r.selected(&app.repos).cloned();
    let foot = div()
        .flex()
        .items_center()
        .gap(px(m::SPACING))
        .child(Button::new("Add Repository…").id("add-repo").on_click(cx.listener(|v: &mut OsUpdates, _: &ClickEvent, cx| {
            v.run("add-repo", cx);
        })))
        .child(Button::new("Remove…").id("remove-repo").enabled(selected.as_ref().is_some_and(removable)).on_click(cx.listener(|v: &mut OsUpdates, _: &ClickEvent, cx| {
            v.run("remove-repo", cx);
        })))
        .child(div().flex_1().min_w_0().child(match &app.r.busy {
            Some(b) => StringView::new(b.clone()).dim(true).into_element(),
            None => app.status(cx),
        }));
    div()
        .size_full()
        .flex()
        .flex_col()
        .gap(px(m::SPACING))
        .child(head)
        .child(div().flex_1().min_h_0().flex().flex_row().gap(px(m::SPACING)).child(div().flex_1().min_w_0().h_full().child(list)).child(div().w(px(DETAILS)).flex_shrink_0().h_full().child(details)))
        .child(foot)
        .into_element()
}

fn cell_text(r: &Repo, c: usize) -> String {
    match c {
        1 => r.name.clone(),
        2 => r.id.clone(),
        3 => {
            if r.enabled {
                r.available_pkgs.to_string()
            } else {
                String::new()
            }
        }
        4 => words::date(r.updated),
        5 => r.repofile.rsplit('/').next().unwrap_or_default().to_owned(),
        _ => String::new(),
    }
}

fn list_view(app: &mut OsUpdates, cx: &mut Cx<OsUpdates>) -> Element {
    let columns: Rc<[Column]> =
        Rc::from(vec![Column::new("On", 30.0), Column::new("Name", 230.0), Column::new("Id", 170.0), Column::new("Packages", 72.0).right(), Column::new("Updated", 96.0), Column::new("File", 200.0)]);
    let rows = app.r.rows.clone();
    let checks = app.r.checks.clone();
    let press = checks.clone();
    let (cell_rows, key_rows, menu_rows) = (rows.clone(), rows.clone(), rows.clone());
    ColumnListView::new(cx, "repos-list", &app.r.list, columns, rows.len(), move |v: &OsUpdates, cell, g| {
        let Some(r) = cell_rows.get(cell.row).and_then(|i| v.repos.get(*i)) else { return };
        if cell.column == 0 {
            let p = theme::palette();
            let s = px(m::MARK_SIZE);
            let c = cell.bounds.center();
            let well = style::snap(g, Bounds::new(olo_kit::point(c.x - s / 2.0, c.y - s / 2.0), olo_kit::size(s, s)));
            check_box_look(g, well, &p, r.enabled);
            checks.borrow_mut().insert(cell.row, cell.bounds);
            agent::record(g, agent::Node { role: "CheckBox", name: Some(r.id.clone().into()), bounds: well, enabled: true, clickable: true, checked: Some(r.enabled), ..Default::default() });
            return;
        }
        if r.enabled {
            cell.text(g, cell_text(r, cell.column));
        } else {
            let p = theme::palette();
            let color = if cell.selected { cell.text_color } else { style::disabled_text(&p) };
            cell.text_with(g, cell_text(r, cell.column), style::ui_font(olo_kit::FontWeight::NORMAL), color);
        }
    })
    .row_key(move |v: &OsUpdates, row| {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        if let Some(r) = key_rows.get(row).and_then(|i| v.repos.get(*i)) {
            (&r.id, r.enabled, r.available_pkgs, r.updated).hash(&mut h);
        }
        h.finish()
    })
    .on_select(cx.listener(|v: &mut OsUpdates, _: &(), cx| {
        message(v, Msg::Selected, cx);
        cx.invalidate();
    }))
    .on_press(cx.asks(move |v: &mut OsUpdates, &(row, at): &(usize, Point<Pixels>), cx| {
        if !press.borrow().get(&row).is_some_and(|b| b.contains(&at)) {
            return false;
        }
        message(v, Msg::Toggle(row), cx);
        cx.invalidate();
        true
    }))
    .on_context(cx.listener(move |v: &mut OsUpdates, &(row, at): &(usize, Point<Pixels>), cx| {
        v.r.list.state_mut().selected.clear();
        v.r.list.state_mut().select(row);
        message(v, Msg::Selected, cx);
        let Some(r) = menu_rows.get(row).and_then(|i| v.repos.get(*i)).cloned() else { return };
        let m = olo_kit::Menu::new("Repository")
            .item(if r.enabled { "Turn Off" } else { "Turn On" }, if r.enabled { "repo-off" } else { "repo-on" })
            .item("Remove Repository…", "remove-repo")
            .enabled(removable(&r))
            .separator()
            .item("Show File in Tracker", "repo-file")
            .enabled(!r.repofile.is_empty());
        let to = cx.view_ref();
        cx.invalidate();
        olo_kit::menu::context_menu(cx, m, at, move |command, cx| {
            let command = command.to_owned();
            to.update(cx, move |v: &mut OsUpdates, cx| {
                v.run(&command, cx);
            });
        });
    }))
    .into_element()
}

fn details_view(app: &mut OsUpdates, cx: &mut Cx<OsUpdates>) -> Element {
    let p = theme::palette();
    let content: Element = match app.r.selected(&app.repos).cloned() {
        None => StringView::new("Select a repository to see everything about it.").dim(true).wrap(true).into_element(),
        Some(r) => {
            let mut col = div().flex().flex_col().gap(px(m::TIGHT));
            col = col.child(StringView::new(r.name.clone()).bold(true).size(style::TITLE_SIZE).height(style::line_row(style::TITLE_SIZE)).wrap(true));
            col = col.child(div().pt(px(m::TIGHT)).child(CheckBox::new("On: packages come from it", r.enabled).id("repo-enabled").on_toggle(cx.listener(|v: &mut OsUpdates, on: &bool, cx| {
                if let Some(id) = v.r.selected.clone() {
                    switch(v, id, *on, cx);
                    cx.invalidate();
                }
            }))));
            col = col.child(heading("Where From"));
            col = col.child(field("Id", text(r.id.clone())));
            for u in &r.baseurl {
                col = col.child(field("Address", if u.starts_with("http") { link(u.clone(), u.clone()) } else { text(u.clone()) }));
            }
            if !r.metalink.is_empty() {
                col = col.child(field("Metalink", text(r.metalink.clone())));
            }
            if !r.mirrorlist.is_empty() {
                col = col.child(field("Mirror list", text(r.mirrorlist.clone())));
            }
            if r.enabled {
                col = col.child(field("Packages", text(format!("{} ({})", r.available_pkgs, dnf::size_words(r.size)))));
            }
            if r.updated > 0 {
                col = col.child(field("Made", text(words::date_time(r.updated))));
            }
            if r.cache_updated > 0 {
                col = col.child(field("Fetched", text(words::ago(r.cache_updated))));
            }
            if r.metadata_expire > 0 {
                let h = r.metadata_expire / 3600;
                col = col.child(field("Checked every", text(if h >= 48 && h % 24 == 0 { format!("{} days", h / 24) } else { format!("{h} hours") })));
            }
            col = col.child(heading("Trust"));
            col = col.child(field("Signatures", text(if r.gpgcheck { "Each package’s is checked" } else { "Not checked: its packages could be anyone’s" })));
            if r.repo_gpgcheck {
                col = col.child(field("Metadata", text("Its signature is checked")));
            }
            for k in &r.gpgkey {
                col = col.child(field("Key", if k.starts_with("http") { link(k.clone(), k.clone()) } else { text(k.trim_start_matches("file://").to_owned()) }));
            }
            col = col.child(heading("Settings"));
            col = col.child(field("Priority", text(format!("{} (lower wins)", r.priority))));
            col = col.child(field("Cost", text(r.cost.to_string())));
            col = col.child(field("If it’s down", text(if r.skip_if_unavailable { "Skipped" } else { "Everything waits" })));
            if !r.excludepkgs.is_empty() {
                col = col.child(field("Leaves out", text(r.excludepkgs.join(", "))));
            }
            if !r.includepkgs.is_empty() {
                col = col.child(field("Only", text(r.includepkgs.join(", "))));
            }
            if !r.proxy.is_empty() {
                col = col.child(field("Proxy", text(r.proxy.clone())));
            }
            col = col.child(field("File", text(r.repofile.clone())));
            if let Some(Some(pkg)) = app.r.owners.get(&r.repofile) {
                col = col.child(field("Came with", text(format!("the package {pkg}"))));
            }
            col.into_element()
        }
    };
    let inner = div().w_full().p(px(m::SPACING)).child(content);
    crate::well(cx, p.document_background, ScrollView::new(&app.r.scroll).border(false).auto_hide(true).background(p.document_background).child(inner))
}

// Add Repository.

/// A repository a .repo file says.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Section {
    pub id: String,
    pub name: String,
    pub source: String,
    pub enabled: bool,
    pub gpgcheck: bool,
    pub gpgkey: String,
}

/// A .repo file's repositories, or why it isn't one: each `[id]` must
/// say where it's fetched from (baseurl, metalink or mirrorlist).
pub fn parse_repo_file(text: &str) -> Result<Vec<Section>, String> {
    if text.len() > 64 * 1024 {
        return Err("That’s too big to be a .repo file.".into());
    }
    let mut out: Vec<Section> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(id) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            let id = id.trim();
            if id.is_empty() || id.contains(char::is_whitespace) || id.contains('/') {
                return Err(format!("“[{id}]” isn’t a repository’s id."));
            }
            out.push(Section { id: id.to_owned(), enabled: true, gpgcheck: true, ..Default::default() });
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            if out.last().is_some_and(|s| s.source.ends_with(',')) || line.starts_with("http") {
                continue; // (a continued list of addresses)
            }
            return Err(format!("“{line}” isn’t a setting."));
        };
        let Some(s) = out.last_mut() else { return Err("A setting comes before any [repository].".into()) };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "name" => s.name = v.to_owned(),
            "baseurl" | "metalink" | "mirrorlist" if s.source.is_empty() => s.source = v.split_whitespace().next().unwrap_or_default().to_owned(),
            "enabled" => s.enabled = !matches!(v, "0" | "false" | "no" | "False"),
            "gpgcheck" | "pkg_gpgcheck" => s.gpgcheck = !matches!(v, "0" | "false" | "no" | "False"),
            "gpgkey" => s.gpgkey = v.to_owned(),
            _ => {}
        }
    }
    if out.is_empty() {
        return Err("It has no [repository] in it.".into());
    }
    if let Some(s) = out.iter().find(|s| s.source.is_empty()) {
        return Err(format!("“{}” doesn’t say where its packages are (no baseurl, metalink or mirrorlist).", s.id));
    }
    Ok(out)
}

/// The HTTPS client (certificates as the computer has them).
fn agent() -> ureq::Agent {
    static AGENT: std::sync::OnceLock<olo_net::PerNetwork<ureq::Agent>> = std::sync::OnceLock::new();
    AGENT
        .get_or_init(|| {
            olo_net::watch();
            olo_net::PerNetwork::new(|| {
                let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
                let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
                ureq::Agent::config_builder()
                    .tls_config(tls)
                    .user_agent(concat!("olo-updates/", env!("CARGO_PKG_VERSION")))
                    .http_status_as_error(false)
                    .timeout_connect(Some(Duration::from_secs(15)))
                    .timeout_global(Some(Duration::from_secs(60)))
                    .build()
                    .into()
            })
        })
        .get()
}

/// Fetch `url`'s text (https; a test's file:// too).
pub fn fetch(url: &str) -> Result<String, String> {
    if let Some(path) = url.strip_prefix("file://")
        && !dnf::real()
    {
        return std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"));
    }
    if !url.starts_with("https://") {
        return Err("Only an https:// address: one that can be listened in on could be changed on its way.".into());
    }
    let mut reply = agent().get(url).call().map_err(|e| format!("It couldn’t be fetched: {e}"))?;
    let status = reply.status().as_u16();
    if status != 200 {
        return Err(format!("The server answered {status}{}.", if status == 404 { ": nothing’s there (for this Fedora release?)" } else { "" }));
    }
    reply.body_mut().with_config().limit(64 * 1024).read_to_string().map_err(|e| format!("It couldn’t be read: {e}"))
}

/// What's being added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Way {
    Address,
    Copr,
    Fusion,
}

pub struct AddRepo {
    way: Way,
    address: Child<TextField>,
    copr: Child<TextField>,
    free: bool,
    nonfree: bool,
    /// The file fetched: its name, text and what it says.
    found: Option<(String, String, Vec<Section>)>,
    note: Option<(String, bool)>,
    looks: u64,
    to_main: Messenger<AppMsg>,
    scroll: ScrollHandle,
}

#[derive(Clone)]
pub enum AddMsg {
    Way(Way),
    /// Typed in a field: what was found was for what was there.
    Typed,
    Look,
    Found(u64, Result<(String, String, Vec<Section>), String>),
    Add,
    Free(bool),
    Nonfree(bool),
    Cancel,
}

static LOOKS: AtomicU64 = AtomicU64::new(0);

/// A Copr project's .repo file: its address and the name dnf gives it.
fn copr(project: &str, release: u32) -> Result<(String, String), String> {
    let (owner, name) = project.trim().trim_start_matches("copr:").split_once('/').ok_or_else(|| "A Copr project is owner/project (as “@group/project” for a group’s).".to_owned())?;
    let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.+".contains(c));
    let (group, owner) = match owner.strip_prefix('@') {
        Some(g) => (true, g),
        None => (false, owner),
    };
    if !ok(owner) || !ok(name) {
        return Err("That isn’t a Copr project’s name.".into());
    }
    let who = if group { format!("group_{owner}") } else { owner.to_owned() };
    let path = if group { format!("g/{owner}") } else { owner.to_owned() };
    let url = format!("https://copr.fedorainfracloud.org/coprs/{path}/{name}/repo/fedora-{release}/{who}-{name}-fedora-{release}.repo");
    Ok((url, format!("_copr:copr.fedorainfracloud.org:{who}:{name}.repo")))
}

impl AddRepo {
    pub fn open(to_main: Messenger<AppMsg>) {
        olo_kit::open_window(WindowOptions::new("Add Repository", 560.0, 420.0).dialog().fits_content(), move |cx| {
            let address = text_field(cx, |f| f.name("Address").hint("https://example.org/example.repo"));
            let copr = text_field(cx, |f| f.name("Copr project").hint("owner/project"));
            for field in [&address, &copr] {
                cx.subscribe(field, |_: &mut AddRepo, e: &TextFieldEvent, cx| match e {
                    TextFieldEvent::Changed => cx.send(AddMsg::Typed),
                    TextFieldEvent::Entered => cx.send(AddMsg::Look),
                    _ => {}
                });
            }
            address.focus(cx);
            AddRepo { way: Way::Address, address, copr, free: true, nonfree: true, found: None, note: None, looks: 0, to_main, scroll: ScrollHandle::new() }
        });
    }

    fn look(&mut self, cx: &mut Cx<Self>) {
        let n = LOOKS.fetch_add(1, Ordering::Relaxed) + 1;
        self.looks = n;
        self.found = None;
        let release = release::current().unwrap_or(0);
        let (url, name) = match self.way {
            Way::Address => {
                let url = self.address.read(cx).text().trim().to_owned();
                if url.is_empty() {
                    return;
                }
                let name = url.rsplit('/').next().unwrap_or_default().to_owned();
                (url, name)
            }
            Way::Copr => match copr(self.copr.read(cx).text(), release) {
                Ok(x) => x,
                Err(e) => {
                    self.note = Some((e, true));
                    return;
                }
            },
            Way::Fusion => return,
        };
        let name = if name.ends_with(".repo") { name } else { format!("{name}.repo") };
        self.note = Some(("Fetching…".into(), false));
        cx.work(
            move || {
                let text = fetch(&url)?;
                let sections = parse_repo_file(&text)?;
                Ok((name, text, sections))
            },
            move |r| AddMsg::Found(n, r),
        );
    }
}

impl View for AddRepo {
    type Message = AddMsg;

    fn message(&mut self, message: AddMsg, cx: &mut Cx<Self>) {
        match message {
            AddMsg::Way(w) => {
                if w != self.way {
                    self.way = w;
                    self.found = None;
                    self.note = None;
                }
            }
            AddMsg::Typed => {
                self.found = None;
                self.note = None;
            }
            AddMsg::Look => self.look(cx),
            AddMsg::Found(n, r) => {
                if n != self.looks {
                    return;
                }
                match r {
                    Ok(found) => {
                        self.note = None;
                        self.found = Some(found);
                    }
                    Err(e) => self.note = Some((e, true)),
                }
            }
            AddMsg::Free(on) => self.free = on,
            AddMsg::Nonfree(on) => self.nonfree = on,
            AddMsg::Add => {
                match self.way {
                    Way::Fusion => {
                        let release = release::current().unwrap_or(0);
                        let mut urls = Vec::new();
                        if self.free {
                            urls.push(format!("https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-{release}.noarch.rpm"));
                        }
                        if self.nonfree {
                            urls.push(format!("https://mirrors.rpmfusion.org/nonfree/fedora/rpmfusion-nonfree-release-{release}.noarch.rpm"));
                        }
                        if urls.is_empty() {
                            return;
                        }
                        let title = "Add RPM Fusion".to_owned();
                        review::open(Plan { title: title.clone(), ops: vec![(Op::Install, urls)], release: None, offline_only: false, comment: title }, self.to_main.clone());
                    }
                    _ => {
                        let Some((name, text, _)) = self.found.clone() else { return };
                        self.to_main.send(AppMsg::Repos(Msg::Write(name, text)));
                    }
                }
                cx.close_window();
            }
            AddMsg::Cancel => cx.close_window(),
        }
        cx.invalidate();
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        if command == "close" {
            cx.close_window();
            return true;
        }
        false
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let way = self.way;
        let radio = |label: &str, w: Way, cx: &mut Cx<Self>| RadioButton::new(label.to_owned(), way == w).on_select(cx.msg(AddMsg::Way(w)));
        let mut col = div().w_full().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET)).bg(style::c(p.panel_background));
        col = col.child(StringView::new("Add a repository: its packages and updates come with Fedora’s. Add only ones you trust: whoever makes it can put anything on this computer.").wrap(true));
        col = col.child(radio("A .repo file’s address", Way::Address, cx));
        if way == Way::Address {
            col = col.child(div().pl(px(22.0)).flex().gap(px(m::SPACING)).child(div().flex_1().child(TextControl::new(cx, &self.address))).child(Button::new("Look").on_click(cx.msg(AddMsg::Look))));
        }
        col = col.child(radio("A Copr project", Way::Copr, cx));
        if way == Way::Copr {
            col = col.child(div().pl(px(22.0)).flex().gap(px(m::SPACING)).child(div().flex_1().child(TextControl::new(cx, &self.copr))).child(Button::new("Look").on_click(cx.msg(AddMsg::Look))));
        }
        col = col.child(radio("RPM Fusion: what Fedora can’t ship (codecs, drivers)", Way::Fusion, cx));
        if way == Way::Fusion {
            col = col.child(
                div()
                    .pl(px(22.0))
                    .flex()
                    .flex_col()
                    .gap(px(m::TIGHT))
                    .child(CheckBox::new("Free (open source)", self.free).on_toggle(cx.msg_with(|on: &bool| AddMsg::Free(*on))))
                    .child(CheckBox::new("Nonfree (NVIDIA’s drivers, Steam, …)", self.nonfree).on_toggle(cx.msg_with(|on: &bool| AddMsg::Nonfree(*on))))
                    .child(StringView::new("Its release packages are installed: they add its repositories and keys.").dim(true).small(true).wrap(true)),
            );
        }
        if let Some((t, failed)) = &self.note {
            let mut s = StringView::new(t.clone()).wrap(true);
            s = if *failed { s.color(style::c(p.failure)) } else { s.dim(true) };
            col = col.child(s);
        }
        if let Some((name, _, sections)) = &self.found {
            let mut list = div().w_full().flex().flex_col().gap(px(m::TIGHT)).p(px(m::TIGHT));
            for s in sections {
                list = list
                    .child(StringView::new(format!("[{}] {}", s.id, if s.name.is_empty() { &s.id } else { &s.name })).bold(true).wrap(true))
                    .child(StringView::new(s.source.clone()).small(true).wrap(true));
                if !s.gpgcheck {
                    list = list.child(StringView::new("Its packages’ signatures aren’t checked.").small(true).color(style::c(p.failure)));
                }
                if !s.enabled {
                    list = list.child(StringView::new("Added turned off.").small(true).dim(true));
                }
            }
            col = col.child(StringView::new(format!("{} will hold:", helper::repos_dir().join(name).display())).dim(true)).child(div().h(px(120.0)).child(crate::well(
                cx,
                p.list_background,
                ScrollView::new(&self.scroll).border(false).background(p.list_background).child(list),
            )));
        }
        let can = match way {
            Way::Fusion => self.free || self.nonfree,
            _ => self.found.is_some(),
        };
        col = col.child(
            button_row()
                .child(Button::new("Cancel").on_click(cx.msg(AddMsg::Cancel)))
                .child(Button::new(if way == Way::Fusion { "Add…" } else { "Add" }).id("add").default(can).enabled(can).on_click(cx.msg(AddMsg::Add))),
        );
        col.into_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_files_are_read() {
        let s = parse_repo_file(
            "[olo]\nname=Olo\nbaseurl=https://example.org/$basearch\ngpgcheck=1\ngpgkey=https://example.org/key\n\n[olo-src]\nname=Olo source\nmetalink=https://example.org/m\nenabled=0\n",
        )
        .unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].id.as_str(), s[0].source.as_str(), s[0].gpgcheck), ("olo", "https://example.org/$basearch", true));
        assert!(!s[1].enabled);
        assert!(parse_repo_file("<html>Not found</html>").is_err());
        assert!(parse_repo_file("[x]\nname=no source\n").is_err());
        assert!(parse_repo_file("name=x\n").is_err());
    }

    #[test]
    fn copr_projects_have_addresses() {
        assert_eq!(
            copr("ben/olo", 45).unwrap(),
            ("https://copr.fedorainfracloud.org/coprs/ben/olo/repo/fedora-45/ben-olo-fedora-45.repo".into(), "_copr:copr.fedorainfracloud.org:ben:olo.repo".into())
        );
        assert_eq!(copr("@kde/stable", 45).unwrap().1, "_copr:copr.fedorainfracloud.org:group_kde:stable.repo");
        assert!(copr("nonsense", 45).is_err() && copr("a/b c", 45).is_err());
    }
}

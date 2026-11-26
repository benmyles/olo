//! `olo-rex-link screen …`: Agent Screens of this host's own. An agent on
//! a Rex host has had a screen on the person's desktop, asked through
//! Rex (`olo screen …`, the bridge): the desktop had to be connected and
//! awake, and the screen drew there. This is the same thing here (Ben,
//! 2026-11-26: "there should be an option for agents on a rex host to use
//! olo agent screens on the host itself … the host's olo agent screens
//! should use software rendering and provide a full olo desktop for the
//! agent and it should also have chrome installed and ready to use").
//!
//! A screen is what it is on the desktop: `olo-screen`'s, a headless Olo
//! session with Deskbar, Tracker's desktop and every app, drawn in
//! software where the host has no GPU (app_server takes Mesa's software
//! device by itself). So Olo itself is put on the host, under the account
//! and nowhere else, in a place of its own: `~/.local/share/olo/desktop`
//! (`bin`, `share/olo`), from Olo's release on GitHub, checked, without
//! its wallpapers (1.6 GB the screens do without). Its programs find
//! their data beside them (`olo_xdg::data_dirs`). Chrome is put in the
//! home folder by Olo's own `olo-chrome` (Google's package unpacked
//! there: no root, nothing outside the home). And a font with emoji,
//! where the host has none (a server seldom has; every emoji is a box
//! then, in Olo's windows and on a page): Noto Color Emoji, from its
//! makers' repository at one commit, checked against the SHA-256 written
//! here, into the account's fonts (`~/.local/share/fonts/olo-screens`),
//! where fontconfig finds it for Olo's programs and Chrome alike.
//!
//! ```text
//! olo-rex-link screen install [--tag TAG | --from ARCHIVE] [--again] [--no-chrome] [--no-emoji-font]
//!     Olo put here (the newest release; the one tagged TAG; ARCHIVE, a
//!     file or a URL with its .sha256 beside it), or brought up to it
//!     (nothing fetched where that release is here already, unless
//!     `--again`),
//!     and Chrome: a line of JSON for each step as it goes, then what
//!     `status` says (status 3: it is put, and this host lacks something
//!     its screens need, which `missing` and `how` say)
//! olo-rex-link screen status
//!     what is here, as JSON: {"installed": VERSION|null, "prefix",
//!     "bytes", "chrome": VERSION|null, "missing": [libraries its
//!     programs need that this host hasn't], "older": {library:
//!     [versions of its symbols they need that this host's hasn't]},
//!     "software_drawing": true|false, "emoji_font": true|false (none:
//!     every emoji is a box, on Olo's windows and on a page), "default":
//!     "host"|"desktop"}
//! olo-rex-link screen default [host|desktop]
//!     where `olo screen` goes on this host when it isn't said
//!     (`--on host`, `--on desktop`, `OLO_SCREENS_ON`); none: says it
//! olo-rex-link screen remove
//!     Olo taken off this host again (Chrome and the screens' own
//!     folders stay)
//! olo-rex-link screen COMMAND…
//!     `olo-screen COMMAND…` of the Olo here: start, run, look, tree,
//!     click, type, key, stop and the rest, as on the desktop
//! olo-rex-link screen start [NAME] [--keep] …
//!     the same, and the screen ends with the Rex tab it was started
//!     from (below); `--keep`: it stays until it is stopped
//! ```
//!
//! **A screen ends with the tab that started it.** On the desktop a
//! screen left running is seen by the person, in Agent Screens; on a
//! host nobody sees it, and one left by each agent that ever wanted a
//! page is a quarter of a gigabyte each, more with Chrome, for ever. So
//! a screen started from a Rex tab is stopped (kept, as a stopped one
//! is: its history, its Chrome profile) when that tab's own program
//! ends: the agent, or the shell. The tab is known by `REX_BLOCK`, and
//! its program is the furthest parent of ours started with the same one
//! (`/proc/PID/environ`). A keeper waits for that program's end, on a
//! file descriptor of the process itself (`pidfd_open`: no polling, and
//! no other process ever taken for it), and for the screen's own
//! session's the same way: a screen stopped first, by hand, leaves its
//! keeper nothing to do, and it ends. Outside a Rex tab there is no
//! such program to go by, and the screen stays until stopped, as with
//! `--keep`.

use std::os::unix::fs::{FileExt as _, MetadataExt as _};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use serde_json::json;

const USAGE: &str = "usage: olo screen install [--tag TAG | --from ARCHIVE] [--no-chrome] [--no-emoji-font]
                                            Olo and Chrome put on this host for screens here (and a font with emoji, if it has none)
       olo screen status                    what is here, as JSON
       olo screen default [host|desktop]    where `olo screen` goes on this host when it isn't said
       olo screen remove                    Olo taken off this host again
       olo screen --on host COMMAND…        olo-screen's own commands, on this host (start, run, look, tree, click, …)";

/// Noto Color Emoji (SIL Open Font License 1.1), as its makers' own
/// repository has it at one commit (the release "unicode 18.0" of
/// 2026-09-24): 10.7 MB. What is fetched is refused unless it is this.
const EMOJI_FONT: &str = "NotoColorEmoji.ttf";
const EMOJI_FONT_URL: &str = "https://raw.githubusercontent.com/googlefonts/noto-emoji/e20cbc2bbec1926686be9f9bee7d1d2cfa1fea0e/2D/fonts/NotoColorEmoji.ttf";
const EMOJI_FONT_SHA256: &str = "15671215ab769fdc7162a045d56fd7d7e477c51b04e6b3c761d914d8fdd6cc44";

/// The account's own fonts (fontconfig's `<dir prefix="xdg">fonts</dir>`).
fn account_fonts() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home().join(".local/share")).join("fonts")
}

/// Where a font put here for the screens goes: a folder of its own
/// among the account's fonts, so `remove` knows what is Olo's.
fn screens_fonts() -> PathBuf {
    account_fonts().join("olo-screens")
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// Where Olo is on a host, for its screens (`OLO_HOST_DESKTOP`: another
/// place, for Olo's own tests).
pub fn prefix() -> PathBuf {
    std::env::var_os("OLO_HOST_DESKTOP").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home().join(".local/share/olo/desktop"))
}

/// Where this host's default is kept: one word, `host` or `desktop`.
fn default_file() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home().join(".config")).join("olo/screens-on")
}

fn default_now() -> &'static str {
    match std::fs::read_to_string(default_file()).unwrap_or_default().trim() {
        "host" => "host",
        _ => "desktop",
    }
}

fn say(v: serde_json::Value) {
    use std::io::Write as _;
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

fn fail(why: impl std::fmt::Display) -> ! {
    eprintln!("olo screen: {why}");
    std::process::exit(1);
}

pub fn run(args: Vec<String>) -> ! {
    match args.first().map(String::as_str) {
        None | Some("--help" | "-h" | "help") => {
            println!("{USAGE}");
            std::process::exit(0);
        }
        Some("install") => install(&args[1..]),
        Some("status") => {
            say(status());
            std::process::exit(0);
        }
        Some("default") => default(&args[1..]),
        Some("remove") => remove(),
        // (The window that shows screens is a window: the desktop's.)
        Some("watch") => fail("watching a screen is done from the Olo desktop; here, `olo screen --on host look NAME` is a picture of it"),
        Some("start") => start(&args),
        // (Its own: what waits for a tab's program to end.)
        Some("keep-until") => keep_until(&args[1..]),
        Some(_) => exec(&args),
    }
}

/// The Olo here's `olo-screen`, to be run with `args`: its own programs
/// first on the path (`olo screen run NAME olo-chrome …`).
fn screen_tool(args: &[String]) -> Command {
    let prefix = prefix();
    let program = prefix.join("bin/olo-screen");
    if !program.is_file() {
        fail(format_args!("Olo isn't on this host yet: `olo screen install` puts it here, under {} (about 600 MB to download; nothing needs root)", prefix.display()));
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(std::iter::once(prefix.join("bin")).chain(std::env::split_paths(&path))).unwrap_or(path);
    let mut tool = Command::new(&program);
    tool.args(args).env("PATH", path);
    tool
}

/// `olo-screen ARGS…` of the Olo here.
fn exec(args: &[String]) -> ! {
    let mut tool = screen_tool(args);
    let e = tool.exec();
    fail(format_args!("{}: {e}", tool.get_program().to_string_lossy()));
}

/// `start`: the screen started, and (from a Rex tab, unless `--keep`)
/// a keeper left that stops it when the tab's program ends.
fn start(args: &[String]) -> ! {
    let keep = args.iter().any(|a| a == "--keep");
    let passed: Vec<String> = args.iter().filter(|a| *a != "--keep").cloned().collect();
    let out = match screen_tool(&passed).stdin(std::process::Stdio::inherit()).stderr(std::process::Stdio::inherit()).output() {
        Ok(out) => out,
        Err(e) => fail(format_args!("olo-screen: {e}")),
    };
    {
        use std::io::Write as _;
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(&out.stdout);
        let _ = stdout.flush();
    }
    if out.status.success()
        && !keep
        && let Ok(screen) = serde_json::from_slice::<serde_json::Value>(&out.stdout)
        && let (Some(name), Some(session)) = (screen["name"].as_str(), screen["pid"].as_i64())
        && let Some((owner, began)) = tab_program()
        && let Some((_, session_began)) = parent_and_start(session as i32)
    {
        // (In the screen's process group, so that it is seen with it.
        // Its own file: this program, as it was started.)
        let me = std::env::current_exe().map(|p| PathBuf::from(p.to_string_lossy().trim_end_matches(" (deleted)"))).unwrap_or_else(|_| "olo-rex-link".into());
        let kept = Command::new(me)
            .args(["screen", "keep-until", name, &owner.to_string(), &began.to_string(), &session.to_string(), &session_began.to_string()])
            .process_group(session as i32)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if let Err(e) = kept {
            eprintln!("olo screen: {name} will stay until it is stopped (what would stop it with this tab couldn't start: {e})");
        }
    }
    std::process::exit(out.status.code().unwrap_or(1));
}

/// A process's parent and when it began (in the kernel's ticks since
/// boot: with its number, what it is and no other), from
/// `/proc/PID/stat`.
fn parent_and_start(pid: i32) -> Option<(i32, u64)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // (After its name, which may hold anything: the last `)`.)
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    Some((fields.get(1)?.parse().ok()?, fields.get(19)?.parse().ok()?))
}

/// What a process was started with for `REX_BLOCK`: the Rex tab's pane
/// it is in.
fn block_of(pid: i32) -> Option<Vec<u8>> {
    let environ = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    environ.split(|b| *b == 0).find_map(|var| var.strip_prefix(b"REX_BLOCK=")).filter(|v| !v.is_empty()).map(<[u8]>::to_vec)
}

/// The program of the Rex tab this was run from: the furthest of our
/// parents started in the same pane as we were (`REX_BLOCK`), with when
/// it began. None outside a Rex tab.
fn tab_program() -> Option<(i32, u64)> {
    let block = std::env::var_os("REX_BLOCK").filter(|b| !b.is_empty())?;
    let block = std::os::unix::ffi::OsStrExt::as_bytes(block.as_os_str());
    let mut found = None;
    let mut pid = unsafe { libc::getppid() };
    // (A chain of parents is short; a hundred is a loop that isn't one.)
    for _ in 0..100 {
        if pid <= 1 || block_of(pid).as_deref() != Some(block) {
            break;
        }
        let (parent, began) = parent_and_start(pid)?;
        found = Some((pid, began));
        pid = parent;
    }
    found
}

/// A process held: a descriptor of the process itself (the one that
/// began at `began`), readable when it has ended. Its number given to
/// another after it ends is never taken for it. None where it has ended
/// already.
fn held(pid: i32, began: u64) -> Option<i32> {
    // SAFETY: a system call with two integers.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
    if fd < 0 {
        return None;
    }
    if parent_and_start(pid).is_some_and(|(_, b)| b == began) {
        return Some(fd);
    }
    // SAFETY: ours, just opened.
    unsafe { libc::close(fd) };
    None
}

/// `keep-until NAME PID BEGAN SESSION SESSION_BEGAN`: wait for the
/// process PID (the tab's program) to end, then stop screen NAME; or
/// for the screen's own session to end, and then there is nothing to
/// stop. (The session too: a screen stopped by hand ends its session,
/// not its process group, and its keeper, left waiting for the tab,
/// would have stopped whatever screen had that name by then: seen on
/// the dev server's own account, 2026-11-26.)
fn keep_until(args: &[String]) -> ! {
    let [name, owner, began, session, session_began] = args else { std::process::exit(2) };
    let (Ok(owner), Ok(began), Ok(session), Ok(session_began)) = (owner.parse::<i32>(), began.parse::<u64>(), session.parse::<i32>(), session_began.parse::<u64>()) else { std::process::exit(2) };
    // The screen it was left for has ended already: nothing to keep.
    let Some(session) = held(session, session_began) else { std::process::exit(0) };
    if let Some(owner) = held(owner, began) {
        let mut waiting = [libc::pollfd { fd: owner, events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: session, events: libc::POLLIN, revents: 0 }];
        // (Each readable when its process has ended. Interrupted: wait again.)
        // SAFETY: two descriptors of ours, in an array of two.
        while unsafe { libc::poll(waiting.as_mut_ptr(), 2, -1) } < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {}
        // The screen ended first, stopped by hand or by itself.
        if waiting[1].revents != 0 {
            std::process::exit(0);
        }
    }
    // The tab's program has ended and the screen still runs: stopped.
    // (In a group of its own: `stop` may end the screen's, which is ours.)
    let _ = screen_tool(&["stop".to_owned(), name.clone()]).process_group(0).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    std::process::exit(0);
}

/// Says how far a step has got: when the step changes, when it ends,
/// and each quarter second between.
struct Told {
    doing: &'static str,
    last: Option<Instant>,
}

impl Told {
    fn tell(&mut self, doing: &'static str, done: u64, total: u64) {
        let now = Instant::now();
        if self.doing == doing && done < total && self.last.is_some_and(|t| now.duration_since(t) < Duration::from_millis(250)) {
            return;
        }
        (self.doing, self.last) = (doing, Some(now));
        let fraction = if total > 0 { (done as f64 / total as f64).min(1.0) } else { 0.0 };
        say(json!({ "doing": doing, "fraction": (fraction * 1000.0).round() / 1000.0 }));
    }
}

fn option(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn install(args: &[String]) -> ! {
    let prefix = prefix();
    if olo_release::arch() != "x86_64" && option(args, "--from").is_none() {
        fail(format_args!("Olo's releases are for x86_64; this host is {}", olo_release::arch()));
    }
    let ask = olo_release::Install { from: option(args, "--from"), tag: option(args, "--tag"), retire: Vec::new(), without: vec!["olo-wallpapers-".into()] };
    let mut told = Told { doing: "", last: None };
    // The release asked for is the one here already: nothing is fetched
    // (600 MB, to find every program the same). `--again` fetches it
    // all the same; an archive given is always put.
    let here = olo_release::installed_in(&prefix).version.filter(|_| prefix.join("bin/olo-screen").is_file());
    let wanted = match (&ask.from, &ask.tag, args.iter().any(|a| a == "--again")) {
        (None, Some(tag), false) => olo_release::by_tag(tag).ok().map(|r| r.version),
        (None, None, false) => olo_release::latest().ok().map(|r| r.version),
        _ => None,
    };
    if here.is_some() && here == wanted {
        say(json!({ "installed": here, "changed": 0, "was": true }));
    } else {
        let (changed, version) = match olo_release::install(&prefix, &ask, &AtomicBool::new(false), &mut |doing, done, all| told.tell(doing, done, all)) {
            Ok(done) => done,
            Err(e) => {
                say(json!({ "error": e }));
                std::process::exit(1);
            }
        };
        say(json!({ "installed": version, "changed": changed.len() }));
    }
    // Chrome, by Olo's own launcher: Google's package, unpacked in the
    // home folder. What it says is passed on as it comes.
    if !args.iter().any(|a| a == "--no-chrome") {
        say(json!({ "doing": "chrome" }));
        match Command::new(prefix.join("bin/olo-chrome")).arg("--olo-install").stdin(std::process::Stdio::null()).status() {
            Ok(s) if s.success() => {}
            Ok(s) => say(json!({ "error": format!("Chrome wasn't put here (olo-chrome --olo-install: {s}); Olo is, and `olo screen install` again tries Chrome again") })),
            Err(e) => say(json!({ "error": format!("Chrome wasn't put here (olo-chrome: {e})") })),
        }
    }
    // A font with emoji, where this host has none.
    if !args.iter().any(|a| a == "--no-emoji-font") && !emoji_font() {
        let put = (|| {
            let fetched = olo_release::fetch_file(EMOJI_FONT, EMOJI_FONT_URL, EMOJI_FONT_SHA256, &olo_release::cache_dir(), &AtomicBool::new(false), &mut |got, of| told.tell("emoji font", got, of))?;
            let dir = screens_fonts();
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let (tmp, to) = (dir.join(format!(".{EMOJI_FONT}.new")), dir.join(EMOJI_FONT));
            std::fs::copy(&fetched, &tmp).and_then(|_| std::fs::rename(&tmp, &to)).map_err(|e| format!("{}: {e}", to.display()))?;
            let _ = std::fs::remove_file(&fetched);
            Ok::<_, String>(to)
        })();
        match put {
            Ok(to) => say(json!({ "emoji_font": to })),
            Err(e) => say(json!({ "error": format!("a font with emoji wasn't put here ({e}); emoji are boxes until one is: `olo screen install` again tries again") })),
        }
    }
    let now = status();
    let ready = now["missing"].as_array().is_some_and(Vec::is_empty) && now["older"].as_object().is_some_and(|o| o.is_empty()) && now["software_drawing"] == true;
    say(now);
    std::process::exit(if ready { 0 } else { 3 });
}

fn default(args: &[String]) -> ! {
    match args.first().map(String::as_str) {
        None => println!("{}", default_now()),
        Some(to @ ("host" | "desktop")) => {
            let file = default_file();
            if let Some(dir) = file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = std::fs::write(&file, format!("{to}\n")) {
                fail(format_args!("{}: {e}", file.display()));
            }
            println!("{to}");
        }
        Some(other) => fail(format_args!("`{other}` is neither `host` nor `desktop`")),
    }
    std::process::exit(0);
}

fn remove() -> ! {
    let prefix = prefix();
    if !prefix.join("bin/olo-screen").is_file() {
        println!("Olo isn't on this host: nothing to take off");
        std::process::exit(0);
    }
    // (Never under a screen that runs: its programs are these files.)
    let listed = Command::new(prefix.join("bin/olo-screen")).arg("list").output().ok().and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok()).unwrap_or_default();
    let running: Vec<String> = listed.as_array().into_iter().flatten().filter(|s| s["state"] == "running").filter_map(|s| s["name"].as_str().map(str::to_owned)).collect();
    if !running.is_empty() {
        fail(format_args!("{} here ({}): `olo screen --on host stop NAME` first", if running.len() == 1 { "a screen still runs" } else { "screens still run" }, running.join(", ")));
    }
    if let Err(e) = std::fs::remove_dir_all(&prefix) {
        fail(format_args!("{}: {e}", prefix.display()));
    }
    // (And the font put here with it, in its folder of its own.)
    let _ = std::fs::remove_dir_all(screens_fonts());
    println!("Olo is off this host ({})", prefix.display());
    std::process::exit(0);
}

/// What is here.
fn status() -> serde_json::Value {
    let prefix = prefix();
    let installed = olo_release::installed_in(&prefix).version.filter(|_| prefix.join("bin/olo-screen").is_file());
    let dirs = library_dirs();
    let has = |name: &str| dirs.iter().any(|d| d.join(name).exists());
    let mut missing: Vec<String> = Vec::new();
    // A library here but older than the programs were linked against:
    // the versions of its symbols they need that it hasn't.
    let mut older: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> = Default::default();
    let mut defined: std::collections::HashMap<String, Option<Vec<String>>> = Default::default();
    let mut programs = 0;
    for entry in std::fs::read_dir(prefix.join("bin")).into_iter().flatten().flatten() {
        let Some(program) = Linked::open(&entry.path()) else { continue };
        programs += 1;
        for library in program.needed() {
            if !has(&library) && !missing.contains(&library) {
                missing.push(library);
            }
        }
        lacking(&program.versions_needed(), &mut older, &mut |library| {
            defined.entry(library.to_owned()).or_insert_with(|| dirs.iter().map(|d| d.join(library)).find(|p| p.exists()).and_then(|p| Linked::open(&p)).map(|l| l.versions_defined())).clone()
        });
    }
    missing.sort();
    // Drawn in software: Mesa's EGL, and its software renderer (one
    // library since Mesa 24.2, `libgallium`; a driver of its own before).
    let gallium = dirs.iter().any(|d| std::fs::read_dir(d).into_iter().flatten().flatten().any(|e| e.file_name().to_string_lossy().starts_with("libgallium")) || d.join("dri/swrast_dri.so").exists());
    let software = has("libEGL_mesa.so.0") && gallium;
    let chrome = std::fs::read_link(home().join(".local/share/olo/chrome/current")).ok().map(|p| p.to_string_lossy().into_owned());
    let mut v = json!({
        "installed": installed,
        "prefix": prefix,
        "programs": programs,
        "bytes": if installed.is_some() { bytes_under(&prefix) } else { 0 },
        "chrome": chrome,
        "missing": missing,
        "older": older,
        "software_drawing": software,
        "emoji_font": emoji_font(),
        "default": default_now(),
    });
    if installed.is_none() {
        v["how"] = json!("olo screen install");
    } else if !older.is_empty() {
        v["how"] = json!("this host's libraries are older than Olo's programs are linked against (Fedora 44's): its screens can't run here until the system is a newer one");
    } else if !software {
        v["how"] =
            json!("Mesa's EGL and software renderer aren't here: on Fedora, `sudo dnf install mesa-libEGL mesa-dri-drivers`; on Debian or Ubuntu, `sudo apt install libegl-mesa0 libgl1-mesa-dri`");
    }
    v
}

/// Whether this host has a font with emoji, as far as a file's name
/// says (Noto Color Emoji, Twemoji, Apple Color Emoji…), among the
/// system's fonts and the account's. A server often has none, and then
/// every emoji is a box, in Olo's windows and on a page in Chrome alike.
fn emoji_font() -> bool {
    let mut todo: Vec<(PathBuf, u32)> = [PathBuf::from("/usr/share/fonts"), PathBuf::from("/usr/local/share/fonts"), account_fonts(), home().join(".fonts")].into_iter().map(|d| (d, 0)).collect();
    while let Some((dir, depth)) = todo.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                if depth < 4 {
                    todo.push((entry.path(), depth + 1));
                }
            } else if name.contains("emoji") && [".ttf", ".otf", ".ttc"].iter().any(|x| name.ends_with(x)) {
                return true;
            }
        }
    }
    false
}

/// Of the versions a program `needs` of each library's symbols, those
/// the library here hasn't (`defined`: what it has; None where it isn't
/// here at all, which `missing` says), added to `older`.
fn lacking(needs: &[(String, Vec<String>)], older: &mut std::collections::BTreeMap<String, std::collections::BTreeSet<String>>, defined: &mut dyn FnMut(&str) -> Option<Vec<String>>) {
    for (library, versions) in needs {
        let Some(here) = defined(library) else { continue };
        for version in versions {
            if !here.contains(version) {
                older.entry(library.clone()).or_default().insert(version.clone());
            }
        }
    }
}

/// What `dir` holds on disk, in bytes (as it is stored, not its length).
fn bytes_under(dir: &Path) -> u64 {
    let mut total = 0;
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                todo.push(entry.path());
            } else {
                total += meta.blocks() * 512;
            }
        }
    }
    total
}

/// Where this host's loader looks for a library: its usual folders for
/// this machine, and those `/etc/ld.so.conf` names.
fn library_dirs() -> Vec<PathBuf> {
    let machine = std::env::consts::ARCH;
    let mut dirs: Vec<PathBuf> = ["/lib64", "/usr/lib64", "/usr/local/lib64", "/lib", "/usr/lib", "/usr/local/lib"].iter().map(PathBuf::from).collect();
    dirs.push(format!("/lib/{machine}-linux-gnu").into());
    dirs.push(format!("/usr/lib/{machine}-linux-gnu").into());
    let mut confs = vec![PathBuf::from("/etc/ld.so.conf")];
    confs.extend(std::fs::read_dir("/etc/ld.so.conf.d").into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "conf")));
    for conf in confs {
        for line in std::fs::read_to_string(conf).unwrap_or_default().lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.starts_with('/') {
                dirs.push(line.into());
            }
        }
    }
    dirs.retain(|d| d.is_dir());
    dirs.dedup();
    dirs
}

/// A program or a library as its loader reads it: a 64-bit ELF of this
/// machine's byte order with a dynamic section, read from the file's own
/// headers (nothing of it is run). None for anything else (a script, a
/// static program).
pub struct Linked {
    file: std::fs::File,
    /// Its dynamic section: each entry's tag and value.
    dynamic: Vec<(u64, u64)>,
    /// Where its strings are, in the file.
    strings: u64,
    /// What is loaded where: (address, place in the file, length).
    loads: Vec<(u64, u64, u64)>,
}

fn u16_of(b: &[u8], at: usize) -> u64 {
    b.get(at..at + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]) as u64)
}

fn u32_of(b: &[u8], at: usize) -> u64 {
    b.get(at..at + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as u64)
}

fn u64_of(b: &[u8], at: usize) -> u64 {
    b.get(at..at + 8).and_then(|b| b.try_into().ok()).map_or(0, u64::from_le_bytes)
}

impl Linked {
    pub fn open(path: &Path) -> Option<Linked> {
        let file = std::fs::File::open(path).ok()?;
        let read = |at: u64, len: usize| {
            let mut buf = vec![0u8; len];
            file.read_exact_at(&mut buf, at).ok().map(|()| buf)
        };
        let head = read(0, 64)?;
        // (0x7f "ELF", 64-bit, little-endian.)
        if head[..4] != *b"\x7fELF" || head[4] != 2 || head[5] != 1 {
            return None;
        }
        let (phoff, phentsize, phnum) = (u64_of(&head, 0x20), u16_of(&head, 0x36), u16_of(&head, 0x38));
        if phentsize < 56 || phnum > 512 {
            return None;
        }
        // Its segments: what is loaded where (to find a table by its
        // address), and the dynamic one.
        let (mut loads, mut dynamic) = (Vec::new(), None);
        for i in 0..phnum {
            let ph = read(phoff + i * phentsize, 56)?;
            let (kind, offset, vaddr, filesz) = (u32_of(&ph, 0), u64_of(&ph, 8), u64_of(&ph, 16), u64_of(&ph, 32));
            match kind {
                1 => loads.push((vaddr, offset, filesz)),
                2 => dynamic = Some((offset, filesz)),
                _ => {}
            }
        }
        let (at, len) = dynamic?;
        if len > 1 << 20 {
            return None;
        }
        let table = read(at, len as usize)?;
        let dynamic: Vec<(u64, u64)> = table.chunks_exact(16).map(|e| (u64_of(e, 0), u64_of(e, 8))).take_while(|(tag, _)| *tag != 0).collect();
        let mut linked = Linked { file, dynamic, strings: 0, loads };
        // (DT_STRTAB.)
        linked.strings = linked.at(linked.tag(5)?)?;
        Some(linked)
    }

    fn tag(&self, tag: u64) -> Option<u64> {
        self.dynamic.iter().find(|(t, _)| *t == tag).map(|(_, v)| *v)
    }

    /// Where an address is in the file.
    fn at(&self, address: u64) -> Option<u64> {
        self.loads.iter().find(|(vaddr, _, filesz)| address >= *vaddr && address < vaddr + filesz).map(|(vaddr, offset, _)| offset + (address - vaddr))
    }

    fn read(&self, at: u64, len: usize) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; len];
        self.file.read_exact_at(&mut buf, at).ok().map(|()| buf)
    }

    /// The string at `name` in its strings. (A library's or a version's
    /// name is short; the file may end sooner than 256.)
    fn string(&self, name: u64) -> Option<String> {
        let mut buf = vec![0u8; 256];
        let got = self.file.read_at(&mut buf, self.strings + name).ok()?;
        let end = buf[..got].iter().position(|b| *b == 0)?;
        Some(String::from_utf8_lossy(&buf[..end]).into_owned())
    }

    /// The libraries it asks its loader for (`DT_NEEDED`).
    pub fn needed(&self) -> Vec<String> {
        self.dynamic.iter().filter(|(tag, _)| *tag == 1).filter_map(|(_, name)| self.string(*name)).collect()
    }

    /// The versions it needs of each library's symbols (`DT_VERNEED`:
    /// `libc.so.6` at `GLIBC_2.38`): a library older than the one it
    /// was linked against hasn't them, and the loader refuses the
    /// program ("version `GLIBC_2.43' not found").
    pub fn versions_needed(&self) -> Vec<(String, Vec<String>)> {
        let (Some(table), Some(count)) = (self.tag(0x6fff_fffe).and_then(|a| self.at(a)), self.tag(0x6fff_ffff)) else { return Vec::new() };
        let mut out = Vec::new();
        let mut at = table;
        for _ in 0..count.min(256) {
            // (vn_version, vn_cnt, vn_file, vn_aux, vn_next.)
            let Some(need) = self.read(at, 16) else { break };
            let (cnt, file, aux, next) = (u16_of(&need, 2), u32_of(&need, 4), u32_of(&need, 8), u32_of(&need, 12));
            let mut versions = Vec::new();
            let mut aux_at = at + aux;
            for _ in 0..cnt.min(256) {
                // (vna_hash, vna_flags, vna_other, vna_name, vna_next.)
                let Some(entry) = self.read(aux_at, 16) else { break };
                versions.extend(self.string(u32_of(&entry, 8)));
                match u32_of(&entry, 12) {
                    0 => break,
                    n => aux_at += n,
                }
            }
            if let Some(file) = self.string(file) {
                out.push((file, versions));
            }
            match next {
                0 => break,
                n => at += n,
            }
        }
        out
    }

    /// The versions of its symbols a library has (`DT_VERDEF`).
    pub fn versions_defined(&self) -> Vec<String> {
        let (Some(table), Some(count)) = (self.tag(0x6fff_fffc).and_then(|a| self.at(a)), self.tag(0x6fff_fffd)) else { return Vec::new() };
        let mut out = Vec::new();
        let mut at = table;
        for _ in 0..count.min(4096) {
            // (vd_version, vd_flags, vd_ndx, vd_cnt, vd_hash, vd_aux,
            // vd_next; its first aux entry is its own name.)
            let Some(def) = self.read(at, 20) else { break };
            let (aux, next) = (u32_of(&def, 12), u32_of(&def, 16));
            if let Some(entry) = self.read(at + aux, 8) {
                out.extend(self.string(u32_of(&entry, 0)));
            }
            match next {
                0 => break,
                n => at += n,
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A C library older than a program was linked against is said by
    /// the versions it lacks; one that isn't here at all is `missing`'s
    /// to say, not this.
    #[test]
    fn an_older_library_is_said_by_the_versions_it_lacks() {
        let needs = vec![
            ("libc.so.6".to_owned(), vec!["GLIBC_2.34".to_owned(), "GLIBC_2.43".to_owned(), "GLIBC_2.42".to_owned()]),
            ("libm.so.6".to_owned(), vec!["GLIBC_2.29".to_owned()]),
            ("libabsent.so.1".to_owned(), vec!["ABSENT_1".to_owned()]),
        ];
        let mut older = Default::default();
        lacking(&needs, &mut older, &mut |library| match library {
            "libc.so.6" => Some(vec!["GLIBC_2.2.5".into(), "GLIBC_2.34".into(), "GLIBC_2.39".into()]),
            "libm.so.6" => Some(vec!["GLIBC_2.29".into()]),
            _ => None,
        });
        assert_eq!(serde_json::to_string(&older).unwrap(), r#"{"libc.so.6":["GLIBC_2.42","GLIBC_2.43"]}"#);
    }

    /// This test's own program asks for the C library, at versions of
    /// its symbols this host's has (it runs); a script asks for nothing.
    #[test]
    fn a_programs_libraries_are_read_from_its_headers() {
        let me = Linked::open(&std::env::current_exe().unwrap()).expect("a test is a dynamic program");
        let libraries = me.needed();
        assert!(libraries.iter().any(|l| l.starts_with("libc.so")), "{libraries:?}");
        let script = std::env::temp_dir().join(format!("olo-screens-needed-{}", std::process::id()));
        std::fs::write(&script, "#!/bin/sh\necho hello\n").unwrap();
        assert!(Linked::open(&script).is_none());
        let _ = std::fs::remove_file(&script);
        // And this host has what it asks for, each at the versions it needs.
        let dirs = library_dirs();
        for library in &libraries {
            assert!(dirs.iter().any(|d| d.join(library).exists()), "{library} in none of {dirs:?}");
        }
        let versions = me.versions_needed();
        let (_, of_c) = versions.iter().find(|(library, _)| library.starts_with("libc.so")).expect("versions of the C library's symbols");
        assert!(of_c.iter().any(|v| v.starts_with("GLIBC_2.")), "{of_c:?}");
        for (library, needs) in &versions {
            let here = dirs.iter().map(|d| d.join(library)).find(|p| p.exists()).and_then(|p| Linked::open(&p)).map(|l| l.versions_defined()).unwrap_or_default();
            assert!(here.len() >= needs.len(), "{library} defines {here:?}");
            for version in needs {
                assert!(here.contains(version), "{library} has no {version}: {here:?}");
            }
        }
    }
}

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
//! there: no root, nothing outside the home).
//!
//! ```text
//! olo-rex-link screen install [--tag TAG | --from ARCHIVE] [--no-chrome]
//!     Olo put here (the newest release; the one tagged TAG; ARCHIVE, a
//!     file or a URL with its .sha256 beside it), or brought up to it,
//!     and Chrome: a line of JSON for each step as it goes, then what
//!     `status` says (status 3: it is put, and this host lacks something
//!     its screens need, which `missing` and `how` say)
//! olo-rex-link screen status
//!     what is here, as JSON: {"installed": VERSION|null, "prefix",
//!     "bytes", "chrome": VERSION|null, "missing": [libraries its
//!     programs need that this host hasn't], "software_drawing":
//!     true|false, "default": "host"|"desktop"}
//! olo-rex-link screen default [host|desktop]
//!     where `olo screen` goes on this host when it isn't said
//!     (`--on host`, `--on desktop`, `OLO_SCREENS_ON`); none: says it
//! olo-rex-link screen remove
//!     Olo taken off this host again (Chrome and the screens' own
//!     folders stay)
//! olo-rex-link screen COMMAND…
//!     `olo-screen COMMAND…` of the Olo here: start, run, look, tree,
//!     click, type, key, stop and the rest, as on the desktop
//! ```

use std::os::unix::fs::{FileExt as _, MetadataExt as _};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use serde_json::json;

const USAGE: &str = "usage: olo screen install [--tag TAG | --from ARCHIVE] [--no-chrome]   Olo and Chrome put on this host for screens here
       olo screen status                    what is here, as JSON
       olo screen default [host|desktop]    where `olo screen` goes on this host when it isn't said
       olo screen remove                    Olo taken off this host again
       olo screen --on host COMMAND…        olo-screen's own commands, on this host (start, run, look, tree, click, …)";

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
        Some(_) => exec(&args),
    }
}

/// `olo-screen ARGS…` of the Olo here.
fn exec(args: &[String]) -> ! {
    let prefix = prefix();
    let program = prefix.join("bin/olo-screen");
    if !program.is_file() {
        fail(format_args!("Olo isn't on this host yet: `olo screen install` puts it here, under {} (about 600 MB to download; nothing needs root)", prefix.display()));
    }
    // (Its own programs first: `olo screen run NAME olo-chrome …`.)
    let path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(std::iter::once(prefix.join("bin")).chain(std::env::split_paths(&path))).unwrap_or(path);
    let e = Command::new(&program).args(args).env("PATH", path).exec();
    fail(format_args!("{}: {e}", program.display()));
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
    let (changed, version) = match olo_release::install(&prefix, &ask, &AtomicBool::new(false), &mut |doing, done, all| told.tell(doing, done, all)) {
        Ok(done) => done,
        Err(e) => {
            say(json!({ "error": e }));
            std::process::exit(1);
        }
    };
    say(json!({ "installed": version, "changed": changed.len() }));
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
    let now = status();
    let ready = now["missing"].as_array().is_some_and(Vec::is_empty) && now["software_drawing"] == true;
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
    let mut programs = 0;
    for entry in std::fs::read_dir(prefix.join("bin")).into_iter().flatten().flatten() {
        let Some(libraries) = needed(&entry.path()) else { continue };
        programs += 1;
        for library in libraries {
            if !has(&library) && !missing.contains(&library) {
                missing.push(library);
            }
        }
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
        "software_drawing": software,
        "default": default_now(),
    });
    if installed.is_none() {
        v["how"] = json!("olo screen install");
    } else if !software {
        v["how"] =
            json!("Mesa's EGL and software renderer aren't here: on Fedora, `sudo dnf install mesa-libEGL mesa-dri-drivers`; on Debian or Ubuntu, `sudo apt install libegl-mesa0 libgl1-mesa-dri`");
    }
    v
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

/// The libraries the program at `path` asks its loader for (its
/// `DT_NEEDED`), read from the file's own headers: a 64-bit ELF of this
/// machine's byte order, linked to libraries at all. None for anything
/// else (a script, a static program).
pub fn needed(path: &Path) -> Option<Vec<String>> {
    let file = std::fs::File::open(path).ok()?;
    let read = |at: u64, len: usize| {
        let mut buf = vec![0u8; len];
        file.read_exact_at(&mut buf, at).ok().map(|()| buf)
    };
    let u16_of = |b: &[u8], at: usize| u16::from_le_bytes([b[at], b[at + 1]]) as u64;
    let u32_of = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap_or_default());
    let u64_of = |b: &[u8], at: usize| u64::from_le_bytes(b[at..at + 8].try_into().unwrap_or_default());
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
    let (mut names, mut strings) = (Vec::new(), None);
    for entry in table.chunks_exact(16) {
        match (u64_of(entry, 0), u64_of(entry, 8)) {
            (0, _) => break,
            (1, name) => names.push(name),
            (5, address) => strings = Some(address),
            _ => {}
        }
    }
    let strings = strings?;
    let (vaddr, offset, _) = loads.into_iter().find(|(vaddr, _, filesz)| strings >= *vaddr && strings < vaddr + filesz)?;
    let strings = offset + (strings - vaddr);
    let mut needed = Vec::new();
    for name in names {
        // (A library's name is short; the file may end sooner than 256.)
        let mut buf = vec![0u8; 256];
        let got = file.read_at(&mut buf, strings + name).ok()?;
        let end = buf[..got].iter().position(|b| *b == 0)?;
        needed.push(String::from_utf8_lossy(&buf[..end]).into_owned());
    }
    Some(needed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This test's own program asks for the C library, and a script for
    /// nothing.
    #[test]
    fn a_programs_libraries_are_read_from_its_headers() {
        let me = std::env::current_exe().unwrap();
        let libraries = needed(&me).expect("a test is a dynamic program");
        assert!(libraries.iter().any(|l| l.starts_with("libc.so")), "{libraries:?}");
        let script = std::env::temp_dir().join(format!("olo-screens-needed-{}", std::process::id()));
        std::fs::write(&script, "#!/bin/sh\necho hello\n").unwrap();
        assert_eq!(needed(&script), None);
        let _ = std::fs::remove_file(&script);
        // And this host has what it asks for.
        let dirs = library_dirs();
        for library in libraries {
            assert!(dirs.iter().any(|d| d.join(&library).exists()), "{library} in none of {dirs:?}");
        }
    }
}

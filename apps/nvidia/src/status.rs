//! Keep partial findings. A broken module or a protected firmware
//! variable should explain its own row, not hide the other kernels.
use crate::{
    boot, build,
    modules::{self, OURS, STOCK},
    system::{self, Manifest, Outcome, System},
};
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use std::{collections::BTreeMap, fs, path::Path};

#[derive(Serialize)]
pub struct Status {
    pub running_kernel: String,
    pub secure_boot: Option<bool>,
    pub secure_boot_error: Option<String>,
    pub opted_in: Option<bool>,
    pub opt_in_error: Option<String>,
    pub supported_version: &'static str,
    pub kernels: Vec<Kernel>,
}
#[derive(Serialize)]
pub struct Kernel {
    pub release: String,
    pub stock_version: Option<String>,
    pub supported: bool,
    pub olo_present: bool,
    pub olo_version: Option<String>,
    pub manifest: Option<Manifest>,
    pub next_boot: &'static str,
    pub index: modules::Paths,
    pub loaded: Option<&'static str>,
    pub loaded_build_id: Option<String>,
    pub loaded_modules: Option<BTreeMap<String, ResidentModule>>,
    pub last_build: Option<Outcome>,
    pub last_check: Option<Outcome>,
    pub errors: Vec<String>,
}
fn read<T>(result: Result<T>, errors: &mut Vec<String>) -> Option<T> {
    match result {
        Ok(t) => Some(t),
        Err(e) => {
            errors.push(format!("{e:#}"));
            None
        }
    }
}
fn optional<T: serde::de::DeserializeOwned>(path: &Path, errors: &mut Vec<String>) -> Option<T> {
    if path.exists() { read(system::json(path), errors) } else { None }
}
fn core(set: Option<modules::Set>, dir: &Path) -> Option<modules::Module> {
    if let Some(mut complete) = set {
        return complete.remove("nvidia");
    }
    // The core's build ID is still evidence even if another file from
    // the set went missing during a package update.
    let mut paths = ["ko", "ko.xz", "ko.zst"].map(|e| dir.join(format!("nvidia.{e}"))).into_iter().filter(|p| p.is_file());
    let path = paths.next()?;
    if paths.next().is_some() {
        return None;
    }
    modules::read(&path, "nvidia").ok()
}
#[derive(Serialize)]
pub struct ResidentModule {
    pub initstate: &'static str,
    pub build_id: Option<String>,
    pub origin: Option<&'static str>,
}

fn disk_id(set: Option<&modules::Set>, dir: &Path, name: &str) -> Result<Option<String>> {
    if let Some(set) = set {
        return Ok(set.get(name).map(|m| m.build_id.clone()));
    }
    let mut id = None;
    for ext in ["ko", "ko.xz", "ko.zst"] {
        let p = dir.join(format!("{name}.{ext}"));
        if p.try_exists()? {
            ensure!(id.is_none(), "ambiguous build ID for {name} in {}", dir.display());
            id = Some(modules::read(&p, name)?.build_id);
        }
    }
    Ok(id)
}

fn initstate(dir: &Path) -> Result<&'static str> {
    let path = dir.join("initstate");
    match fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?.trim() {
        "live" => Ok("live"),
        "coming" => Ok("coming"),
        "going" => Ok("going"),
        other => bail!("unexpected module state {other:?} in {}", path.display()),
    }
}

fn residents(sys: &Path, kernel: &Path, stock: Option<&modules::Set>, ours: Option<&modules::Set>, errors: &mut Vec<String>) -> BTreeMap<String, ResidentModule> {
    let mut seen = BTreeMap::new();
    if read(fs::read_dir(sys).with_context(|| format!("reading {}", sys.display())), errors).is_none() {
        return seen;
    }
    for n in modules::NAMES {
        let dir = sys.join(n.replace('-', "_"));
        let exists = read(dir.try_exists().with_context(|| format!("checking {}", dir.display())), errors);
        let mut row = ResidentModule { initstate: "unknown", build_id: None, origin: None };
        match exists {
            Some(false) => row.initstate = "absent",
            Some(true) => {
                let before = read(initstate(&dir), errors);
                let note = dir.join("notes/.note.gnu.build-id");
                let id = fs::read(&note).with_context(|| format!("reading {}", note.display())).and_then(|b| modules::loaded_id(&b).with_context(|| format!("invalid GNU build ID for {n}")));
                row.build_id = read(id, errors);
                if let Some(id) = &row.build_id {
                    let a = read(disk_id(stock, &kernel.join(STOCK), n), errors);
                    let b = read(disk_id(ours, &kernel.join(OURS), n), errors);
                    row.origin = Some(match (a, b) {
                        (Some(s), Some(o)) => match (s.as_ref() == Some(id), o.as_ref() == Some(id)) {
                            (true, true) => "both",
                            (true, false) => "stock",
                            (false, true) => "olo",
                            _ => "unknown",
                        },
                        _ => "unknown",
                    });
                }
                let after = read(initstate(&dir), errors);
                if before == after {
                    row.initstate = before.unwrap_or("unknown");
                }
            }
            None => {}
        }
        seen.insert(n.to_owned(), row);
    }
    seen
}

fn loaded_set(seen: &BTreeMap<String, ResidentModule>) -> &'static str {
    for state in ["going", "coming", "unknown"] {
        if seen.values().any(|m| m.initstate == state) {
            return state;
        }
    }
    if seen.len() != modules::NAMES.len() {
        return "unknown";
    }
    if seen.values().all(|m| m.initstate == "absent") {
        return "unloaded";
    }
    if seen.get("nvidia").is_none_or(|m| m.initstate != "live") {
        return "unknown";
    }
    let has = |name| seen.values().any(|m| m.origin == Some(name));
    if has("stock") && has("olo") {
        return "mixed";
    }
    if seen.values().any(|m| m.initstate == "live" && m.origin.is_none_or(|o| o == "unknown")) {
        return "unknown";
    }
    if has("olo") {
        "olo"
    } else if has("stock") {
        "stock"
    } else {
        "both"
    }
}

pub fn status(s: &System) -> Result<Status> {
    let running = s.running()?;
    let secure = boot::secure_boot(&s.root);
    let secure_boot_error = secure.as_ref().err().map(|e| format!("{e:#}"));
    let mut kernels = vec![];
    for r in s.releases()? {
        let k = s.kernel(&r);
        let olo_present = k.join(OURS).is_dir();
        if !olo_present && !k.join(STOCK).is_dir() && !s.manifest(&r).is_file() && !s.outcome(&r).is_file() {
            continue;
        }
        let mut errors = vec![];
        let stock = read(modules::set(&k.join(STOCK), &r), &mut errors);
        let ours = olo_present.then(|| read(modules::set(&k.join(OURS), &r), &mut errors)).flatten();
        let index = read(modules::index(&k), &mut errors).unwrap_or_default();
        let binary = read(modules::verify_binary(&k, &index), &mut errors).is_some();
        let next =
            |dir: &str, set: &Option<modules::Set>| binary && set.is_some() && modules::NAMES.iter().all(|n| index.get(*n).is_some_and(|p| p.parent() == Some(Path::new(dir)) && k.join(p).is_file()));
        let next_boot = if next(STOCK, &stock) {
            "stock"
        } else if next(OURS, &ours) {
            "olo"
        } else if index.len() == 5 {
            "mixed_or_unknown"
        } else {
            "unknown"
        };
        let loaded_modules = (r == running).then(|| residents(&s.root.join("sys/module"), &k, stock.as_ref(), ours.as_ref(), &mut errors));
        let loaded = loaded_modules.as_ref().map(loaded_set);
        let loaded_build_id = loaded_modules.as_ref().and_then(|m| m.get("nvidia")).and_then(|m| m.build_id.clone());
        let stock = core(stock, &k.join(STOCK));
        let ours = core(ours, &k.join(OURS));
        let stock_version = stock.map(|m| m.version);
        let manifest = if olo_present { read(system::json(&s.manifest(&r)), &mut errors) } else { optional(&s.manifest(&r), &mut errors) };
        kernels.push(Kernel {
            supported: stock_version.as_deref() == Some(build::VERSION),
            stock_version,
            olo_present,
            olo_version: ours.map(|m| m.version),
            manifest,
            next_boot,
            index,
            loaded,
            loaded_build_id,
            loaded_modules,
            last_build: optional(&s.outcome(&r), &mut errors),
            last_check: optional(&s.state().join("checks").join(format!("{r}.json")), &mut errors),
            errors,
            release: r,
        });
    }
    let flag = s.opted_in();
    let opt_in_error = flag.as_ref().err().map(|e| format!("{e:#}"));
    Ok(Status { running_kernel: running, secure_boot: secure.ok(), secure_boot_error, opted_in: flag.ok(), opt_in_error, supported_version: build::VERSION, kernels })
}

pub fn print(status: &Status) {
    let secure = match status.secure_boot {
        Some(true) => "on",
        Some(false) => "off",
        None => "unknown",
    };
    println!(
        "Running kernel {}. Secure Boot {secure}. Follow kernel updates: {}.",
        status.running_kernel,
        match status.opted_in {
            Some(true) => "yes",
            Some(false) => "no",
            None => "unknown",
        }
    );
    if let Some(e) = &status.opt_in_error {
        println!("{e}");
    }
    if let Some(e) = &status.secure_boot_error {
        println!("{e}");
    }
    for k in &status.kernels {
        println!(
            "{}  distribution {}  Olo {}  next start {}{}",
            k.release,
            k.stock_version.as_deref().unwrap_or("unknown"),
            k.olo_version.as_deref().unwrap_or(if k.olo_present { "unknown" } else { "absent" }),
            k.next_boot,
            k.loaded.map_or(String::new(), |l| format!("  loaded set {l}"))
        );
        println!("  Prevention patches available for this version: {}.", if k.supported { "yes" } else { "no" });
        if let Some(m) = &k.manifest {
            println!("  {} built at {} (Unix time); patches: {}", m.set, m.built_at, m.patches.keys().cloned().collect::<Vec<_>>().join(", "));
        }
        if let Some(id) = &k.loaded_build_id {
            println!("  Loaded core build ID {id}");
        }
        if let Some(o) = &k.last_build {
            println!("  Last build: {}", o.message);
        }
        if let Some(o) = &k.last_check {
            println!("  Last withdrawal check: {}", o.message);
        }
        for e in &k.errors {
            println!("  {e}");
        }
    }
}

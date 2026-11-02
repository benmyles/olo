//! olo-reentry: finds where a GPUI entity is read or updated while it is
//! already being updated, which panics when it runs ("cannot read X while
//! it is already being updated").
//!
//! `Entity<T>::update` moves the entity out of the app while its closure
//! runs (GPUI calls it a lease), so reading or updating it again from
//! inside, at any depth of calls, panics. The compiler can't see that:
//! entities are shared handles, checked when used, like `Rc<RefCell<T>>`.
//! This rustc driver walks the MIR instead: from every update of an
//! `Entity<T>` (a `WeakEntity<T>`'s, a `WindowHandle<T>`'s root), through
//! the closure it runs and every call made from it (into Olo's crates;
//! closures handed to std, third-party code and GPUI's synchronous
//! helpers: `cx.new`, `open_window`, `read_with`, `with_…`), and reports a
//! read or update of an entity of the same type T.
//!
//! Closures kept for later (a button's `on_click`, `cx.defer`,
//! `cx.spawn`, listeners) aren't followed: they don't run inside the
//! update. Two entities of one type are one to it, so an update of one
//! that reads another of its type is reported too: `// reentry: ok` on
//! the line (or the line above) says it's meant.
//!
//! It runs as cargo's RUSTC_WRAPPER (`tools/dev/lockcheck reentry`):
//! Olo's crates cargo was asked to check are analysed, everything else is
//! compiled as it is. Findings go to `$OLO_REENTRY_OUT/<crate>.txt`.

#![feature(rustc_private)]

extern crate rustc_driver;
extern crate rustc_hir;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_span;

use std::{collections::HashSet, path::PathBuf, process::Command};

use rustc_hir::{
    def::DefKind,
    def_id::{DefId, LOCAL_CRATE},
};
use rustc_middle::{
    mir::TerminatorKind,
    ty::{self, EarlyBinder, GenericArgs, GenericArgsRef, Instance, InstanceKind, Ty, TyCtxt, TypingEnv},
};
use rustc_span::Span;

/// GPUI calls that update an entity, running a closure meanwhile.
const UPDATES: &[&str] = &["update", "update_in", "update_entity", "update_window_entity"];
/// GPUI calls that read an entity.
const READS: &[&str] = &["read", "read_with", "read_entity", "as_mut"];
/// GPUI calls that run the closure they're given before they return.
const RUN_NOW: &[&str] = &[
    "new",
    "new_window_entity",
    "insert_entity",
    "open_window",
    "update",
    "update_in",
    "update_window",
    "read_with",
    "read_window",
    "update_global",
    "update_default_global",
    "read_global",
    "observe_new",
];
/// Calls into std and other crates that keep a closure, not call it.
const KEEPS: &[&str] = &[
    "new", "clone", "drop", "from", "into", "push", "push_back", "push_front", "insert", "extend", "spawn", "send", "set", "replace", "store", "swap",
];

/// An entity being updated.
#[derive(Clone, Copy)]
struct Updating<'tcx> {
    ty: Ty<'tcx>,
    at: Span,
}

struct Finding<'tcx> {
    at: Span,
    what: &'static str,
    ty: Ty<'tcx>,
    began: Span,
    via: Vec<Span>,
}

struct Walker<'tcx> {
    tcx: TyCtxt<'tcx>,
    env: TypingEnv<'tcx>,
    seen: HashSet<(Instance<'tcx>, Vec<Ty<'tcx>>)>,
    findings: Vec<Finding<'tcx>>,
    reported: HashSet<(Span, Span)>,
}

const DEEPEST: usize = 48;

impl<'tcx> Walker<'tcx> {
    fn walk(&mut self, inst: Instance<'tcx>, ctx: &mut Vec<Updating<'tcx>>, via: &mut Vec<Span>) {
        if via.len() > DEEPEST || !matches!(inst.def, InstanceKind::Item(_) | InstanceKind::ClosureOnceShim { .. }) {
            return;
        }
        if !self.seen.insert((inst, ctx.iter().map(|u| u.ty).collect())) {
            return;
        }
        let tcx = self.tcx;
        let def = inst.def_id();
        if matches!(inst.def, InstanceKind::Item(_)) && (!tcx.is_mir_available(def) || tcx.is_coroutine(def)) {
            return;
        }
        let body = tcx.instance_mir(inst.def);
        for block in body.basic_blocks.iter() {
            let Some(term) = &block.terminator else { continue };
            let TerminatorKind::Call { func, args, .. } = &term.kind else { continue };
            let mono = |ty: Ty<'tcx>| inst.try_instantiate_mir_and_normalize_erasing_regions(tcx, self.env, EarlyBinder::bind(ty)).ok();
            let Some(fty) = mono(func.ty(&body.local_decls, tcx)) else { continue };
            let ty::FnDef(callee, cargs) = *fty.kind() else { continue };
            let Some(arg_tys) = args.iter().map(|a| mono(a.node.ty(&body.local_decls, tcx))).collect::<Option<Vec<_>>>() else { continue };
            self.call(callee, cargs, &arg_tys, term.source_info.span, ctx, via);
        }
    }

    fn call(&mut self, callee: DefId, cargs: GenericArgsRef<'tcx>, arg_tys: &[Ty<'tcx>], span: Span, ctx: &mut Vec<Updating<'tcx>>, via: &mut Vec<Span>) {
        let tcx = self.tcx;
        let name = tcx.opt_item_name(callee).map(|s| s.to_string()).unwrap_or_default();
        if tcx.crate_name(callee.krate).as_str() == "gpui" {
            let entity = arg_tys.iter().find_map(|t| entity_of(tcx, *t));
            if let Some(ty) = entity {
                let updates = UPDATES.contains(&name.as_str());
                if updates || READS.contains(&name.as_str()) {
                    if let Some(u) = ctx.iter().find(|u| u.ty == ty).copied() {
                        if self.reported.insert((span, u.at)) {
                            self.findings.push(Finding { at: span, what: if updates { "updates" } else { "reads" }, ty, began: u.at, via: via.clone() });
                        }
                        // (It panics there: nothing after it runs.)
                        return;
                    }
                }
                if updates {
                    ctx.push(Updating { ty, at: span });
                    via.push(span);
                    for c in self.closures(arg_tys, false) {
                        self.walk(c, ctx, via);
                    }
                    via.pop();
                    ctx.pop();
                    return;
                }
            }
            if RUN_NOW.contains(&name.as_str()) || name.starts_with("with_") {
                via.push(span);
                for c in self.closures(arg_tys, false) {
                    self.walk(c, ctx, via);
                }
                via.pop();
            }
            return;
        }
        let Ok(Some(inst)) = Instance::try_resolve(tcx, self.env, callee, cargs) else { return };
        let def = inst.def_id();
        if def.krate == LOCAL_CRATE || tcx.crate_name(def.krate).as_str().starts_with("olo") || tcx.is_closure_like(def) {
            via.push(span);
            self.walk(inst, ctx, via);
            via.pop();
        } else if !KEEPS.contains(&name.as_str()) {
            // std and the rest: what it's handed may be called now
            // (`Option::map`, an iterator's `for_each`, `LocalKey::with`).
            for c in self.closures(arg_tys, true) {
                self.walk(c, ctx, via);
            }
        }
    }

    /// The closures (and functions) among a call's arguments; `deep`: in
    /// the types holding them too (an iterator's `Map<I, F>`), though not
    /// in what a closure captured.
    fn closures(&self, tys: &[Ty<'tcx>], deep: bool) -> Vec<Instance<'tcx>> {
        fn find<'tcx>(w: &Walker<'tcx>, ty: Ty<'tcx>, deep: bool, depth: usize, out: &mut Vec<Instance<'tcx>>) {
            if depth > 8 {
                return;
            }
            match *ty.kind() {
                ty::Ref(_, inner, _) => find(w, inner, deep, depth + 1, out),
                ty::Closure(def, args) => out.push(Instance::new_raw(def, args)),
                ty::FnDef(def, args) => {
                    if let Ok(Some(i)) = Instance::try_resolve(w.tcx, w.env, def, args) {
                        out.push(i);
                    }
                }
                ty::Adt(_, args) if deep => {
                    for t in args.types() {
                        find(w, t, deep, depth + 1, out);
                    }
                }
                ty::Tuple(tys) if deep => {
                    for t in tys {
                        find(w, t, deep, depth + 1, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for t in tys {
            find(self, *t, deep, 0, &mut out);
        }
        out
    }
}

/// T, for a GPUI `Entity<T>`, `WeakEntity<T>` or `WindowHandle<T>`.
fn entity_of<'tcx>(tcx: TyCtxt<'tcx>, ty: Ty<'tcx>) -> Option<Ty<'tcx>> {
    match *ty.kind() {
        ty::Ref(_, inner, _) => entity_of(tcx, inner),
        ty::Adt(adt, args) if tcx.crate_name(adt.did().krate).as_str() == "gpui" => {
            matches!(tcx.item_name(adt.did()).as_str(), "Entity" | "WeakEntity" | "WindowHandle").then(|| args.type_at(0))
        }
        _ => None,
    }
}

fn analyse<'tcx>(tcx: TyCtxt<'tcx>) -> Vec<Finding<'tcx>> {
    let mut w = Walker { tcx, env: TypingEnv::fully_monomorphized(), seen: HashSet::new(), findings: Vec::new(), reported: HashSet::new() };
    for &local in tcx.mir_keys(()) {
        let def = local.to_def_id();
        if !matches!(tcx.def_kind(def), DefKind::Fn | DefKind::AssocFn | DefKind::Closure) || tcx.is_coroutine(def) || tcx.generics_of(def).requires_monomorphization(tcx) {
            continue;
        }
        let inst = Instance::new_raw(def, GenericArgs::identity_for_item(tcx, def));
        w.walk(inst, &mut Vec::new(), &mut Vec::new());
    }
    w.findings
}

/// "path:line:col".
fn place(tcx: TyCtxt<'_>, span: Span) -> String {
    let s = tcx.sess.source_map().span_to_diagnostic_string(span.source_callsite());
    s.split_once(": ").map(|(a, _)| a.to_owned()).unwrap_or(s)
}

/// Whether the line (or the one above) says the finding is meant.
fn allowed(tcx: TyCtxt<'_>, span: Span) -> bool {
    let loc = tcx.sess.source_map().lookup_char_pos(span.source_callsite().lo());
    let line = |n: usize| loc.file.get_line(n).is_some_and(|l| l.contains("reentry: ok"));
    line(loc.line.saturating_sub(1)) || (loc.line >= 2 && line(loc.line - 2))
}

fn report(tcx: TyCtxt<'_>, findings: &[Finding<'_>]) {
    let krate = tcx.crate_name(LOCAL_CRATE).to_string();
    let mut text = String::new();
    for f in findings.iter().filter(|f| !allowed(tcx, f.at)) {
        let via: Vec<String> = f.via.iter().map(|s| place(tcx, *s)).collect();
        text += &format!(
            "{}: {} {} while it is being updated\n    the update began at {}\n    via {}\n",
            place(tcx, f.at),
            f.what,
            f.ty,
            place(tcx, f.began),
            via.join(" → ")
        );
    }
    if !text.is_empty() {
        eprint!("{text}");
    }
    if let Some(dir) = std::env::var_os("OLO_REENTRY_OUT") {
        let dir = PathBuf::from(dir);
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(format!("{krate}.txt")), text);
    }
}

struct Check;

impl rustc_driver::Callbacks for Check {
    fn after_analysis<'tcx>(&mut self, _: &rustc_interface::interface::Compiler, tcx: TyCtxt<'tcx>) -> rustc_driver::Compilation {
        if tcx.crate_name(LOCAL_CRATE).as_str().starts_with("olo") {
            report(tcx, &analyse(tcx));
        }
        rustc_driver::Compilation::Continue
    }
}

fn main() -> std::process::ExitCode {
    let mut args: Vec<String> = std::env::args().collect();
    // Cargo runs `$RUSTC_WRAPPER rustc ARGS`.
    let rustc = if args.len() > 1 && std::path::Path::new(&args[1]).file_stem().is_some_and(|s| s == "rustc") { Some(args.remove(1)) } else { None };
    let analysed = std::env::var_os("CARGO_PRIMARY_PACKAGE").is_some() && !args.iter().any(|a| a == "build_script_build" || a.starts_with("--print") || a == "-vV" || a == "-V");
    if !analysed {
        // Everything else is the nightly's own rustc's to compile, and a
        // build script asking which rustc this is hears a stable one, so
        // it doesn't turn on nightly-only features (pathfinder_simd).
        let Some(rustc) = rustc else { return std::process::ExitCode::FAILURE };
        let probing = args.iter().any(|a| a == "-vV") && std::env::var_os("CARGO_PKG_NAME").is_some();
        let mut cmd = Command::new(rustc);
        cmd.args(&args[1..]);
        if probing {
            let Ok(out) = cmd.output() else { return std::process::ExitCode::FAILURE };
            print!("{}", String::from_utf8_lossy(&out.stdout).replace("-nightly", ""));
            return if out.status.success() { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::FAILURE };
        }
        use std::os::unix::process::CommandExt;
        let e = cmd.exec();
        eprintln!("olo-reentry: {e}");
        return std::process::ExitCode::FAILURE;
    }
    // (lockcheck says where the nightly's std is.)
    if !args.iter().any(|a| a.starts_with("--sysroot"))
        && let Some(root) = std::env::var_os("OLO_REENTRY_SYSROOT")
    {
        args.push(format!("--sysroot={}", root.to_string_lossy()));
    }
    // Every function's MIR in the metadata, so a crate using another's
    // (Preferences using olo-ui's) can walk into it.
    args.push("-Zalways-encode-mir".into());
    rustc_driver::install_ice_hook("tools/reentry in Olo", |_| ());
    rustc_driver::catch_with_exit_code(|| rustc_driver::run_compiler(&args, &mut Check))
}

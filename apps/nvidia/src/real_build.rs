//! Tests against NVIDIA's source itself: a git checkout of the pinned
//! commit (`OLO_NVIDIA_BASELINE`) and one with the series applied by
//! git (`OLO_NVIDIA_PATCHED`), as `kernel/nvidia/prepare-and-build.sh`
//! makes them. Without them the comparison is skipped and the build
//! can't run.
//!
//! A checkout that was built in holds the products of its own build.
//! Copy only tracked source from its index, never an object, generated
//! header, or prelinked core from the old build.
use crate::{build, modules, patches};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

fn checkout(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_dir())
}

/// A scratch folder on disk (`OLO_NVIDIA_TEST_DIR`, or /var/tmp: never
/// /tmp, which is memory, and a build is gigabytes).
pub fn scratch() -> tempfile::TempDir {
    let dir = std::env::var_os("OLO_NVIDIA_TEST_DIR").map(PathBuf::from).unwrap_or_else(|| "/var/tmp".into());
    tempfile::Builder::new().prefix("olo-nvidia-test-").tempdir_in(dir).unwrap()
}

fn tracked(base: &Path) -> Vec<PathBuf> {
    // (A worktree's `.git` is a file naming its folder; a clone's is
    // the folder.)
    let dot = base.join(".git");
    let git = if dot.is_dir() { dot } else { PathBuf::from(fs::read_to_string(&dot).unwrap().trim().strip_prefix("gitdir: ").unwrap()) };
    let index = fs::read(git.join("index")).unwrap();
    assert_eq!(&index[..8], b"DIRC\0\0\0\x02");
    let entries = u32::from_be_bytes(index[8..12].try_into().unwrap());
    assert_eq!(entries, 4005);
    let mut start = 12;
    (0..entries)
        .map(|_| {
            let name = &index[start + 62..];
            let len = name.iter().position(|b| *b == 0).unwrap();
            start += (62 + len + 1).next_multiple_of(8);
            let path = PathBuf::from(std::str::from_utf8(&name[..len]).unwrap());
            assert!(path.components().all(|p| matches!(p, std::path::Component::Normal(_))));
            path
        })
        .collect()
}

#[test]
fn patches_match_the_reviewed_checkout_and_refuse_changed_context() {
    assert_eq!(patches::hashes().unwrap().len(), 4);
    let (Some(base), Some(patched)) = (checkout("OLO_NVIDIA_BASELINE"), checkout("OLO_NVIDIA_PATCHED")) else {
        eprintln!("Skipped the comparison with NVIDIA's source: OLO_NVIDIA_BASELINE and OLO_NVIDIA_PATCHED name no checkouts");
        return;
    };
    let scratch = scratch();
    let names: Vec<_> = patches::PREVENTION
        .iter()
        .map(|(_, p)| *p)
        .chain([include_str!("../fedora-open.patch")])
        .flat_map(str::lines)
        .filter_map(|l| l.strip_prefix("--- a/"))
        .map(|p| p.split_ascii_whitespace().next().unwrap())
        .collect();
    for p in &names {
        let out = scratch.path().join(p);
        fs::create_dir_all(out.parent().unwrap()).unwrap();
        fs::copy(base.join(p), out).unwrap();
    }
    patches::prepare(scratch.path()).unwrap();
    for p in &names {
        assert!(fs::read(scratch.path().join(p)).unwrap() == fs::read(patched.join(p)).unwrap(), "{p}");
    }
    let path = scratch.path().join(names[0]);
    let original = fs::read_to_string(base.join(names[0])).unwrap();
    fs::write(path, original.replacen("#include \"gpu/fsp/kern_fsp.h\"", "#include \"gpu/fsp/kern_fsp.X\"", 1)).unwrap();
    assert!(patches::apply(scratch.path(), patches::PREVENTION[0].1).unwrap_err().to_string().contains("context differs"));
}

#[test]
#[ignore = "compiles all five modules from a checkout of NVIDIA's source (OLO_NVIDIA_BASELINE) against the installed kernel-devel: a minute or two on every core"]
fn prevention_builds_for_this_kernel() {
    assert_ne!(unsafe { libc::geteuid() }, 0, "run this test as an ordinary user");
    let base = checkout("OLO_NVIDIA_BASELINE").expect("OLO_NVIDIA_BASELINE: a git checkout of NVIDIA's open-gpu-kernel-modules at the pinned commit");
    let scratch = scratch();
    let work = build::Work::new(scratch.path()).unwrap();
    let start = Instant::now();
    for name in tracked(&base) {
        let dest = work.tree().join(&name);
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fs::copy(base.join(name), dest).unwrap();
    }
    patches::prepare(&work.tree()).unwrap();
    let release = olo_kmod::release();
    let kernel = Path::new("/lib/modules").join(&release);
    work.compile(&kernel.join("build").canonicalize().unwrap(), &release).unwrap();
    let stock = modules::set(&kernel.join(modules::STOCK), &release).unwrap();
    for (name, m) in modules::set(&work.tree().join("kernel-open"), &release).unwrap() {
        assert_eq!(m.version, build::VERSION);
        assert_eq!((m.version.as_str(), m.vermagic.as_str()), (stock[&name].version.as_str(), stock[&name].vermagic.as_str()));
        println!("{name}  {}  {:?}  {}", m.version, m.vermagic, m.build_id);
    }
    println!("Five checked in {:.2?}", start.elapsed());
}
